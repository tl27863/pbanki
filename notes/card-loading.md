# Card loading

## Worker jobs

- `next_card` and `rate_card` in `crates/core/src/study.rs` use only the collection. They return `LoadedCard`.
- `crates/core/src/frontend/card_jobs.rs` takes the collection out of `LearnSession`. It runs the job on a `std::thread` and puts the collection back when the result arrives.
- `spawn_collection_job` is the generic runner: it sets `busy`, moves the collection (an `Option`) to the worker, and keeps the single running job in the thread-local `PENDING_JOB`. On delivery it puts the collection back, clears `busy`, increments `job_seq`, and calls the `finish` closure of the job on the UI thread (`None` if the worker panicked). `spawn_card_job` is its caller for cards (loading timer and `finish_job`). Sync and stats use the same runner.
- The worker sends results with `slint::invoke_from_event_loop`. Both backends implement the event loop proxy.
- The `DeliverOnDrop` guard of the worker schedules `deliver_job`, also when the job panics. The worker drops the sender first. Thus delivery always sees the result or a disconnected channel.
- Slint timers fire only because the backend uses `duration_until_next_timer_update`.

## The busy flag

- While a job or a sync runs, `LearnSession.busy` is true. `MainWindow.busy` copies it.
- While busy, the app ignores Home and deck-selection navigation.
- `LearnScreen.busy` disables Show Answer, the rating buttons, and the study keys. Thus the controls of the previous card do not catch taps aimed at the next card.
- `MainWindow.loading` shows the full-screen loader (deck entry, stats). `MainWindow.card-loading` shows the loading icon in the study top bar (rating).
- Do not touch the collection from UI callbacks without a check of `busy`. The lint rule `pbanki-collection-busy-check` gives a hint.

## Loading screens

Deck entry:

- The loading screen (`components/loader.slint`: four squares, one more filled every 500 ms, and the fixed text "Loading deck") shows only if the job still runs after 200 ms (`Loading::Transition(DECK_LOADING_DELAY)`). The stats screen uses the same loader and delay (`frontend/stats.rs`). Thus a fast load presents the card with one full present, without a loading screen.
- The deck list stays visible until then. Rust sets `MainWindow.learning` (from the loading timer, `finish_job`, or the collection-unavailable error), not `ui/main.slint`. Thus `LearnScreen` never shows the card of the previous session.
- `LearnScreen` is then created while Slint evaluates the window layout. It takes key focus from a one-shot `Timer`, not from `init => { self.focus(); }`. The `init` call entered the geometry binding again and stopped with "Recursion detected". Do not put focus calls in `init` of screens that are created conditionally. The lint rule `pbanki-no-focus-in-init` checks this. Home takes key focus the same way, for the pager keys.
- `on_open_settings` ignores taps while busy, because the gear stays available during that time.

Rating:

- A rating shows no loading screen. The answered card stays visible, and after 300 ms (`Loading::Deferred(RATING_LOADING_DELAY)`) the loading icon shows in the activity slot of the top bar (`card-loading`). A card that arrives sooner costs no extra panel update. The icon steps through three frames at 500 ms; the time to the next card with the icon is not measured on the PB710 yet (`ui-redesign.md`, "Device checks").
- The rating handlers do not change `show_answer`. The answer stays visible until the next card shows. `changed card` resets it.
- When a job finishes, the app increments `job_seq`. Thus its loading timer cannot fire during a later sync or job.

## Card pages

- The job also renders the card (notes/blitz-card-render.md). `next_card` sends both sides to the card-render thread (`CardRenderer::start_render`) and waits for question page 1 and the answer start page. This adds about 100–200 ms to the job; `busy` covers it.
- While it waits, the job builds the documents of the next two cards in the queue and sends them as lookahead. The render thread renders them while the user reads. When the next job asks for one of them, the pages are ready, and the job is the Anki part only. A wrong prediction (a learning card came due, an edit, another deck) costs nothing extra: the card renders as before. `blitz-card-render.md`, "The card-render thread".
- The card area size comes from `MainWindow.card-area-width/height` times the window scale factor (`frontend/study.rs`, `render_context`). The top bar and the answer bar have fixed heights (`Constants.bar_height`, `answer_bar_height`) and are edge to edge. The card area keeps the 20 px screen padding on all four sides, so the page size is the same as before the redesign. The area is known before `LearnScreen` exists.
- `finish_job` installs the pages in `frontend/card_pages.rs`, then `show_new_card` resets `show-answer` and `study-page` and sets the card. Do not call `set_current_card` directly (lint rule `pbanki-show-new-card`): a `changed card` handler in Slint runs after the first frame of the card, which then showed the old page state.
- Later pages arrive from the render thread through `upgrade_in_event_loop`. If the page is the one waiting to show, `card_pages::deliver` changes `page-version` and calls `request_redraw()`, as for job results.

## Explicit redraws

- The loading and result changes of card jobs call `request_redraw()` explicitly. The backend then presents their damage without more input.
- On the PB710, property invalidation alone sometimes left a loading screen or a delivered card invisible until the next touch. This happened also when the worker finished in milliseconds.
- The `request_redraw()` calls in `spawn_card_job`, its loading timer, and `finish_job` are intentional. The same applies to the sync result and the sync error in `frontend/sync.rs`.
- After an InkView or Slint upgrade, do these checks on the device before you remove them. None of these steps must need an extra touch.
  1. Cold launch, then deck selection.
  2. Loading screen to card.
  3. Rating.
  4. Home and deck entry again.
