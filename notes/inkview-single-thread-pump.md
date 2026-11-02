# Single-threaded InkView pump (design, not implemented)

This design is the structural fix for the cross-thread hazard (see "Threads and the cross-thread wake" in `inkview-backend.md`). Use it if update stalls come back, or if the wake stops working on new firmware.

## Changes

- `inkview`: add the safe wrappers `prepare_for_loop(iv, handler)` and `process_event_loop_quick(iv)`. `PrepareForLoop` and `ProcessEventLoopQuick` are already in the bindings (`bindings_6_10.rs`, `bindings_6_11.rs`). Forward the handler as `iv_main` does.
- `inkview-slint`: `Backend::run_event_loop` calls `ProcessEventLoopQuick()` on each iteration. While idle, it sleeps for a maximum of 20–50 ms, not in `recv_timeout`. It reads the events that the handler queued on the same thread. The `Presenter`, the damage path, and the logs do not change. `wake_event_loop()` is then not necessary.
- `crates/pocketbook/src/main.rs`: remove the `pbanki-ui` thread and `iv_main`. Call `prepare_for_loop` and run Slint on the main thread. Then all InkView calls run on one thread.

## Risks to check on the device

- `keyboard::open` (`OpenKeyboardEx`, with a callback) and `network::ensure_connected` (the `NetConnect(NULL)` dialog) now run on the UI thread, and `InkViewMain` services them on the main thread. Both must still work with the pump.
- The sync blocks the UI thread. With the pump, it also stops InkView event processing while it runs.

## Release

1. Ship the pump as a separate build.
2. Do the launch, resume, and study-flow device tests again against the current backend.
