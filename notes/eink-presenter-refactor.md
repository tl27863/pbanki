# E-ink presenter refactor (Blitz phase 1)

Phase 1 of `deep-research/blitz-on-inkview.md`. The user decided to go for the full Blitz backend.

## Status

2026-11-02: done. Steps 1–5 are on the fork branch `pbanki-eink-core` (`55c0854`). The PB710 check passed (`pbanki-20261102-195601.log`). `pbanki-device-panel` is fast-forwarded to `55c0854` and pushed. Rollback target: `070ce4f`. Open: the pbAnki `Cargo.lock` update (`cargo update -p inkview -p inkview-slint`, its own commit).

| Commit | Step |
|---|---|
| `ecf8244` | 1: characterization tests, `MockScreen` |
| `dfce54b` | 2 and 3 in one commit: `DamagePath`, `Presenter`, `Surface` (retained frame), `Clock` |
| `b02de11` | 4: `EinkPresenter`, `run_event_loop` on top of it |
| `55c0854` | 5: the new crate `inkview-eink` |

Checks: 35 tests in `inkview-eink` and `inkview-slint`. Clippy is clean for `sdk-6-10` and `sdk-6-11`, and `cargo doc` gives no warnings. In pbAnki (local path patch, not committed): `make lint` clippy commands, `cargo test -p pbanki-desktop --features eink-sim`, `cargo test -p pbanki --features sdk-6-10`, and a softfp release build.

Differences from the plan below:

- Steps 2 and 3 are one commit. Both change the same loop lines.
- Methods do not take `now`. `EinkPresenter` reads a `Clock` (`Instant::now()`, or a manual time in tests) at the same points as the old code. For example, `last_draw_at` is read after the update call, as before.
- `EinkPresenter::new` logs the policy, waveform and panel lines (at backend construction, as before). `start()` requests the launch present when the loop starts, as before. Thus `issued reason=… after=` still counts from the loop start.
- `on_event` replaces `on_lifecycle`. It also writes the pointer, key and `evt:` trace lines. `after_dispatch` returns only `redraw`; `full_render_needed()` gives the full-render flag.
- `Surface` holds the screen, `top`, the retained frame and the wake flag. `inkview-eink` also exports `wait_events` (step 4 of the loop) and `content_pointer`.
- `eink_log!` is exported from `inkview-eink`. The Slint backend writes its startup line with it, so the epoch and the format stay the same.

## Review follow-up

2026-11-02: changes after the architecture and code review of `070ce4f..55c0854`. They are four commits on `pbanki-device-panel`, pushed after the PB710 check:

| Commit | Change |
|---|---|
| `b9ae258` | `wait_delay` |
| `8b30c92` | The loop owns the presenter |
| `bfaa0b0` | Frame completeness, private fields, tests |
| `fc8ff7f` | Docs: `start()`, `EinkOptions`, loop order |

Device check: passed (`pbanki-20261102-202534.log`, softfp build against `fc8ff7f`). Launch, the system panel tray, sleep and wake, Settings and Home, deck entry and Home: each gave one `issued` and one `verify issued`. No `finalize fallback`. `first full frame rendered` showed once (no panel height change). Timings are the same as the `55c0854` build. Not tested on the device: the Show + new height + Background case (unit test only). Checks: 37 tests in `inkview-eink` and `inkview-slint`. Clippy for `sdk-6-10` and `sdk-6-11` gives no new warnings. `cargo doc` gives no warnings. In pbAnki (local path patch): the `make lint` clippy commands, `cargo test -p pbanki-desktop --features eink-sim` and `cargo test -p pbanki --features sdk-6-10`.

