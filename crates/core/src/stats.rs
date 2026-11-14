//! Deck statistics for the stats screen.
//!
//! `query` runs on the worker thread: one graphs query per open, for the
//! whole history. `build` and `month_levels` are pure. A month change only
//! slices the loaded data (`frontend/stats.rs`).

use std::collections::BTreeMap;

use anki::collection::Collection;
use anki::decks::DeckId;
use anki::search::{SearchBuilder, SearchNode, SortMode};
use anki::services::StatsService;
use anki_proto::stats::{GraphsRequest, GraphsResponse};
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeDelta};

/// Days of the future-due chart.
pub const CHART_DAYS: usize = 30;

/// Day 1 of a month (from 1). `None` for an invalid month.
fn month_start(year: i32, month: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, 1)
}

/// When the query ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The date of the current Anki day (it changes at the rollover hour).
    pub today: NaiveDate,
    /// First day of the week for the calendar: 0 is Sunday.
    pub first_weekday: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsData {
    pub deck_name: String,
    pub timing: Timing,
    /// Today: answers, their time, cards rated Again, and the share of
    /// answers that were not Again (`None` without answers).
    pub today_cards: u32,
    pub today_millis: u32,
    pub today_again: u32,
    pub today_correct_percent: Option<u32>,
    /// New, learning (with relearning), young, mature, inactive (suspended
    /// and buried).
    pub counts: [u32; 5],
    /// Cards due on each of the next 30 days. Index 0 is today, with the
    /// backlog.
    pub future_due: [u32; CHART_DAYS],
    /// Reviews by day for the whole history. The key is the day offset: 0 is
    /// today, -1 is yesterday.
    pub reviews_by_day: BTreeMap<i32, u32>,
    /// Mean and longest interval of the review cards, in days.
    pub interval_mean: Option<f32>,
    pub interval_max: Option<u32>,
}

impl StatsData {
    /// Reviews on each day of a month, day 1 first. A day after today has 0.
    /// Empty for an invalid month.
    pub fn month_reviews(&self, year: i32, month: u32) -> Vec<u32> {
        month_start(year, month).map_or_else(Vec::new, |first| {
            day_reviews(&self.reviews_by_day, first, self.timing.today)
                .into_iter()
                .map(|reviews| reviews.unwrap_or(0))
                .collect()
        })
    }

    /// Mean reviews per day of a month, over its days up to today (see
    /// `studied_days`). `None` for a future month.
    pub fn month_average(&self, year: i32, month: u32) -> Option<u32> {
        let (_, days) = self.studied_days(year, month);
        let total: u32 = self.month_reviews(year, month).iter().sum();
        (days > 0).then(|| (total + days / 2) / days)
    }

    /// `(days with reviews, days)` of a month. The days of the current month
    /// end today; a future month has none.
    pub fn studied_days(&self, year: i32, month: u32) -> (u32, u32) {
        let Some(start) = month_start(year, month) else {
            return (0, 0);
        };
        // Day offsets from today, as in `reviews_by_day`.
        let first = (start - self.timing.today).num_days();
        let last = (first + i64::from(start.num_days_in_month()) - 1).min(0);
        if last < first {
            return (0, 0);
        }
        let studied = self
            .reviews_by_day
            .range(first as i32..=last as i32)
            .count();
        (studied as u32, (last - first + 1) as u32)
    }

    /// The month of the first review, or of today without reviews.
    pub fn first_month(&self) -> (i32, u32) {
        let offset = self
            .reviews_by_day
            .keys()
            .next()
            .copied()
            .unwrap_or(0)
            .min(0);
        let date = self
            .timing
            .today
            .checked_add_signed(TimeDelta::days(i64::from(offset)))
            .unwrap_or(self.timing.today);
        (date.year(), date.month())
    }
}

