# Performance

## Profiling

Profiling is opt-in through the `profile` cargo feature. `pbanki-core` defines it. The `pbanki` and `pbanki-desktop` manifests forward it.

- Device: `make release-profile` or `make release-rk3566-profile`.
- Desktop: `cd crates/desktop && cargo run --features profile`.
- Without the feature, `profile_log!` and `ProfileTimer` (`crates/core/src/profile.rs`) compile to nothing. Release binaries contain no `[profile]` strings.

With the feature:

- `crates/core/src/study.rs` splits card loading into deck selection, queue, state descriptions, render, text processing, font choice, pagination, and answer write.
- `crates/core/src/fonts.rs` logs the catalog, coverage, and registration times.
- `app::start` logs the startup times.
- `spawn_font_probe` starts 10 s after launch at nice 19. It logs:
  - The read time and size of each registered card font (Slint reads them fully at startup): `[profile] font '…' … read=… bytes=…`.
  - The font that `Monospace` resolves to in the shared collection of Slint, in Slint's order (named family, sans-serif, system-ui): `[profile] UI family '…' resolves to …`.
- Logs go to `pbanki.log` on the eMMC. Thus the times include the logging overhead.

## PB710 baselines

Measured with `make release`, Oct 2026.

- First frame: 300–500 ms after backend start.
- A full-screen `PartialUpdate` keeps the panel busy for 550–650 ms. This is the minimum time for an answer or the next card. To go lower, use smaller damage or `DynamicUpdate` for card changes. That is a ghosting and quality tradeoff to test on the device.
- Card jobs: 10–30 ms.
- First deck entry: about 200 ms. About 135 ms of it is `set_current_deck`.
- `answer_card`: about 140 ms (FAT I/O). Do not make SQLite less durable to hide it.
- With the old Slint pin, the first parse of each font by fontdue (vendored lazy patch) took: Ryo 48 ms, Adobe Ming 126 ms, Adobe Heiti 177 ms, Noto CJK 249 ms. The UI family `Monospace` resolved to Roboto.

These values are not measured again for Slint 1.18 (swash rasterization, fontconfig through dlopen).

## Waveform probe on the PB710

Firmware U710.6.11.2124, band of 1264×280, Nov 2026.

`GetWaveformTimes` (ms, by waveform ID): `[11, 152, 329, 152, 117, 329, 235, 211, 152, 0, 0, 0, 0, 623, 329, 0]`.

Busy time (`IsUpdateInProcess`):

| Mode | Busy (ms) |
|---|---|
| `full`, `full-hq` | ~630 |
| `soft` | 463 |
| `soft-hq` | 656 |
| `partial`, `partial-bw`, `partial-du4`, `dynamic`, `dynamic-bw` | ~443 |
| `partial-hq` | 610 |
| `ex:` init, du, gc16, gc4, du4, glr16, gln16 | ~440 |
| `ex:a2` | 406 |
| `ex:a2in` | 524 |
| `ex:a2out` | 506 |
| `ex:gl16`, `ex:gcc16` | 619 |

Results:

- `UpdateEx` passes the waveform bits to the panel. `ex:gl16` is about equal to `partial-hq`, and `ex:a2` is the fastest.
- Busy time does not follow `GetWaveformTimes`. `du` is 152 ms in the table, but the panel is busy for about 440 ms.
- About 440 ms seems to be the floor for this band size. Faster waveforms give little latency gain here.
- Ghosting and quality differences still need a visual check.