| Change | Why |
|---|---|
| `EinkPresenter::wait_delay(ui_deadline, animating)` and `IDLE_WAIT`. `run_event_loop` calls it for step 3. | Step 3 (no wait while an application request is set) was a device regression. A second backend must not copy it. |
| `Backend` holds `RefCell<Option<EinkPresenter>>`. `run_event_loop` takes the presenter, runs the loop, and puts it back. A second run while the loop runs returns `PlatformError::Other`. | Before, a `borrow_mut` stayed active for the whole loop. A future backend method that borrows the presenter from a Slint callback would panic. |
| `have_frame` moved from `Presenter` to `Surface` (`is_complete`, `mark_complete`). `refresh_panel_height` sets it to false. | Before, a new panel height reallocated the frame (black pixels), but the flag stayed true. Show with a new height and Background in one drain cleared the full render. The next Foreground then blitted the incomplete frame. Now the present waits for a full render. |
| `Presenter` and `Surface` fields are private (`show`, `is_visible`, accessors). | `EinkPresenter` wrote the presenter's fields directly. |
| Test `resize_then_background_renders_in_full_before_the_next_present`, test `wait_delay_follows_the_request_animations_and_deadlines`. | Nothing tested that Background drops the full render. Mutation checks: both bugs now fail a test. |

Behavior changes (device check needed before a push):

- After a panel height change, `first full frame rendered in …ms` shows again (see `eink-logs.md`).
- The Show + resize + Background case above. Other transitions give the same updates and log lines.
- `run_event_loop` reads Slint's next timer and the animation state in each iteration, also when an application request is set. Both are queries without side effects.

### Step numbers

The rustdoc on `EinkPresenter` uses 10 steps. The plan below uses 13.

| Rustdoc | Plan | Call |
|---|---|---|
| 1 | 1 | backend timers |
| 2 | 2 | `sample_panel` |
| 3 | 3 | `wait_delay` |
| 3, 4 | 4 | `wait_events` |
| 5 | 5 | `before_dispatch` |
| 6 | 6 | `on_event`, then the UI input |
| 7 | 7 | backend timers |
| 8 | 8–11 | `after_dispatch` |
| 9 | 12 | render, `submit_frame` |
| 10 | 13 | `after_render` |

### Open design points (for phase 2)

- **Loop order is not enforced (A1, large option).** The types do not force the call order. Each backend follows the rustdoc list. When the Blitz shell exists, a shared driver in `inkview-eink` over a backend trait (`update_timers`, `animating`, `dispatch`, `render(frame, full) -> Option<(Rect, Duration)>`) can own the order. Do not design that trait from Slint alone.
- **Two-step start (A2).** `EinkPresenter::new` logs the policy, waveform and panel lines at backend construction. `start()` requests the launch present when the loop starts, so `request reason=Launch` and `after=` count from the loop start, as before. A backend that does not call `start()` gets no launch present: the screen stays as it was, with no error. A second call starts over. A typestate or a `debug_assert!` can catch a missing `start()`. Not done, because the log timing must stay the same.
- **Environment variables (A4).** `EinkPresenter::new` applies `INKVIEW_SLINT_UPDATES`. The backend reads `INKVIEW_SLINT_TRACE_EVENTS` and sets `EinkOptions::trace_events`, because the Slint startup line also shows it. The Blitz shell must read it too. Both keep the `INKVIEW_SLINT_` prefix, so device test commands do not change. If a second backend exists, move both reads to one place (`EinkOptions::from_env`).

## Device check (step 6)

Result on the PB710 (fw U710.6.11.2124, softfp build): cold launch, deck entry (two decks), show answer and rating (also the `Next` key), Home, Settings and back, the system tray, a second Foreground, and sync. No step needed an extra touch. Each transition gave one `issued` and one `verify issued`. No `finalize fallback`. The log lines are the same as before the refactor. Not tested: Settings opened automatically (no collection), which uses the same `request reason=App` path.


Build pbAnki against the local fork without a change to `Cargo.toml`, then restore `Cargo.lock`:

```sh
cat > /tmp/inkview-patch/local.toml <<'TOML'
[patch."https://github.com/tl27863/inkview-rs"]
inkview = { path = "/Data/Project/inkview-rs/inkview" }
inkview-slint = { path = "/Data/Project/inkview-rs/inkview-slint" }
TOML
cp Cargo.lock /tmp/pbanki-Cargo.lock.bak
RUSTFLAGS="-C target-feature=+vfp4,+neon,-soft-float" cargo zigbuild --config /tmp/inkview-patch/local.toml \
    --target armv7-unknown-linux-gnueabi.2.38 --profile release -p pbanki --features=sdk-6-10
cp /tmp/pbanki-Cargo.lock.bak Cargo.lock
```

