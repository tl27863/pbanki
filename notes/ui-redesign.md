# UI redesign

Status: implemented (2026-11-11), with one open item. This note records the design, what was built, and what is left.

Open items:

- **Device checks.** See "Device checks" below. The layouts were checked in eink-sim at 1264×1680 only; the simulator has a fixed size.

Figma file: <https://www.figma.com/design/9DsC6ViN0wC2bNdbp2xRL0/PbAnki>

## Goals

- Plain black and white. An accent color appears only where it carries data (deck counts, rating intervals, errors).
- One layout for all PocketBook panel sizes.
- No scrolling. Lists paginate.
- Activity is shown with small icons, not status text.
- All icons are SVG.

## Figma file layout

| Section | Contents |
|---|---|
| Main | The screens at 1264×1680: Home, Study (Question, Answer, Answer - Paged), Settings (Sync, Sync - Full sync, Device), Stats (01 Overview, 02 History), Loading 1–4 |
| Addon › Status | Activity icon frames and the animation rules |
| Addon › Icons | 12 action icons, each with an SVG export setting |
| Addon › Components | DeckRow, RatingButton, Tab, Toggle, Field, Button, Pager |
| Resize check | Home and Study at two other panel sizes |

Units: the frames are in physical px for a 300 dpi panel. Slint uses logical px × `dpi/100`, so 3 Figma px = 1 Slint px. Divide every Figma value by 3.

## Tokens

Figma variables, collection `Theme`. The Figma plan allows one mode only, so the gray-panel value is in each variable's description.

| Token | Color panel | Gray panel |
|---|---|---|
| `ink`, `line` | #000000 | #000000 |
| `paper` | #FFFFFF | #FFFFFF |
| `muted` | #808080 | #808080 |
| `new` | #0000FF | #000000 |
| `learn` | #FF0000 | #000000 |
| `review` | #008000 | #000000 |
| `error` | #CC0000 | #000000 |
| `warning` | #B36B00 | #404040 |
| `tone-30`, `tone-60` (stacked bar) | #B3B3B3, #666666 | the same |

These are the values already in `crates/core/ui/theme.slint`. On gray panels the accents are black, so the column header (Home) and the fixed order New, Learn, Due (Study) carry the meaning.

Sizes in Slint px:

| Item | Value |
|---|---|
| Spacing | 4, 8, 12, 20 |
| Screen padding | 20 |
| Top bar, tab header, touch target | 44 |
| Icon | 24 |
| Deck row | 48 |
| Answer bar | 84 |
| Hairline | 1 |
| Font sizes | 30 (title), 20 (body), 15 (secondary), 12 (caption) |
| Font weights | 400, 700 |

The four font sizes are the ones the UI already embeds (30, 20, 15, 12). The 14 px size in `PlainButton` and `RatingButton` goes away.

## Screens

### Home

- Top bar: "pbAnki" on the left. Sync button and settings button on the right.
- The sync button is also the sync indicator. It animates in place during a sync. There is no second sync icon.
- Deck list: a header line (New, Learn, Due), then rows. A row has the indent and chevron for subdecks, the name, three counts, and a stats button. A zero count is `muted`.
- A pager at the bottom replaces scrolling. Rows per page = floor(list height ÷ 48).

### Study

- Top bar: back on the left. On the right, the activity slot and the remaining counts in the order New, Learn, Due.
- Card content fills the middle.
- Answer bar, full width, edge to edge, with a hairline on top:
  - Question side: one "Show answer" button.
  - Answer side: four equal buttons. Each shows the interval above the label. Only the interval has the accent.
- Hardware key hints: a small key glyph sits on the control that the key triggers. Defaults: Next = Show answer, then Good. Prev = Again.
- If a card has more than one page, the keys turn the page first: Prev until the first page, Next until the last page. While a key turns the page, its glyph sits beside the page indicator ("1 / 2") at the bottom right of the card. At its end of the side, the glyph moves to the control that the key triggers. So on the last page Next is on Show answer or Good, and on the first page Prev is on its control (Again on the answer side). The hidden pager glyph keeps its place, so the indicator does not move.
- The study controls have no pressed state (existing rule, see `eink-refresh.md`).

### Settings

