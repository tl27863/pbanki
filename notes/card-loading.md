# Card loading

## Worker jobs

- `next_card` and `rate_card` in `crates/core/src/study.rs` use only the collection. They return `LoadedCard`.
- `crates/core/src/frontend/card_jobs.rs` takes the collection out of `LearnSession`. It runs the job on a `std::thread` and puts the collection back when the result arrives.
- `spawn_card_job` keeps the single running job in the thread-local `PENDING_JOB`.
- The worker sends results with `slint::invoke_from_event_loop`. Both backends implement the event loop proxy.
- The `DeliverOnDrop` guard of the worker schedules `deliver_job`, also when the job panics. The worker drops the sender first. Thus delivery always sees the result or a disconnected channel.
- Slint timers fire only because the backend uses `duration_until_next_timer_update`.
- The proxy also makes it possible to move the blocking sync off the UI thread later.

## The busy flag

- While a job or a sync runs, `LearnSession.busy` is true. `MainWindow.busy` copies it.
- While busy, the app ignores Home and deck-selection navigation.
- `LearnScreen.busy` disables Show Answer, the rating buttons, and the study keys. Thus the controls of the previous card do not catch taps aimed at the next card.
- `MainWindow.loading` and `loading_message` show a loading state.
- Do not touch the collection from UI callbacks without a check of `busy`. The lint rule `pbanki-collection-busy-check` gives a hint.

## Loading screens

Deck entry:

- The loading screen shows only if the job still runs after 200 ms (`Loading::Transition(DECK_LOADING_DELAY)`). Thus a fast load presents the card with one full present, without a loading screen.
- The deck list stays visible until then. Rust sets `MainWindow.learning` (from the loading timer, `finish_job`, or the collection-unavailable error), not `ui/main.slint`. Thus `LearnScreen` never shows the card of the previous session.
- `LearnScreen` is then created while Slint evaluates the window layout. It takes key focus from a one-shot `Timer`, not from `init => { self.focus(); }`. The `init` call entered the geometry binding again and stopped with "Recursion detected". Do not put focus calls in `init` of screens that are created conditionally. The lint rule `pbanki-no-focus-in-init` checks this.
- `on_open_settings` ignores taps while busy, because the gear stays available during that time.

Rating:

- The loading overlay waits 300 ms (`Loading::Deferred(RATING_LOADING_DELAY)`). Thus the `RatingButton` that got the click is not removed inside its own click callback.
- The rating handlers do not change `show_answer`. The answer stays visible until the overlay or the next card shows. `changed card` resets it.
- When a job finishes, the app increments `job_seq`. Thus its overlay timer cannot fire during a later sync or job.

## Card pages

- The job also renders the card (notes/blitz-card-render.md). `next_card` sends both sides to the card-render thread (`CardRenderer::start_render`) and waits for question page 1 and the answer start page. This adds about 100–200 ms to the job; `busy` covers it.
- While it waits, the job builds the documents of the next two cards in the queue and sends them as lookahead. The render thread renders them while the user reads. When the next job asks for one of them, the pages are ready, and the job is the Anki part only. A wrong prediction (a learning card came due, an edit, another deck) costs nothing extra: the card renders as before. `blitz-card-render.md`, "The card-render thread".
- The card area size comes from `MainWindow.card-area-width/height` times the window scale factor (`frontend/study.rs`, `render_context`). The study rows have fixed heights (`Constants.study_menu_height`, `study_buttons_height`), so the area is known before `LearnScreen` exists.
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