Then copy the binary into `target/pocketbook/` as `pbanki.bin` and run the checklist under "Behavior check". After the check: fast-forward `pbanki-device-panel` to `pbanki-eink-core` and push. Then update the pbAnki lock (`cargo update -p inkview -p inkview-slint` alone, as its own change).

## Goal

Move the e-ink logic out of the Slint backend into a crate without Slint, so a Blitz shell (phase 2) uses the same code. Slint behavior must not change: the same updates in the same order, and the same `[eink …]` log lines.

The e-ink logic is everything in `inkview-slint/src/lib.rs` `run_event_loop` that is not Slint:

- The `Presenter` state machine (full presents, verification, wait budget).
- The damage path: partial when idle, dynamic when busy, finalizer, fallback.
- The `UpdatePolicy` and `UpdateMode` (`policy.rs`).
- The retained frame, the system panel rows (`top`), `blit_all`, and the resize on a panel height change.
- The `FULL_PRESENT_REQUESTED` latch and `request_full_present()`.
- The `EinkScreen` trait (`eink_screen.rs`).
- `eink_log!` and its epoch, the wake after each update, `scale_from_dpi`, and the panel-row pointer rule.

## The loop order to keep

Each item below was a device regression (`eink-refresh.md`, `card-loading.md`). The order of one iteration of `run_event_loop` today:

1. `update_timers_and_animations()`.
2. `presenter.sample_panel()` (busy→idle log line).
3. Delay: 0 if the latch is set. Non-blocking if Slint animates. Otherwise the minimum of the Slint timer, the finalizer, the presenter deadline and the panel poll, or 1 s if none is set. The 1 s is not a cap: a later Slint timer is waited for in full.
4. Wait for the first event, then drain the queue (Show and Foreground become one present).
5. Finalizer: if dynamic damage waits and the last draw is older than 200 ms, issue it (partial), retry after 20 ms while busy, or use the fallback after 100 retries.
6. Dispatch in queue order: callbacks inline. Show, Repaint and Foreground request a present (and re-read the panel height). Background and Hide drop it. Input goes to Slint.
7. `update_timers_and_animations()` again, so `changed` handlers that input queued render in this iteration.
8. Consume the latch (`request(App)`) after dispatch and before rendering.
9. `poll_verification(now)`.
10. A new present drops the waiting dynamic damage.
11. Pending present without a complete frame: force a full render.
12. Render. A full render first sets the "complete frame" flag. If a present is pending, damage only updates the retained frame. Zero-area damage issues nothing. Otherwise copy the damage and issue partial (idle) or dynamic (busy, ≤ 100k px, ≥ 20 ms since the last draw), and arm the finalizer.
13. Pending present: if the panel is busy, wait (20 ms retries, max 1.2 s), then blit all, draw the panel, issue `present` over the full screen, wake, and go to `Verifying` (300 ms) or `Idle`.

The shared API below keeps steps 2, 3, 5 and 8–13 in the shared crate. A backend can then only call them in this order.

## New crate: `inkview-eink`

In the `../inkview-rs` workspace, next to `inkview-slint`. Dependencies: `inkview` (default features off, SDK features forwarded as in `inkview-slint`), `euclid`, `log`. No Slint.

| Module | Content | From |
|---|---|---|
| `screen.rs` | `EinkScreen` and its impl for `inkview::screen::Screen` | `inkview-slint/src/eink_screen.rs`, moved unchanged |
| `policy.rs` | `UpdateMode`, `UpdatePolicy`, `UPDATES_ENV` | `inkview-slint/src/policy.rs`, moved unchanged. The variable stays `INKVIEW_SLINT_UPDATES`, so device test commands do not change. |
| `log.rs` | `eink_log!`, epoch, `since_epoch_ms` | `lib.rs`. Log target stays `eink`, so `eink-logs.md` stays correct. |
| `present.rs` | `PresentReason`, `PresentKind`, `PresentState`, `Presenter`, the latch, `request_full_present()` | `lib.rs`, moved unchanged |
| `damage.rs` | `DamagePath` (new struct for the five loop variables: `fulfill_after`, `dynamic_region`, `accumulated`, `finalize_retries`, `last_draw_at`) | Code of steps 5 and 12 |
| `queue.rs` | `wait_events(rx, delay, out)`: block for the first event or until `delay`, non-blocking if `delay` is `None`, then drain. Returns `Disconnected`. | Step 4 |
| `input.rs` | `content_pointer(top, kind, x, y) -> Option<(i32, i32)>`: presses on panel rows are dropped. Move and release are clamped to `top` and shifted. | `ink_evt_to_slint` |
| `lib.rs` | `EinkPresenter<S>` and `scale_from_dpi` | — |