/// Builds the stats from a graphs response for the whole history
/// (`days: 0`). `again_today` is the number of cards rated Again today.
pub fn build(
    deck_name: String,
    graphs: &GraphsResponse,
    again_today: u32,
    timing: Timing,
) -> StatsData {
    let today = graphs.today.unwrap_or_default();
    let today_correct_percent = (today.answer_count > 0).then(|| {
        (u64::from(today.correct_count) * 100 / u64::from(today.answer_count)).min(100) as u32
    });

    let counts = graphs
        .card_counts
        .as_ref()
        .and_then(|counts| counts.excluding_inactive)
        .map(|counts| {
            [
                counts.new_cards,
                counts.learn + counts.relearn,
                counts.young,
                counts.mature,
                counts.suspended + counts.buried,
            ]
        })
        .unwrap_or_default();

    let mut future_due = [0; CHART_DAYS];
    if let Some(due) = &graphs.future_due {
        for (&day, &count) in &due.future_due {
            // A negative day is the backlog: it is due today.
            let index = day.max(0) as usize;
            if index < CHART_DAYS {
                future_due[index] += count;
            }
        }
    }

    let mut reviews_by_day = BTreeMap::new();
    if let Some(reviews) = &graphs.reviews {
        for (&day, count) in &reviews.count {
            let total = count.learn + count.relearn + count.young + count.mature + count.filtered;
            if total > 0 {
                reviews_by_day.insert(day, total);
            }
        }
    }

    let (interval_mean, interval_max) = graphs
        .intervals
        .as_ref()
        .map(|intervals| interval_summary(&intervals.intervals))
        .unwrap_or((None, None));

    StatsData {
        deck_name,
        timing,
        today_cards: today.answer_count,
        today_millis: today.answer_millis,
        today_again: again_today,
        today_correct_percent,
        counts,
        future_due,
        reviews_by_day,
        interval_mean,
        interval_max,
    }
}

/// Mean and longest interval of an interval histogram (interval in days to
/// card count).
fn interval_summary<'a>(
    histogram: impl IntoIterator<Item = (&'a u32, &'a u32)>,
) -> (Option<f32>, Option<u32>) {
    let mut cards = 0u64;
    let mut sum = 0u64;
    let mut longest = None;
    for (&interval, &count) in histogram {
        if count > 0 {
            cards += u64::from(count);
            sum += u64::from(interval) * u64::from(count);
            longest = longest.max(Some(interval));
        }
    }
    if cards == 0 {
        return (None, None);
    }
    (Some(sum as f32 / cards as f32), longest)
}

/// One calendar month as cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthGrid {
    /// Empty cells before day 1, from the first day of the week.
    pub lead: u32,
    /// One level for each day of the month: 0 no reviews, 1 outline (1 to 49
    /// reviews), 2 solid (50 or more), 3 a future day.
    pub levels: Vec<u8>,
}

/// The review levels of the days of one month. `reviews_by_day` is keyed by
/// the day offset from `today` (0 is today). `first_weekday`: 0 is Sunday.
pub fn month_levels(
    reviews_by_day: &BTreeMap<i32, u32>,
    year: i32,
    month: u32,
    first_weekday: u32,
    today: NaiveDate,
) -> MonthGrid {
    let Some(first) = month_start(year, month) else {
        return MonthGrid {
            lead: 0,
            levels: Vec::new(),
        };
    };
    let lead = (first.weekday().num_days_from_sunday() + 7 - first_weekday % 7) % 7;
    let levels = day_reviews(reviews_by_day, first, today)
        .into_iter()
        .map(|reviews| match reviews {
            None => 3,
            Some(0) => 0,
            Some(1..=49) => 1,
            Some(_) => 2,
        })
        .collect();
    MonthGrid { lead, levels }
}

/// The reviews on each day of the month that starts on `first`, day 1 first.
/// `None` is a day after `today`. The calendar and the Reviews chart share it.
fn day_reviews(
    reviews_by_day: &BTreeMap<i32, u32>,
    first: NaiveDate,
    today: NaiveDate,
) -> Vec<Option<u32>> {
    let first_offset = (first - today).num_days();
    (0..i64::from(first.num_days_in_month()))
        .map(|day| {
            let offset = first_offset + day;
            if offset > 0 {
                return None;
            }
            let reviews = i32::try_from(offset)
                .ok()
                .and_then(|offset| reviews_by_day.get(&offset))
                .copied()
                .unwrap_or(0);
            Some(reviews)
        })
        .collect()
}

