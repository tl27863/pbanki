# InkView backend (inkview-rs fork)

## Branches

pbAnki uses `inkview-slint` from the fork `tl27863/inkview-rs`. The fork development uses the local checkout `../inkview-rs`.

| Branch | Content | Role |
|---|---|---|
| `pbanki-device-panel` | `pbanki-upstream-master` at `3a651ab`, plus device info, the update API, panel modes, `EinkScreen`, `UpdatePolicy`, the waveform probe, and the presenter refactor (`inkview-eink` crate, `55c0854`). plus the review follow-up (`fc8ff7f`, see `eink-presenter-refactor.md`) | Used by the root `Cargo.toml`. Rollback targets: `55c0854` (before the follow-up), `070ce4f` (before the refactor) |
| `pbanki-eink-core` | The presenter refactor (`55c0854`), merged into `pbanki-device-panel` | Kept for reference |
| `pbanki-upstream-master` | Upstream master `15cdb98` (Sep 2026), plus `a2beb6c` (pixel formats, `copy_rect`, wake), `6765a59` (presenter, damage policy, proxy, Slint 1.18.1), `3a651ab` (DPI-only scale factor) | Rollback target for `pbanki-device-panel` |
| `pbanki-eink-refresh` | Upstream `34ae238` plus Slint `44208ff` | Older rollback target, kept unchanged |

- Upstream master `15cdb98` added a generic pixel type, `EventLoopProxy`, the `GetCanvas()` fallback, `PANEL_NO_FB_OFFSET`, and SDK 6.11 bindings.
- Keep `pbanki-device-panel` pushed. Pin a specific revision if reproducibility is important.
- The user permits commits and pushes to the pbanki branches of the fork as part of normal work.
- Upstream (`simmsb/inkview-rs`, remote `upstream` in `../inkview-rs`) has no full-refresh policy and no `Show`/`Repaint` handling. The upstream backend has no presenter and no finalizer. On the PB710 it can leave a loaded card or the loading screen invisible. Use the fork.
- To follow a newer upstream master, rebase `pbanki-upstream-master`. Keep the Slint version the same in the fork and in all three pbanki manifests. Two Slint copies fail to compile at `set_platform`.

## SDK features and bindings

- pbAnki uses `sdk-6-10` (base) or `sdk-6-11` (RK3566) with default features off. Upstream defaults to `sdk-6-11`.
- The bindings load symbols lazily. A missing symbol panics only when the code calls it. New calls must check `iv.X.as_ref().is_ok()`.
- `DeviceInfo::query` and `Screen::update` never panic on a missing symbol.
- The `sdk-5-19` feature does not compile (`EVT_CUSTOM`). This is a known problem.

## Threads and the cross-thread wake

- All InkView screen calls run on the `pbanki-ui` thread. `InkViewMain` runs on the main thread.
- On the PB710, the panel sometimes accepted an update but did not do it. The panel reported idle about 100 ms later and showed nothing until the next touch woke the main loop.
- The fix: after every update call, the backend runs `Screen::wake_event_loop()`. This posts a no-op `EVT_CUSTOM` to its own task with `SendEventTo`. `Event::from_raw` does not map it, so the backend never forwards it.
- If the stall comes back, use the single-threaded pump (see `inkview-single-thread-pump.md`).

## Rendering

- `crates/pocketbook/src/main.rs` opens the framebuffer with `WhichScreen::with_panel(iv, PanelMode)`.
  - 8-bit depth gives `BB8`. 24-bit depth gives `RGB24`. Other depths panic.