### `EinkPresenter<S: EinkScreen>`

Owns the screen, the size, `top`, the policy, the retained frame (`Vec<S::Pixel>`), the `Presenter`, the `DamagePath`, and the wake-logged flag.

```rust
impl<S: EinkScreen> EinkPresenter<S> {
    pub fn new(screen: S, options: EinkOptions) -> Self;        // startup log lines as today
    pub fn content_size(&self) -> (usize, usize);               // width × (height - top)
    pub fn top(&self) -> usize;
    pub fn visible(&self) -> bool;

    // Step 2 and 3.
    pub fn sample_panel(&mut self);
    pub fn deadline(&self, now: Instant) -> Option<Duration>;   // finalizer, present, panel poll

    // Step 5.
    pub fn before_dispatch(&mut self, now: Instant);

    // Step 6, for each InkView event. Returns what the backend must do.
    pub fn on_lifecycle(&mut self, evt: &inkview::Event) -> Lifecycle; // { redraw, full_render, resized }

    // Steps 8 to 11. Returns { redraw, full_render }.
    pub fn after_dispatch(&mut self, now: Instant) -> FrameRequest;

    // Step 12. The backend renders into `frame_mut()`, then calls `submit_frame`.
    pub fn frame_mut(&mut self) -> &mut [S::Pixel];
    pub fn submit_frame(&mut self, damage: Rect, render: Duration, full: bool);

    // Step 13.
    pub fn after_render(&mut self, now: Instant);
}
```

- Methods take `now` where the old code read the clock in the middle of a step. Tests can then drive time. The timing stays the same: the backend passes `Instant::now()` at the same points.
- `frame_mut()` returns `&mut [S::Pixel]`. `inkview-slint` casts it to `&mut [Pixel<P>]` (`repr(transparent)`, the reverse of the existing `pixels()` cast). `Pixel<P>` and its `TargetPixel` impl stay in `inkview-slint`, because `TargetPixel` is a Slint trait.
- `Rect` is `euclid::Rect<i32, UnknownUnit>`, as today.

### What stays in `inkview-slint`

- `Backend<S>` with `slint::platform::Platform`, `Proxy`, the `Event` enum (`InkviewEvent`, `Callback`, `Quit`).
- `MinimalSoftwareWindow`, `draw_if_needed`, the `RepaintBufferType` switch for full renders.
- `ink_evt_to_slint` and `ink_key_to_slint` (using `content_pointer`).
- Re-exports: `pub use inkview_eink::{EinkScreen, UpdateMode, UpdatePolicy, UPDATES_ENV, request_full_present};`. pbAnki (`crates/pocketbook/src/main.rs`, `crates/desktop/src/eink_sim.rs`) then needs no change.
- `BackendOptions { policy }` stays and maps to `EinkOptions`.

`run_event_loop` then becomes about 80 lines: steps 1, 3 (Slint timer and animations plus `deadline()`), 4, 6 (callbacks, input), 7 and 12 (render) are Slint. The rest are calls.

### How the Blitz shell (phase 2) uses it

The same calls, in the same order. Step 1 and 7 are `doc.poll()` and `resolve(t)`. Step 12 renders with vello_cpu and converts into `frame_mut()` (section 4 of the research note). This sketch checks that the API is complete. Do not build the shell in phase 1.

## Steps

Each step is one commit on a new fork branch `pbanki-eink-core` (from `pbanki-device-panel`). After each step: `cargo test -p inkview-slint --no-default-features --features sdk-6-10`, `cargo clippy` for the fork, and in pbAnki `make lint` and `cargo test -p pbanki-desktop --features eink-sim` (with a `[patch]` or a path override to the local fork, not committed).

