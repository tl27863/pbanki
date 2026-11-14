# E-ink refresh

The backend sends two types of screen update: content updates and full presents. Damage size never decides which type to use. The app requests full presents explicitly.

## Content updates

Content updates are the answer reveal, rating to the next card, and page turns. They use the damage path:

- Panel idle: `PartialUpdate` of the damage bounding box.
- Panel busy: `DynamicUpdate` for the accumulated damage, up to 100k px. Larger damage waits for the finalizer.
- Finalizer: when the panel is idle again, it sends one `PartialUpdate` of the union of the waiting damage.
- Fallback: if the panel stays busy for about 2 s, the finalizer sends `FullUpdate` (flashing).
- Periodic full refresh: off by default. See "Periodic full refresh" below.

Content updates do not blit the full frame again and do not call `WaitForUpdateComplete`.

## Full presents

Full presents are screen transitions and task show or resume. A full present blits the retained frame again and sends a non-flashing full-screen `PartialUpdate`.

The `Presenter` state machine (`Idle | Pending | Verifying`) owns them:

- Requests come from `Show`, `Foreground`, `Repaint`, and `inkview_slint::request_full_present()`.
- Requests coalesce. The backend reads all queued events before it renders, so Show plus Foreground is one present.
- The present waits until the panel is idle, for a maximum of 1.2 s. Then it sends the update anyway.
- One verification present follows 300 ms after the first present is sent.
- The request is an atomic latch. The backend reads it after event dispatch and before rendering, so the present contains the new screen.

The app requests presents through `FrontendHooks.full_present`. `crates/pocketbook/src/main.rs` sets it, and the eink-sim frontend sets it. The winit desktop frontend does not. The app requests a present for:

- Deck entry: the loading screen (if shown) and the first card. `CardJob.full_transition` tells deck entry from rating jobs.
- Home.
- Settings open and back, also when Settings opens automatically.
- Stats: the loader (if shown), the result, and back.

## Periodic full refresh

`full_refresh_every` (Settings > Device; 0, 1, 5, or 10 cards; default 0) clears ghosting with one flashing update.

- App: `LearnSession.rated_since_refresh` counts rated cards in `finish_job` (`full_refresh_due` in `frontend/card_jobs.rs`). At the limit, the present of the next card calls `FrontendHooks.full_refresh` in place of the plain redraw, and the count starts again. A deck entry is a full present already, so it also starts the count again.
- Fork: `inkview_slint::request_full_refresh()` is a second latch beside `request_full_present()`. The backend reads it after event dispatch and before rendering. The `Presenter` then holds the damage of that frame and sends one full-screen update with the `fallback` mode (`full`) in its place. No verification present follows a refresh: a second update 300 ms later would fight the flashing update.
- A refresh request that meets a pending full present only changes the mode of that present. The present keeps its reason and its verification. A refresh replaces a waiting verification.
- Log: `request reason=Refresh`, then `issued reason=Refresh flash=true …`.
- `crates/pocketbook/src/main.rs` and the eink-sim frontend set the hook. The winit desktop frontend does not. In eink-sim, `full_refresh_every = 1` gives the flashing refresh after each card. Not checked on the PB710 yet.

## Why the design is like this

Each item below was a regression on the device.

- InkView can discard a frame that the app draws while InkView switches tasks. `Foreground` alone makes no Slint damage. Thus the screen stayed blank until a touch. Show and resume must present the retained frame again and verify it.
- Slint marks the whole window (1264×1680) dirty for answer and next-card changes. Thus damage area cannot tell a screen change from a content change. Area thresholds made every rating a full present (1–1.5 s lag). Keep transitions explicit.
- `WaitForUpdateComplete` on content updates blocked the UI thread for 3–4 s per Show Answer.
- Count the verification delay from the time the present is sent, not from the request time. A long first render otherwise skipped the verification.
- Each pressed, hover, or animated style on a study control is one more panel update. It delays the next full-screen update by 0.5–0.8 s. Show Answer (`PlainButton`), `RatingButton`, `IconButton` and `KeyGlyph` have no pressed, hover, or animated styles. Keep it that way. The lint rule `pbanki-study-controls-no-states` checks this.

## Update policy

Each update that the backend sends goes through `UpdatePolicy` (`inkview-eink/src/policy.rs` in the fork). The policy gives one update mode for each class:

| Class | When | Default |
|---|---|---|
| `idle_content` | Damage while the panel is idle | `partial` |
| `busy_content` | Small damage while the panel is busy | `dynamic` |
| `finalize` | Waiting damage, once the panel is idle | `partial` |
| `present` | Full presents | `partial` (`partial-hq` for color panels) |
| `fallback` | Panel seems stuck | `full` |

- `UpdatePolicy::for_panel` selects `partial-hq` for `present` when `DeviceInfo` reports a color mask.
- The defaults are the tuned PB710 behavior. Do not change them without PB710 measurements.
- Zero-area damage sends no update.

Modes:

- An `UpdateKind` name (`full`, `full-hq`, `soft`, `soft-hq`, `partial`, `partial-hq`, `partial-bw`, `partial-du4`, `dynamic`, `dynamic-bw`) calls the named InkView function. The firmware selects the waveform.
- `ex:<waveform>` (for example `ex:gl16`) calls `UpdateEx` (SDK 6.10+) with that `waveform_type_t` in the low flag bits. Without `UpdateEx`, it uses `partial`.
- If the firmware does not have a named function, `Screen::update` uses the nearest kind and logs this once.

Override classes for device tests:

```sh
INKVIEW_SLINT_UPDATES="busy_content=dynamic-bw,present=ex:gl16"
```

The waveform probe confirmed on the PB710 that `UpdateEx` passes the waveform bits to the panel (see `performance.md`). The doc comment of `Screen::update_ex` still calls this an assumption.

Recommended device tests:

1. Set `busy_content=dynamic-bw`.
2. Compare `present=ex:gl16` with `partial` for card changes (latency against ghosting).
3. Record the results in `performance.md`.

The probe shows a floor of about 440 ms for most modes. Thus faster waveforms give little latency gain on the PB710. Ghosting and quality need a visual check.

Not available on purpose: `device_epdc_init()->update`. It goes around InkView.

## Activity icons

- `components/activity_icon.slint` steps through SVG frames with a Slint `Timer` at 500 ms (sync 2 frames, network 4, loading 3; done and error are static). There is no fade or tween. The timer runs only while the icon is visible and animated.
- Each step is a 24×24 px damage update on the normal content path. `components/loader.slint` does the same with four squares.
- The lint rule `pbanki-no-repeated-timer` covers Rust timers only. Repeating timers for these animations are Slint `Timer` elements with `running:` bound to visibility.
- Not measured on the PB710: ghosting of the loops, and the cost of the loading icon on the time to the next card. If either is bad, set `animated: false` (one static frame).

## Known minor issues

- Rating taps make empty damage. The backend skips zero-area updates, but an empty finalize still logs `update finalize 0x0+0+0`.

## KOReader reference

- `ffi/framebuffer_pocketbook.lua` maps partial to `PartialUpdate`/`PartialUpdateHQ`, full to `FullUpdate`/`FullUpdateHQ`, and fast to `DynamicUpdate`. It waits with `WaitForUpdateComplete`.
- `ui/uimanager.lua` changes every 6th partial refresh to a full refresh (`FULL_REFRESH_COUNT`, default 6).
