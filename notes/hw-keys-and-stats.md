# Hardware keys and Anki stats

Status: research for the UI redesign (`ui-redesign.md`, steps 4, 5 and 6). The plans below are implemented (2026-11-11): `keys.rs`, `stats.rs`, `frontend/stats.rs`. The "today" sections describe the code before that.

## Hardware keys today

Only `crates/core/ui/screens/learn.slint:118-147` handles keys. It is one `FocusScope`, guarded by `valid_deck && !loading && !busy`. The deck list, settings and main window have no key handling.

| State | Slint key | Action |
|---|---|---|
| Question | any key | reveal |
| Answer | Left, Down, PageDown | `rate(0)` |
| Answer | Right, Return | `rate(1)` |
| Answer | Menu, Up, PageUp | `rate(2)` |
| Card with more than one page | Left, Down, PageDown / Right, Up, PageUp | previous / next page |

- A page turn comes first. Reveal and rate run only when the card is on page 0 (Prev side) or on the last page (Next side).
- The ratings are 0 Again, 1 Hard, 2 Good, 3 Easy (`study.rs::rate_card`). So before the redesign Next (RightArrow) rated Hard, and Menu, Up and PageUp rated Good. `ui-redesign.md` was right.
- Focus is set by a 1 ms `Timer`, not in `init` (lint `pbanki-no-focus-in-init`, see `card-loading.md`).
- Page turns by key or swipe only change `current-page`. `card_pages.rs` reacts through the `page-image` binding. A fast key press can show the old image until the new page arrives.
- Page turns, reveal and rating are partial updates. A key press never causes a flashing refresh (`eink-refresh.md`).

## InkView key path

The fork is `/Data/Project/inkview-rs`, branch `pbanki-device-panel` (git dependency in the root `Cargo.toml`).

- `inkview/src/event.rs:76-119`: the app gets `KeyDown`, `KeyRepeat` and `KeyUp`. There is no long-press event. `EVT_KEYPRESS_EXT`, `EVT_POINTERLONG` and `EVT_POINTERHOLD` are dropped.
- `Key` enum (`event.rs:15-51`): Prev (24), Next (25), Prev2 (28), Next2 (29), Plus, Minus, Menu, Home, Back, Ok, the d-pad, digits, and others. Keys not in the enum are dropped. `IV_KEY_TAB` is one of them.
- `inkview-slint/src/lib.rs:289-306` (`ink_key_to_slint`) is lossy:

| InkView key | Slint key |
|---|---|
| Up / Down | `UpArrow` / `DownArrow` |
| Left, Prev, Prev2 | `LeftArrow` |
| Right, Next, Next2 | `RightArrow` |
| Ok | `Return` |
| Back | `Backspace` |
| Menu | `Menu` |
| Home | `Home` |
| Plus / Minus | `PageUp` / `PageDown` |

- Power, Delete, Music, Zoom, Shift and the digits are dropped.
- Slint cannot tell Prev from the d-pad Left, or Next from Right.
- Back is `Backspace`, not `Escape`. `learn.slint` has no branch for Back or Home.
- Key repeat is delivered. To detect a long press, the app must time `KeyDown` to `KeyUp` itself.
- The bindings have no key remap setter. `GetKeyMapping*` only reads. `SetKeyboardRate` is bound but not used.
- The event handler returns `Some` for handled and `None` for not handled (`inkview/src/lib.rs:41-56`). It is not checked on the device whether a handled key stops the system action (for example Back leaving the app).
- eink-sim (`crates/desktop/src/eink_sim.rs:342-356`): PageUp is Prev, PageDown is Next, arrows are the d-pad, Enter and Space are Ok, Backspace and Escape are Back, M is Menu. Plain desktop sends winit keys to Slint directly.

## Hardware keys: plan

1. Put the key to action logic in Rust so it can be tested: `KeyAction { Nothing, ShowAnswer, Again, Hard, Good, Easy }`, and a pure `resolve(key, side, page_state, config)`. Slint only classifies the key and calls a non-blocking callback. Slint also gets the resolved actions as properties, to place the key glyphs.
2. Keep the page-turn precedence from today. If a page can turn, the key turns it and the glyphs sit by the page indicator.
3. Defaults: Question: Next = Show answer. Answer: Next = Good, Prev = Again. Hard and Easy have no key unless the user maps one.
4. Prev and Next must be told apart from the d-pad. First try the app-only way: treat Left, Right, PageUp and PageDown all as Prev and Next. The app uses no d-pad action. Change `inkview-slint` (distinct Slint keys for Prev and Next) only if this is not enough.
5. Config fields in `config.rs`: `key_prev_question`, `key_next_question`, `key_prev_answer`, `key_next_answer`, and `full_refresh_every`. Each new `general` field goes in five places: the `GeneralConfig` struct (with a serde default), `Default`, the commented template, the missing-key list in `load_or_create`, and `save()` (`set_with_comment`).
6. The settings save path today only covers `[ankiweb]` (`save_sync_settings`, `frontend/settings.rs:99-125`). Add a Device settings struct like `SyncSettings`, a `save_device_settings`, and a callback in `wire`.
7. No long-press. Do not design for it.