1. **Characterization tests first, on the current code.**
   - A test-only `MockScreen: EinkScreen`. It records each `copy_rect`, `update`, `update_ex` and `draw_panel` call. Its busy state follows a script (busy for N ms after each update).
   - Tests for `Presenter` (it is a private struct in `lib.rs`, so tests in the same module can reach it): request coalescing, `hide` drops the request and the latch, verification 300 ms after issue, wait budget 1.2 s.
   - These tests stay. Later steps move them with the code.
2. **Extract `DamagePath`** inside `inkview-slint` (steps 5 and 12). Pure move. Tests with `MockScreen` and explicit `now`:
   - Idle damage → one `partial` of the damage box.
   - Busy, small → `dynamic`, then a `partial` finalize after the panel is idle and 200 ms passed.
   - Busy, larger than 100k px → no dynamic, finalize later.
   - Stuck busy → `fallback` (`full`) after 100 retries.
   - Zero-area damage → no update.
   - A new present drops waiting damage.
3. **Extract the retained frame** (`top`, buffer, `blit_all`, `refresh_panel_height`, `resize`). Test: panel height change resizes and forces a full render. A present blits only content rows and calls `draw_panel` when `top > 0`.
4. **Add `EinkPresenter`** with `before_dispatch`, `on_lifecycle`, `after_dispatch`, `submit_frame`, `after_render`. `run_event_loop` calls it. Test: one full scripted sequence (launch → first frame → Show + Foreground in one drain → one present → verify → idle damage → busy damage → finalize) gives the same `MockScreen` call list as step 1 recorded.
5. **Move to the new crate** `inkview-eink`. `inkview-slint` depends on it and re-exports. Move the tests with the code.
6. **Device check and switch.** Build pbAnki against `pbanki-eink-core`, run the checklist below, then fast-forward `pbanki-device-panel` to it. `pbanki-device-panel` before the merge is the rollback target (note its hash in `inkview-backend.md`).

## Behavior check

- `[eink …]` log lines must be the same, apart from timestamps. Compare a pre-refactor and a post-refactor session for the same steps:
  - SDK emulator: `make sdk-emulator` with `INKVIEW_SLINT_TRACE_EVENTS=1`.
  - eink-sim: `cd crates/desktop && cargo run --features eink-sim`.
  - PB710: the user deploys and shares `pbanki/logs/`.
- PB710 checklist. No step may need an extra touch (`card-loading.md`):
  1. Cold launch, then deck selection.
  2. Loading screen to card (a large deck, so the 200 ms loading screen shows).
  3. Show answer, rate, several cards.
  4. Home, then deck entry again.
  5. Settings open and back. Settings opened automatically (no collection).
  6. Pull down the system panel tray and return (`request reason=Foreground`).
  7. Sleep cover or power button, then wake.
  8. Sync (Wi-Fi dialog, then back to the app).
- Check in the logs: one `issued reason=…` plus one `verify issued` per transition, `update partial`/`update dynamic`/`update finalize` for content, no `finalize fallback`.

## Risks

| Risk | Mitigation |
|---|---|
| A changed step order brings back an old regression (blank screen after resume, card invisible until touch) | The API fixes the order. Characterization tests first. Same log lines. Device checklist. |
| Timing differences (extra `Instant::now()` calls, borrow changes) | Pass `now` at the same points as today. No new waits. |
| Hidden coupling to Slint (the latch is read after Slint's `changed` callbacks) | Step 7 stays in the backend, before `after_dispatch`. Document it on `after_dispatch`. |
| The fork drifts from upstream | The new crate is additive. `inkview-slint` keeps its public API. Rebase notes in `inkview-backend.md`. |
| `RefCell` borrows across the Slint render closure | Done differently: `Backend` keeps `RefCell<Option<EinkPresenter<S>>>`, and the loop takes the presenter for its whole run (see "Review follow-up"). No borrow is active while Slint callbacks run. |

## Out of scope

- The Blitz shell and pbAnki UI port (phases 2 and 3).
- The single-threaded InkView pump (`inkview-single-thread-pump.md`). The new crate makes it easier later, because the loop parts are in one place.
- softfp flags for `pbanki.bin` (a separate change, see `performance.md`).
- New update behavior. Any policy change needs PB710 measurements (`eink-refresh.md`).