/// Loads the stats of a deck and its subdecks. It reads every card and
/// revlog entry of the deck: run it on the worker thread.
pub fn query(col: &mut Collection, deck_id: i64) -> Result<StatsData, String> {
    let deck = col
        .get_deck(DeckId(deck_id))
        .map_err(|err| err.to_string())?
        .ok_or("the deck does not exist")?;
    let deck_name = deck.human_name();
    // `deck:"Parent::Child"` includes the subdecks.
    let search = SearchBuilder::from(SearchNode::from_deck_name(&deck_name)).write();

    let graphs = col
        .graphs(GraphsRequest {
            search: search.clone(),
            days: 0,
        })
        .map_err(|err| err.to_string())?;
    let preferences = col.get_graph_preferences().map_err(|err| err.to_string())?;
    let timing = col.timing_today().map_err(|err| err.to_string())?;
    // Cards rated Again today, not button presses.
    let again_today = col
        .search_cards(format!("{search} rated:1:1").as_str(), SortMode::NoOrder)
        .map_err(|err| err.to_string())?
        .len() as u32;

    // The Anki day ends at `next_day_at`; it started a day before.
    let today = DateTime::from_timestamp(timing.next_day_at.0 - 86_400, 0)
        .ok_or("the day rollover time is out of range")?
        .with_timezone(&Local)
        .date_naive();
    let timing = Timing {
        today,
        first_weekday: preferences.calendar_first_day_of_week.clamp(0, 6) as u32,
    };
    Ok(build(deck_name, &graphs, again_today, timing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anki_proto::stats::graphs_response::card_counts::Counts;
    use anki_proto::stats::graphs_response::review_counts_and_times::Reviews;
    use anki_proto::stats::graphs_response::{
        CardCounts, FutureDue, Intervals, ReviewCountsAndTimes, Today,
    };

    const TODAY: NaiveDate = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();

    fn timing() -> Timing {
        Timing {
            today: TODAY,
            first_weekday: 1,
        }
    }

    fn reviews(young: u32) -> Reviews {
        Reviews {
            young,
            ..Default::default()
        }
    }

    fn synthetic() -> GraphsResponse {
        GraphsResponse {
            today: Some(Today {
                answer_count: 40,
                answer_millis: 605_000,
                correct_count: 33,
                ..Default::default()
            }),
            card_counts: Some(CardCounts {
                including_inactive: None,
                excluding_inactive: Some(Counts {
                    new_cards: 100,
                    learn: 5,
                    relearn: 2,
                    young: 30,
                    mature: 60,
                    suspended: 4,
                    buried: 1,
                }),
            }),
            future_due: Some(FutureDue {
                future_due: [(-3, 2), (0, 5), (1, 7), (29, 9), (30, 11)].into(),
                have_backlog: true,
                daily_load: 0,
            }),
            reviews: Some(ReviewCountsAndTimes {
                count: [
                    (0, reviews(40)),
                    (-1, reviews(50)),
                    (-29, reviews(3)),
                    (-30, reviews(8)),
                    (
                        -45,
                        Reviews {
                            learn: 1,
                            relearn: 2,
                            young: 3,
                            mature: 4,
                            filtered: 5,
                        },
                    ),
                    (-50, Reviews::default()),
                ]
                .into(),
                time: Default::default(),
            }),
            intervals: Some(Intervals {
                intervals: [(1, 2), (10, 1), (100, 1)].into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn build_maps_the_response() {
        let data = build("Deck".into(), &synthetic(), 6, timing());

        assert_eq!(data.today_cards, 40);
        assert_eq!(data.today_millis, 605_000);
        assert_eq!(data.today_again, 6);
        assert_eq!(data.today_correct_percent, Some(82));
        assert_eq!(data.counts, [100, 7, 30, 60, 5]);
        // The backlog is due today; day 30 is outside the chart.
        assert_eq!(data.future_due[0], 7);
        assert_eq!(data.future_due[1], 7);
        assert_eq!(data.future_due[29], 9);
        assert_eq!(data.future_due.iter().sum::<u32>(), 23);
        // All review kinds count; a day without reviews has no entry.
        assert_eq!(data.reviews_by_day.get(&-45), Some(&15));
        assert_eq!(data.reviews_by_day.get(&-50), None);
        // November: 50 yesterday, 40 today, nothing after today.
        let november = data.month_reviews(2026, 11);
        assert_eq!(november.len(), 30);
        assert_eq!(november[9], 50);
        assert_eq!(november[10], 40);
        assert_eq!(november.iter().sum::<u32>(), 90);
        // October: -30 is the 12th, -29 the 13th.
        let october = data.month_reviews(2026, 10);
        assert_eq!(october.len(), 31);
        assert_eq!((october[11], october[12]), (8, 3));
        assert_eq!(october.iter().sum::<u32>(), 11);
        assert_eq!(data.month_reviews(2026, 12), vec![0; 31]);
        assert!(data.month_reviews(2026, 13).is_empty());
        // Intervals 1, 1, 10, 100.
        assert_eq!(data.interval_mean, Some(28.0));
        assert_eq!(data.interval_max, Some(100));
        // November: 90 reviews in the 11 days up to today. September: 15 in 30.
        assert_eq!(data.month_average(2026, 11), Some(8));
        assert_eq!(data.month_average(2026, 9), Some(1));
        assert_eq!(data.month_average(2026, 12), None);
        // November up to the 11th: today and yesterday. October: two days.
        assert_eq!(data.studied_days(2026, 11), (2, 11));
        assert_eq!(data.studied_days(2026, 10), (2, 31));
        assert_eq!(data.studied_days(2026, 12), (0, 0));
        // 45 days before 2026-11-11 is 2026-09-27.
        assert_eq!(data.first_month(), (2026, 9));
    }

    #[test]
    fn build_of_an_empty_response() {
        let data = build("Deck".into(), &GraphsResponse::default(), 0, timing());

        assert_eq!(data.today_correct_percent, None);
        assert_eq!(data.counts, [0; 5]);
        assert_eq!(data.future_due, [0; CHART_DAYS]);
        assert!(data.reviews_by_day.is_empty());
        assert_eq!((data.interval_mean, data.interval_max), (None, None));
        assert_eq!(data.month_average(2026, 11), Some(0));
        assert_eq!(data.studied_days(2026, 11), (0, 11));
        assert_eq!(data.first_month(), (2026, 11));
    }

    #[test]
    fn month_levels_of_the_current_month() {
        let data = build("Deck".into(), &synthetic(), 0, timing());

        let grid = month_levels(&data.reviews_by_day, 2026, 11, 1, TODAY);

        // 2026-11-01 is a Sunday: six empty cells in a week that starts on
        // Monday, none in one that starts on Sunday.
        assert_eq!(grid.lead, 6);
        assert_eq!(
            month_levels(&data.reviews_by_day, 2026, 11, 0, TODAY).lead,
            0
        );
        assert_eq!(grid.levels.len(), 30);
        assert_eq!(grid.levels[10], 1, "today, 40 reviews");
        assert_eq!(grid.levels[9], 2, "yesterday, 50 reviews");
        assert_eq!(grid.levels[8], 0, "no reviews");
        assert!(grid.levels[11..].iter().all(|&level| level == 3), "future");
    }

    #[test]
    fn month_levels_of_other_months() {
        let data = build("Deck".into(), &synthetic(), 0, timing());

        // -29 is 2026-10-13, -30 is 2026-10-12.
        let october = month_levels(&data.reviews_by_day, 2026, 10, 1, TODAY);
        assert_eq!(october.lead, 3, "2026-10-01 is a Thursday");
        assert_eq!(october.levels.len(), 31);
        assert_eq!(october.levels[12], 1);
        assert_eq!(october.levels[11], 1);
        assert_eq!(
            october.levels.iter().filter(|&&level| level != 0).count(),
            2
        );

        let december = month_levels(&data.reviews_by_day, 2026, 12, 1, TODAY);
        assert!(december.levels.iter().all(|&level| level == 3));

        assert_eq!(
            month_levels(&data.reviews_by_day, 2024, 2, 1, TODAY)
                .levels
                .len(),
            29
        );
        // An invalid month is empty, not a panic.
        assert!(
            month_levels(&data.reviews_by_day, 2026, 13, 1, TODAY)
                .levels
                .is_empty()
        );
        assert_eq!(data.studied_days(2026, 13), (0, 0));
    }
}