- Device logs show if a stale screen was never rendered, never sent, or dropped by the panel (see `eink-logs.md`).

## Deck selection cache

- `set_current_deck` takes about 135 ms on the PB710. The app calls it only when the selected deck changes (`LearnSession.current_deck_id`). This prevents one transaction per card.
- `session::reset_session_memory` clears the cache after each sync and after a failed card load. A merge can pull the `curDeck` of another device or delete the deck.
- Deck entry logs `[deck <id>] entering, set_deck=<bool>`.

## Rating and timing

- If `answer_card` fails, `rate_card` returns an error card. It does not load the unchanged queue head. A test in `study.rs` covers this.
- The app records `start_time` when it shows the card. Thus `milliseconds_taken` does not include load and render time.

## Bury, suspend, and undo

- The study top bar has three icon buttons: Undo (only while `MainWindow.can-undo`), Bury, Suspend. Order from the left: audio, Undo, Bury, Suspend, activity slot, counts. Everything to the left of Bury and Suspend can come and go, so they keep their place. The counts have a minimum width for the same reason.
- They use the card job of a rating (`JobKind::Action`, `Loading::Deferred`) with `study::apply_card_action`: `bury_or_suspend_cards` with `Mode::BuryUser` or `Mode::Suspend`, or `undo()`, then `next_card`. `frontend/study.rs` has one `card_action` closure for the three callbacks. It returns while `busy`.
- An action that changes nothing is an error (`0` cards changed: a missing card, or a card that is suspended already), like a rating that cannot be saved. Undo is an error unless the last undoable operation is a bury or a suspend (`undo_is_card_action`), so it never reverts a rating.
- Undo shows only if `LoadedCard::can_undo` (the collection's last undoable step is a bury or a suspend, `undo_status`) and `LearnSession.undo_depth` is above 0. The depth counts this study session's own steps (`card_jobs::undo_depth_after`): a deck entry sets it to 0, a bury or a suspend adds 1, an Undo takes 1 off, a rating sets it to 0 (an answer is then on top of the stack). Without it, Undo would reach a bury from before the deck entry: rslib's undo stack outlives the entry, and entering the same deck adds no step. `reset_session_memory` (sync, failed load) clears it, as a sync clears Anki's undo history.
- Burying the last card of the deck shows the "No more cards due!" message with the Undo button.
- `IconButton.keep-icon` keeps the icon while the button is disabled. The three buttons use it, so a job start (for example a rating) does not blank them: no extra panel update. The `busy` check in Rust stops the second tap. The lint rule `pbanki-icon-keep-needs-busy` requires `enabled: !root.busy` on every `IconButton` with `keep-icon: true`.
- Bury is `BuryUser`: it ends at Anki's next-day rollover and does not bury siblings (deck-option sibling burying runs only when a card is answered). Suspend lasts until the card is unsuspended in Anki.
- Bury and suspend clear the study queues in rslib (`maybe_clear_study_queues_after_op`), so the next card builds them again. It is slower than after a rating. Not measured on the PB710 yet.
- These jobs do not count for `full_refresh_every` or for the `[card] lookahead` hit rate.
- `card_pages::next_card_version` changes `page-version` for every new card (`show_new_card`). The card image binding depends on the side, the page and the version only. A bury or a suspend on the question side leaves all three as they were, so the old card's image stayed until this.

## Hardware keys

- `learn.slint` only classifies the key: Left, Down and PageDown are Prev; Right, Up, PageUp, Return and Menu are Next. It calls the non-blocking `study_key(key)` callback.
- `frontend/study.rs` resolves the key with `keys::resolve` (side, page, page count, `KeyConfig`) to a page turn, Show answer, or a rating. A page turn has precedence. The rating goes through the same path as a tap on a `RatingButton`.
- Defaults: question Next = Show answer, Prev = nothing; answer Next = Good, Prev = Again. Before the redesign, Next rated Hard and any key revealed the answer.
- The actions are in the config (`key_prev_question` and so on) and in Settings > Device. `LearnSession.key_config` holds a copy. `MainWindow.key-hints` places the key glyphs (`icons/key_prev.svg`, `key_next.svg`): beside the page indicator while the key turns the page, else before the label of the control that the key triggers. `learn.slint` decides this for each key (`prev-turns-page`, `next-turns-page`), with the same page test as `keys.rs` `resolve`.

## Home deck list

- Home shows one page of fixed 48 px rows and a pager. Nothing scrolls (lint rule `pbanki-no-scroll-widgets`).
- `MainWindow.deck-rows-per-page` comes from the window height (the bars and the header have fixed heights, and the window is shorter with the system panel). When it changes, Slint calls `deck_page_changed(0)`.
- `frontend/decks.rs` keeps the tree, the collapsed set and the page in `LearnSession.deck_list`. `models::visible_decks` hides every deck with a collapsed ancestor; `models::page_slice` cuts the page. A row emits `toggle`; it does not change `collapsed`. The collapsed set lasts for the session. A deck that is new to the app starts with Anki's collapsed state.