- It boxes `Backend<Screen<P>>` through `Backend::with_options` for that pixel type.
- Slint renders into a buffer in the pixel format of the framebuffer (`Pixel<P>`). The backend copies damage and blits row by row with `Screen::copy_rect` (5–9 ms for a full gray screen).
- Color to gray uses integer Rec. 709 luma. Neutral grays do not change.
- The color path has unit tests only. No color device was tested. The framebuffer byte order (assumed RGB) is not verified.
- InkView events reach the backend as `inkview_slint::Event::InkviewEvent`.
- `invoke_from_event_loop` callbacks arrive on the same channel. They run in queue order, before timers and before the full-present latch.
- `update_timers_and_animations()` runs after event dispatch. Thus Slint `changed` callbacks that input queued render in the same iteration.
- `inkview::screen::Screen` also has `full_update()`, `fast_update()` (`SoftUpdate`), `partial_update()`, `dynamic_update()`, `is_updating()`, and `wait_for_update_complete()`. Do not use `wait_for_update_complete()` (see `eink-refresh.md`).

## Scale factor

- The scale factor is `dpi / 100` from InkView: 3.0 on the PB710. The UI layout is for this value.
- Outside [1.5, 6] the backend uses 3.0.
- The backend ignores the screen scale factor of InkView on purpose. That value is the firmware UI zoom (1.16 on the PB710). It made the layout 16% larger (3.48).

## Device info

- `inkview::device::DeviceInfo::query(iv)` gives the model, hardware, and firmware strings, and the hwconfig IDs (device, platform, display geometry, color mask, EPDC, peripherals).
- Call it after `Init`. InkView loads hwconfig during init. Earlier calls read zeros.
- Unknown IDs keep their raw value.
- The PB710 device ID (49) is not in a public header. It comes from a device log (fw U710.6.11.2124).
- pbAnki logs it as `[device] …`. It shows a warning when the hwconfig DPI is not equal to `get_screen_dpi()`.
- The code does not read the serial number or the device key, on purpose.

## System panel

The system panel is the PocketBook status bar (clock, battery, Wi-Fi) at the top of the screen.

Configuration:

- `[general] system_panel = true` is the default since Nov 2026. `false` gives `PanelMode::Hidden`. Configs written before that already hold `false` and keep it.
- `PanelMode::Hidden` sets only `PANEL_NO_FB_OFFSET`. This is the upstream fix for apps drawn below the status bar.
- `PanelMode::System` calls `SetPanelType(PANEL_ENABLED | PANEL_NO_FB_OFFSET)`, `InitPanel()`, and `SetShowPanelReader(1)`.

Why `SetShowPanelReader(1)` is necessary:

- The task sets `APPLICATION_READER`. InkView gives reader apps a panel only when `show_status_bar` in `global.cfg` is on. This is the reader status bar setting. `InitPanel` loads it. The default is 1.
- When it is 0, `is_panel_active()` is false and `PanelHeight()` returns 0. The PB710 had it off.
- `SetShowPanelReader` sets the in-memory flag of InkView for this task only. It does not save the setting.

Backend behavior:

- The backend keeps `Screen::panel_height()` rows at the top free. The Slint window and the retained buffer are `width × (height - top)`. All copies and update rectangles move down by `top`.
- A full present blits only the content rows. Then it calls `DrawPanel(NULL, NULL, NULL, -1)` before its full-screen update.
- The backend does not forward pointer presses with `y < top`. InkView handles the panel and its tray. In the emulator, these taps never get to the app.
- `Show`, `Repaint` and `Foreground` read the height again. If it changed, the backend resizes and renders the full frame. The new retained frame is not complete until that render. A present waits for it, also when a `Background` came in between.

Panel height:

- `PanelHeight()` reports a height also when the panel is hidden, because any panel flag counts as active. Thus `Screen` remembers its `PanelMode` and reports 0 when hidden.
- The height follows the theme and the screen: 76 px in the SDK emulator, 123 px on the PB710.
- `Screen::panel_height()` rejects only values ≤ 0 or ≥ the screen height.
- The panel redraws itself on the InkView timer (clock, battery). If logs show these updates colliding with presents, use `SetManualPanelUpdates(1, handler)`. Then send its rectangles into the damage path.

Test status:

- SDK emulator: the bar shows, taps have the correct offset, also with `show_status_bar=0`.
- PB710: the tray opens (`control_panel_mgr.app`). The emulator has no tray. The app presents again on return (`request reason=Foreground`).