## Anki stats API

Pinned `anki` rev `136bd42` (`Cargo.lock`). The checkout is in `~/.cargo/git/checkouts/anki-9c9b125b7236058d/136bd42/`.

- `Collection::graph_data_for_search` is `pub(crate)`. The public way is the trait method `anki::services::StatsService::graphs(GraphsRequest { search, days }) -> GraphsResponse` (`rslib/src/stats/service.rs:22-27`). `session.rs:12` already imports `SchedulerService` the same way. Add `StatsService`.
- `search` is an Anki search string. An empty string means the whole collection.
- `days`: 0 means all history. Otherwise only the revlog graphs (today, reviews, hours, buttons) are cut. Card graphs (card counts, future due, intervals) ignore `days`.
- `GetGraphPreferences` (same trait) gives the first weekday for the calendar and whether inactive cards are counted separately.
- `GraphsContext` loads every matched card and revlog row. Run it on the worker thread.

### One deck

- `deck:"Parent::Child"` includes the subdecks.
- Build the string with `SearchNode::from_deck_name` and `SearchBuilder` (`search/builder.rs:170`). It escapes `*` and `_`.
- `SearchNode::from_deck_id` writes an empty string, so it cannot be used for a search string. `SearchBuilder::from_decks` gives `did:...` without subdecks.
- `DeckNode.name` is only the last path component (`frontend/models.rs:83-85`). For the full name, call `col.get_deck(DeckId)` or join the ancestors with `::` through `parent_index`.

### Design to API

| Design item | Source |
|---|---|
| Today: cards | `today.answer_count` |
| Today: time | `today.answer_millis` |
| Today: correct | `today.correct_count` as a share of `answer_count` |
| Today: again | Not in `Today`. See below. |
| Card counts | `card_counts.excluding_inactive`: new, learn, relearn, young, mature, suspended, buried |
| Future due, 30 days | `future_due.future_due`, keys 0 to 29. Negative keys are backlog. |
| Reviews, 30 days | `reviews.count`, keys -29 to 0. Sum learn, relearn, young, mature, filtered. |
| Calendar | No calendar field. Derive from `reviews.count` (one entry per day). |
| Intervals | `intervals.intervals` is a histogram (interval to card count). Compute the mean and median in pbanki. |

- Day offsets (`reviews`, `future_due`) are relative to the collection rollover, not calendar days. Convert with `timing_today()` (`rslib/src/scheduler/mod.rs:50`, public).
- **Again today.** `Today` has no again count. The cheap option is `col.search_cards("<deck> rated:1:1")`, which counts cards rated Again today, not button presses. `true_retention.today` only covers review cards. A press count needs a direct revlog read.
- **Calendar months.** `days` is a trailing window. Do one query with a large `days` (or 0) and slice by month in Rust. This changes the sentence in `ui-redesign.md` that a month change is a query for that month only. A month change is then only a slice of the data already loaded.

### Worker thread

- `spawn_card_job` (`frontend/card_jobs.rs:62-153`) takes the collection out of `LearnSession`, runs the closure on a thread, and delivers through `invoke_from_event_loop`. It uses `busy` and `job_seq`, and shows the loader after `DECK_LOADING_DELAY` (200 ms).
- It is hard-wired to `LoadedCard` and `DeckNode`. Make the runner generic, or add a sibling for stats, instead of copying the code.
- `deck_tree` runs on the UI thread and is skipped while `busy` (`frontend/study.rs:78-89`). Stats must not run on the UI thread.

## Stats: plan

1. A `StatsData` plain struct: Today (4 numbers), 5 counts, 30 future-due bars, 30 review bars, a month of day levels, the interval summary. A pure function builds it from `GraphsResponse`. Test it with a synthetic response.
2. A worker job with `Result<StatsData, String>`.
3. UI: `screens/stats.slint`, a `stats_open` state, `stats_clicked(DeckNode)`, `month_changed(delta)`. Bars are `Rectangle` elements in a `for`.
4. Add the 30 % and 60 % ink grays to `Theme` (the lint rule forbids color literals elsewhere).

## Open questions

- Closed: Again on the Today block is the number of cards rated Again today (`<deck> rated:1:1`).
- Closed: the key work stays in the app. Left, Down and PageDown count as Prev; Right, Up, PageUp, Return and Menu count as Next. The fork only gets `request_full_refresh()`.
- Closed: 0 Again, 1 Hard, 2 Good, 3 Easy.
- Does the system act on a key the app reports as handled? Needs a test on the PB710.

## Checks

- `cargo test -p pbanki-core`: the resolver, the config round trip with missing keys, the stats mapping.
- `make lint`.
- Desktop with `--features eink-sim` at 1264×1680, 1404×1872 and 758×1024. PageUp and PageDown stand in for Prev and Next.
- On the PB710: Prev and Next arrive as expected, and the stats query time on a large deck.