- Tab header: back, then the tabs Sync and Device. The current tab is bold with a 3 px underline.
- Sync: Server URL, Email, Password, the toggles Auto sync, Sync on exit, Sync media, a status line, Save and Sync now.
- Sync, full-sync state: a notice, a confirm toggle, Upload and Download.
- Device:
  - Full refresh: a stepper with Never, Every card, Every 5 cards, Every 10 cards.
  - Hardware keys: Prev and Next only. Each has an action for the question side and for the answer side.

### Stats

- Opened from the stats button on a deck row. The stats are for that deck.
- Tab header: back, then the tabs Overview and History. The deck name is in a bar at the bottom.
- Overview: Today (cards, time, again, correct), Card counts (stacked bar and table), Future due (bar chart, 30 days).
- History: Calendar (month grid with previous and next month buttons), Reviews (bar chart of the calendar month, one bar for each day; it moves with the calendar), Intervals (one summary line).
- Calendar cells have three states: no box (no reviews), outline (1–49), solid (50 or more). Future days are `muted`.
- Charts are black bars with a hairline axis and direct labels.

### Loading

- Full screen. Four small squares in a row above one line of text ("Loading deck").
- One more square fills at each step. Only the squares change.

## Activity icons and animation

| Icon | Frames | Where |
|---|---|---|
| sync | 2 (the sync icon at 0° and 90°) | The Home sync button, in place |
| network | 4 (dot, then one more arc each) | The activity slot |
| loading | 3 (one large dot moves along) | The activity slot on Study |
| done, error | 1 each | End states |

Rules:

- Stepped frames only. No fade, no tween, no easing.
- Pure black and white.
- 500 ms or more for each frame. A panel update takes about 440 ms on the PB710.
- The loop stops on an end state.
- All change stays inside the 24×24 icon box. The damage is then far below the 100k px limit for the fast update.
- On Study, the loading icon appears only after the existing 300 ms delay (`RATING_LOADING_DELAY`).

## Resizing

Bars and rows have fixed heights. The deck name, the card area, and the charts fill the remaining space. Only the number of rows on a page changes.

| Panel | Logical size | Rows per page |
|---|---|---|
| 1264×1680 at 300 dpi | 421×560 | 9 |
| 1404×1872 at 227 dpi | 618×825 | 14 |
| 758×1024 at 212 dpi | 357×482 | 7 |

At 357 px wide, a deck name has little room. The name truncates with an ellipsis.

The Figma frames use the full panel. With the system panel on, the window is shorter (see `inkview-backend.md`), and the rows per page must come from the real list height.

## Implementation plan

Each step can ship alone. Read the related note first, as `AGENTS.md` requires.

### 1. Tokens and icons

- `theme.slint`: add `ink`, `paper`, `line`. Keep the existing count, rating, and status tokens.
- `constants.slint`: add the sizes from the table above. Remove the 14 px font size.
- Export the icons from Figma as SVG at 24×24 into `crates/core/ui/icons/`. The Figma components are 72×72, so set the export scale to 1/3 or edit the `viewBox`.
- The exported SVGs use `stroke="black"`. The current icons use `#323544`. Update the sentence about icon color in `inkview-backend.md`.
- Each animation frame is a separate SVG file. Runtime SVG decoding is off, so all frames are embedded at build time.

### 2. Shared components

- New files in `ui/components/`: `top_bar.slint`, `icon_button.slint`, `tab_header.slint`, `pager.slint`, `toggle.slint`, `stepper.slint`, `activity_icon.slint`.
- `Toggle` replaces the std-widgets `CheckBox`. This also removes the CheckBox font-size problem in `fonts.md`.
- `activity_icon.slint`: a `Timer` with a 500 ms interval changes the frame index. The timer runs only while the icon is visible.
- Update `plain_button.slint` and `rating_button.slint` to the edge-to-edge style. Keep them without states. The lint rule `pbanki-study-controls-no-states` must still pass.

### 3. Home

- Replace the `ListView` in `deck_select.slint` with a fixed page of rows.
- The collapsed state must move from the row to the model, because the page content depends on it. Today the row toggles `collapsed` locally (`deck.slint`).
- Compute the visible rows and the page count in Rust (`frontend/models.rs`) or in a Slint pure function. Input: the list height and the row height.
- Add `deck-page` and `deck-page-count` properties and a `stats_clicked(DeckNode)` callback on `MainWindow`.
- Map the hardware Prev and Next keys to the pager. The deck list has no key handling today.
- Remove the "N cards due" line and the status text from the top bar. The sync button shows the activity; errors need a place (see Open questions).

