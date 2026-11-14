//! The stats screen of one deck: the query on the worker thread, and the
//! calendar month, which is a slice of the loaded data.

use std::rc::Rc;

use chrono::Datelike;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::FrontendHooks;
use super::card_jobs::{DECK_LOADING_DELAY, spawn_collection_job};
use super::sync::end_finished_sync;
use crate::session::{LearnSession, StatsState};
use crate::stats::{self, StatsData, month_levels};
use crate::{MainWindow, StatsView};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

pub(super) fn wire(ui: &MainWindow, session: &Rc<LearnSession>, hooks: &FrontendHooks) {
    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let hooks = hooks.clone();
        ui.on_stats_clicked(move |deck| {
            if session.busy.get() {
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                end_finished_sync(&ui);
            }
            open_stats(&ui_weak, &session, &hooks, deck.id, deck.name.to_string());
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let hooks = hooks.clone();
        ui.on_stats_back(move || {
            if session.busy.get() {
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                let was_open = ui.get_stats_open();
                end_finished_sync(&ui);
                ui.set_stats_open(false);
                *session.stats.borrow_mut() = None;
                if was_open {
                    hooks.request_full_present(&ui);
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        // UI thread only: the month is a slice of the loaded data.
        ui.on_stats_month_changed(move |delta| {
            if session.busy.get() {
                return;
            }
            let mut stats = session.stats.borrow_mut();
            let Some(state) = stats.as_mut() else {
                return;
            };
            let today = state.data.timing.today;
            let (year, month) = shift_month(
                (state.year, state.month),
                delta,
                state.data.first_month(),
                (today.year(), today.month()),
            );
            state.year = year;
            state.month = month;
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_stats(stats_view(&state.data, year, month));
                // See notes/card-loading.md, "Explicit redraws".
                ui.window().request_redraw();
            }
        });
    }
}

/// Runs the stats query as a collection job. The loader shows if the query
/// still runs after `DECK_LOADING_DELAY`; the loader and the result each get
/// a full present, as a deck entry does.
fn open_stats(
    ui: &slint::Weak<MainWindow>,
    session: &Rc<LearnSession>,
    hooks: &FrontendHooks,
    deck_id: i64,
    deck_name: String,
) {
    tracing::info!("[stats] deck {deck_id}: query started");
    let finish_ui = ui.clone();
    let finish_session = session.clone();
    let finish_hooks = hooks.clone();
    let Some(job_id) = spawn_collection_job(
        ui,
        session,
        move |collection| match collection {
            Some(collection) => stats::query(collection, deck_id),
            None => Err("collection is unavailable".into()),
        },
        move |result| {
            let result =
                result.unwrap_or_else(|| Err("the stats query stopped unexpectedly".into()));
            let Some(ui) = finish_ui.upgrade() else {
                return;
            };
            match result {
                Ok(data) => {
                    let today = data.timing.today;
                    ui.set_stats(stats_view(&data, today.year(), today.month()));
                    *finish_session.stats.borrow_mut() = Some(StatsState {
                        data,
                        year: today.year(),
                        month: today.month(),
                    });
                }
                Err(err) => {
                    tracing::error!("[stats] deck {deck_id}: {err}");
                    *finish_session.stats.borrow_mut() = None;
                    ui.set_stats(StatsView {
                        deck_name: format!("Could not load stats: {err}").into(),
                        ..Default::default()
                    });
                }
            }
            ui.set_stats_tab(0);
            ui.set_loading(false);
            ui.set_stats_open(true);
            finish_hooks.request_full_present(&ui);
        },
    ) else {
        return;
    };

    let ui = ui.clone();
    let session = session.clone();
    let hooks = hooks.clone();
    slint::Timer::single_shot(DECK_LOADING_DELAY, move || {
        if session.busy.get()
            && session.job_seq.get() == job_id
            && let Some(ui) = ui.upgrade()
        {
            ui.set_stats(StatsView {
                deck_name: deck_name.as_str().into(),
                ..Default::default()
            });
            ui.set_loading(true);
            ui.set_stats_open(true);
            hooks.request_full_present(&ui);
        }
    });
}

/// The month `delta` months from `current`, kept between `first` and `last`.
/// Months are `(year, month)` with the month from 1.
fn shift_month(current: (i32, u32), delta: i32, first: (i32, u32), last: (i32, u32)) -> (i32, u32) {
    let index = |(year, month): (i32, u32)| year * 12 + month as i32 - 1;
    let shifted = (index(current) + delta).clamp(index(first), index(last).max(index(first)));
    (shifted.div_euclid(12), shifted.rem_euclid(12) as u32 + 1)
}

fn int_model(values: impl IntoIterator<Item = u32>) -> ModelRc<i32> {
    let values: Vec<i32> = values
        .into_iter()
        .map(|value| i32::try_from(value).unwrap_or(i32::MAX))
        .collect();
    ModelRc::from(Rc::new(VecModel::from(values)))
}

/// `45s`, `14m`, or `1h 05m`.
fn format_duration(millis: u32) -> String {
    let secs = millis / 1000;
    let (hours, minutes) = (secs / 3600, secs / 60 % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{secs}s")
    }
}

/// `2 342`: groups of three digits.
fn group_digits(value: u32) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(' ');
        }
        grouped.push(digit);
    }
    grouped
}

/// The share of `count` in `total`: `18%`, or `2.6%` below 10 %.
fn format_percent(count: u32, total: u32) -> String {
    if total == 0 {
        return "-".into();
    }
    let percent = f64::from(count) * 100.0 / f64::from(total);
    if count == 0 || percent >= 10.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// `38 days`, `4.5 months`, or `1.2 years`.
fn format_interval(days: f32) -> String {
    if days >= 365.0 {
        format!("{:.1} years", days / 365.0)
    } else if days >= 60.0 {
        format!("{:.1} months", days / 30.0)
    } else if (days - 1.0).abs() < 0.05 {
        "1 day".into()
    } else if days.fract().abs() < 0.05 {
        format!("{days:.0} days")
    } else {
        format!("{days:.1} days")
    }
}

/// The view model of the stats screen with the calendar on `year`/`month`.
pub(crate) fn stats_view(data: &StatsData, year: i32, month: u32) -> StatsView {
    let timing = data.timing;
    let grid = month_levels(
        &data.reviews_by_day,
        year,
        month,
        timing.first_weekday,
        timing.today,
    );
    // The Reviews chart shows the calendar month: day 1 to its last day.
    let reviews = data.month_reviews(year, month);
    let month_name = MONTHS[(month as usize - 1) % 12];
    let short_month = &month_name[..3];
    let reviews_label = |day: usize| format!("{short_month} {day}").into();
    let intervals = match (data.interval_mean, data.interval_max) {
        (Some(mean), Some(longest)) => format!(
            "Average {} \u{b7} Longest {}",
            format_interval(mean),
            format_interval(longest as f32)
        ),
        _ => "No review cards".to_string(),
    };
    let weekdays: Vec<slint::SharedString> = (0..7)
        .map(|day| WEEKDAYS[(timing.first_weekday as usize + day) % 7].into())
        .collect();
    let total: u32 = data.counts.iter().sum();
    let percents: Vec<slint::SharedString> = data
        .counts
        .iter()
        .map(|&count| format_percent(count, total).into())
        .collect();
    let count_texts: Vec<slint::SharedString> = data
        .counts
        .iter()
        .map(|&count| group_digits(count).into())
        .collect();
    let (studied, days) = data.studied_days(year, month);
    let today_day = if (year, month) == (timing.today.year(), timing.today.month()) {
        timing.today.day() as i32
    } else {
        0
    };

    StatsView {
        deck_name: data.deck_name.as_str().into(),
        today_cards: group_digits(data.today_cards).into(),
        today_time: format_duration(data.today_millis).into(),
        today_again: group_digits(data.today_again).into(),
        today_correct: data
            .today_correct_percent
            .map_or("-".to_string(), |percent| format!("{percent}%"))
            .into(),
        counts: int_model(data.counts),
        count_texts: ModelRc::from(Rc::new(VecModel::from(count_texts))),
        count_percents: ModelRc::from(Rc::new(VecModel::from(percents))),
        counts_total: i32::try_from(total).unwrap_or(i32::MAX),
        counts_summary: format!("{} cards", group_digits(total)).into(),
        future_due: int_model(data.future_due),
        future_due_max: data.future_due.iter().copied().max().unwrap_or(0) as i32,
        future_summary: format!("Tomorrow {}", group_digits(data.future_due[1])).into(),
        reviews_max: reviews.iter().copied().max().unwrap_or(0) as i32,
        reviews_first: reviews_label(1),
        reviews_middle: reviews_label(reviews.len().div_ceil(2)),
        reviews_last: reviews_label(reviews.len()),
        reviews: int_model(reviews),
        reviews_summary: data
            .month_average(year, month)
            .map_or(String::new(), |average| {
                format!("Average {} per day", group_digits(average))
            })
            .into(),
        intervals: intervals.into(),
        month_title: format!("{month_name} {year}").into(),
        month_lead: grid.lead as i32,
        month_levels: int_model(grid.levels.iter().map(|&level| u32::from(level))),
        month_today: today_day,
        month_summary: if days == 0 {
            String::new()
        } else {
            format!("Studied {studied} of {days} days")
        }
        .into(),
        month_has_prev: (year, month) > data.first_month(),
        month_has_next: (year, month) < (timing.today.year(), timing.today.month()),
        weekdays: ModelRc::from(Rc::new(VecModel::from(weekdays))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_month_stays_in_range() {
        let first = (2025, 11);
        let last = (2026, 2);
        assert_eq!(shift_month((2026, 1), -1, first, last), (2025, 12));
        assert_eq!(shift_month((2025, 12), 1, first, last), (2026, 1));
        assert_eq!(shift_month((2025, 11), -1, first, last), (2025, 11));
        assert_eq!(shift_month((2026, 2), 1, first, last), (2026, 2));
        // A first month after the last one (a clock change) does not panic.
        assert_eq!(shift_month((2026, 2), 1, (2026, 5), last), (2026, 5));
    }

    #[test]
    fn reviews_chart_follows_the_month() {
        use slint::Model;

        let timing = stats::Timing {
            today: chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
            first_weekday: 1,
        };
        let mut data = stats::build("Deck".into(), &Default::default(), 0, timing);
        // 2026-11-11, 2026-11-10, and 2026-09-27.
        data.reviews_by_day.extend([(0, 40), (-1, 50), (-45, 15)]);
        let values = |model: &ModelRc<i32>| model.iter().collect::<Vec<_>>();

        let november = stats_view(&data, 2026, 11);
        assert_eq!(november.reviews.row_count(), 30);
        assert_eq!(values(&november.reviews)[9..11], [50, 40]);
        assert_eq!(november.reviews_summary.as_str(), "Average 8 per day");
        let labels = [
            november.reviews_first.as_str(),
            november.reviews_middle.as_str(),
            november.reviews_last.as_str(),
        ];
        assert_eq!(labels, ["Nov 1", "Nov 15", "Nov 30"]);

        let september = stats_view(&data, 2026, 9);
        assert_eq!(values(&september.reviews)[26], 15);
        assert_eq!(september.reviews_summary.as_str(), "Average 1 per day");

        let february = stats_view(&data, 2026, 2);
        assert_eq!(february.reviews.row_count(), 28);
        assert_eq!(february.reviews_middle.as_str(), "Feb 14");
        assert_eq!(february.reviews_last.as_str(), "Feb 28");
    }

    #[test]
    fn numbers() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(2342), "2 342");
        assert_eq!(group_digits(1_234_567), "1 234 567");
        assert_eq!(format_percent(420, 2342), "18%");
        assert_eq!(format_percent(60, 2342), "2.6%");
        assert_eq!(format_percent(0, 2342), "0%");
        assert_eq!(format_percent(0, 0), "-");
        assert_eq!(format_interval(38.0), "38 days");
        assert_eq!(format_interval(1.0), "1 day");
        assert_eq!(format_interval(2.5), "2.5 days");
        assert_eq!(format_interval(135.0), "4.5 months");
        assert_eq!(format_interval(438.0), "1.2 years");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45_000), "45s");
        assert_eq!(format_duration(605_000), "10m");
        assert_eq!(format_duration(3_923_000), "1h 05m");
    }
}