### 4. Study

- `learn.slint`: new top bar, the page indicator at the bottom right of the card, the new answer bar.
- The card area height does not change (`height - 2*padding - 44 - 84`), but the answer bar no longer has side padding. Check `card-area-width` in `main.slint`.
- Key handling changes. Today the hardware Next key is RightArrow and rates Hard on the answer side (`learn.slint`). The design makes Next rate Good by default, and the action comes from the config.
- Show the key glyphs from the configured actions, not from fixed positions.
- Replace the full-screen "Loading next card..." text with the loading icon in the top bar. Keep the full-screen loader for deck entry. See `card-loading.md`.
- Risk: each animation frame is one more panel update during the card change. Measure the time to the next card on the PB710 with and without the icon. If it costs more than a few tens of ms, show one static frame instead.

### 5. Settings

- Split `settings.slint` into the tab header and two content components.
- New config fields in `config.rs`:
  - `full_refresh_every`: 0 (never), 1, 5, 10.
  - `key_prev_question`, `key_next_question`, `key_prev_answer`, `key_next_answer`: an action enum (nothing, show answer, again, hard, good, easy).
- The Device tab needs a property struct like `SyncSettings` and a save path in `frontend/settings.rs`.
- Full refresh: there is no periodic full refresh today (`eink-refresh.md`). The setting needs a counter in the frontend and a hook that requests a flashing full update after the card present. Read `eink-refresh.md` before this change; the full-present latch and the 300 ms verify must not fight the new update.

### 6. Stats

- New screen `screens/stats.slint` and a `stats_open` state on `MainWindow`.
- Data: Anki's `rslib` has a graphs service for the stats screen (today, future due, calendar, reviews, card counts, intervals). Confirm the exact API in the pinned `anki` version before the design of the Rust side.
- Run the query on the worker thread, as the card jobs do. Show the full-screen loader if it takes longer than about 200 ms.
- Send plain view models to Slint: four numbers for Today, five counts, arrays of bar heights, and a month of day levels.
- Charts: draw the bars as `Rectangle` elements in a `for` loop. No chart library is necessary.
- Calendar navigation uses the `stats_month_changed(delta)` callback. There is one query per open, for the whole history; a month change slices the loaded data on the UI thread.
- Gray tones in the stacked bar (30 % and 60 % ink) are new colors. Add them to `Theme`; the lint rule forbids color literals in other files.

### 7. Loader

- Replace the loading text with the four-square loader. A `Timer` at 500 ms changes the step.
- Keep the existing delays (`DECK_LOADING_DELAY`, `RATING_LOADING_DELAY`) so that short loads show no loader.

### Tests and checks

- `cargo test -p pbanki-core` for the pagination and the key-action mapping.
- `make lint` after each step. Consider a rule that forbids `ListView` and `ScrollView` in `ui/`, because the no-scroll design is an invariant that clippy cannot check.
- Desktop with `--features eink-sim` at 1264×1680, 1404×1872, and 758×1024.
- On the PB710: binary size (font sizes), time to next card, ghosting from the animation frames.

## Icons

The SVGs in `crates/core/ui/icons/` are the Figma exports (72×72, black stroke). The Figma export names map to the file names like this:

| Figma export | File |
|---|---|
| `icon/back`, `settings`, `stats` | the same name |
| `icon/chevron-right`, `chevron-down` | `chevron_right.svg`, `chevron_down.svg` |
| `icon/page-prev`, `page-next` | `prev.svg`, `next.svg` |
| `icon/key-prev`, `key-next` | `key_prev.svg`, `key_next.svg` |
| `activity/frame=1`, `frame=2` | `sync_0.svg`, `sync_1.svg` |
| `activity/frame=1-1`, `frame=2-1`, `frame=3`, `frame=4` | `network_0.svg` to `network_3.svg` |
| `activity/frame=1-2`, `frame=2-2`, `frame=3-1` | `loading_0.svg` to `loading_2.svg` |
| `activity/done`, `error` | the same name |

`icon/check`, `icon/alert` and `icon/sync` are equal to `done`, `error` and `sync_0`, so they have no file. The network frames are embedded but no screen shows them yet.

## What was built

| Step | Where |
|---|---|
| Sync on a worker thread | `frontend/card_jobs.rs` (`spawn_collection_job`), `sync/mod.rs`, `frontend/sync.rs`; see `sync.md` |
| Tokens, icons | `ui/theme.slint`, `ui/constants.slint`, `ui/icons/` |
| Components | `ui/components/`: `icon_button`, `top_bar`, `tab_header`, `pager`, `toggle`, `stepper`, `activity_icon`, `key_glyph`, `loader`, `deck_row`; `plain_button` and `rating_button` restyled |
| Home | `ui/screens/deck_select.slint`, `frontend/decks.rs`, `frontend/models.rs` (`visible_decks`, `page_slice`); see `card-loading.md`, "Home deck list" |
| Study | `ui/screens/learn.slint`, `keys.rs`, `frontend/study.rs`; see `card-loading.md`, "Hardware keys" |
| Settings | `ui/screens/settings.slint`, `settings_sync.slint`, `settings_device.slint`, `frontend/settings.rs`, `config.rs` |
| Stats | `stats.rs`, `frontend/stats.rs`, `ui/screens/stats.slint` |
| Loader | `ui/components/loader.slint` (deck entry and stats) |

Differences from the design text above:

- The key actions of the Device tab are not dropdown lists: a tap selects the next action and wraps (`components/cycle_select.slint`). A popup list would be two more panel updates. The question side offers Nothing and Show answer; the answer side offers Nothing and the four ratings. Show answer does nothing on the answer side, and a rating does nothing on the question side.
- The Device tab saves each change at once. It has no Save button.
- The full-sync state stays until a full sync or another sync result. It has no way back to the form, as in the design.
- The labels of the Sync tab are the short English ones of the design ("Server URL", "Auto sync"), not Anki's translations: the translated toggle labels do not fit at 20 px.
- The caption under Full refresh is "A flashing refresh clears ghosting.", not the list of values.
- The stacked bar ends with the suspended and buried cards as an open segment (no dashed pattern). The "Suspended" row counts both.
- The calendar always has six rows, so the charts below do not move with the month. Today is bold.
- The Reviews chart is tied to the calendar: the month buttons change both. It has one bar for each day of the month, labeled with the first, middle, and last day ("Nov 1", "Nov 15", "Nov 30"); the Figma frame shows "30 days ago … Today". In the current month, the days after today have no bar. "Average … per day" is the mean over the days of that month up to today. Intervals show the mean and the longest interval.
- The page indicator and its key glyphs (16 px) are in the 20 px padding below the card, right-aligned, so they do not cover the page.
- After a sync without an error, the sync button shows the done icon until the next screen change; then it is the sync icon again (`sync.md`). A media warning shows the error icon, which stays.
- A disabled icon button shows no icon: at the ends of the history (the current month, the month of the first review) that month chevron is hidden. It keeps its place. The month title has the width of the longest title ("September 0000"), so the chevrons stay in place when the month changes and repeated taps hit the same chevron.
- The texts of the new UI (tab names, stats labels, "Loading deck") are English only. The study buttons and the sync labels still come from Anki's translations.

## Device checks

On the PB710, not done yet:

- The four no-extra-touch checks in `card-loading.md`.
- Wi-Fi prompt and radio power after a sync on the worker thread.
- Time to the next card with the animated loading icon, with a static frame, and with none. Record it in `performance.md`. If the animation costs more than a few tens of ms, use a static frame.
- Ghosting of the 2-frame sync loop and of the loader squares.
- The flashing refresh at 1, 5 and 10 cards.
- Prev and Next arrive as expected, and Back does not leave the app unexpectedly.
- The stats query time on a large deck.

On the desktop, not done yet: eink-sim at 1404×1872 and 758×1024 (rows per page 14 and 7, name elision at 357 px, the Sync tab at 357×482), and a sync, a full upload and a full download against the dev sync server from the UI. The sync functions have tests against anki's server; in eink-sim a sync to a closed port ended on the error icon.

## Open questions

- Closed: sync errors show as the error icon in the sync button; the text is on the Sync tab.
- Does a 2-frame icon loop at 500 ms cause visible ghosting on the PB710? Not measured.
- The Hard and Easy buttons have no hardware key by default. The user can map one in Settings > Device.
- The settings from the config file that have no UI (`system_panel`, `card_shadows`) are not in the Device tab.
- The gray theme was not checked in Figma, because the plan has no second variable mode. eink-sim uses the gray theme (`color-panel: false`), and the screens were checked there.
