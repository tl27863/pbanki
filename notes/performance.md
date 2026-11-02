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
- Logs go to the session log in `pbanki/logs/` on the eMMC. Thus the times include the logging overhead.

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

## Float ABI of the PB710 build

- `armv7-unknown-linux-gnueabi` emulates all float math in software. Each `f32`/`f64` operation is a call (`__aeabi_fmul`, `__aeabi_fadd`, …).
- `pbanki.bin` (`make release`) has no hardware float instructions (`llvm-objdump -d` finds 0 `vmul`/`vadd`/`vmla` `.f32`/`.f64`).
- The PB710 CPU has the hardware (`/proc/cpuinfo`: `vfpv4 neon vfpd32`, CPU part `0xc07` = Cortex-A7).
- `RUSTFLAGS="-C target-feature=+vfp4,+neon,-soft-float"` gives "softfp": hardware VFP/NEON instructions, with the soft-float calling convention (arguments and results in core registers). The ELF stays `soft-float ABI`, so it links and runs with the soft-float system libraries. rustc 1.99.1 warns that these features are unstable.
- The precompiled `std` stays software float. Only our crates and dependencies change.
- Not tried for `pbanki.bin` yet. It can make layout and text in Slint faster. Test on the device before release: launch, study flow, sync.

## Blitz render bench (PB710)

`bench/blitz-render/` (see `build.md`), 2026-11-02. PB710: `sun8iw10`, 2 cores, 1008 MHz, governor `interactive`. Scale 3.0, 10 iterations plus 2 warm-up. Medians in ms.

Plain `gnueabi` build (software float), `1264x1557`:

| Fixture | `next_card_total` (new card) | `frame_total` (show answer) | back `paint_record` | back `raster` | `convert_diff` |
|---|---|---|---|---|---|
| `ja-vocab` | 168 | 388 | 265 | 52 | 65 |
| `long-text` | 248 | 1088 | 931 | 82 | 65 |
| `styled` | 251 | 626 | 478 | 70 | 65 |

- `1264x1680` is 1–10% slower.
- `parse` is 2–3 ms. `resolve_cold` is 2–6 ms. `resolve_change` is 5–14 ms. Style and layout are not the problem.
- `paint_record` is the largest part, and it grows with the number of glyphs. vello_cpu flattens paths and makes strips while the scene is recorded. The `mt` build moves this work into `raster` (ja-vocab back: record 7.5 ms, raster 319 ms).
- `raster` (flush and fine rasterization) is 40–90 ms.
- `convert_diff` (RGBA to gray plus diff, integer only) is 65–74 ms. This loop can be made faster.
- `process_first` (first frame of the process): 212 ms.
- `VmHWM`: 4.5 MB after the fonts, 21 MB after the first frame, 25 MB at the end.
- Font read: Roboto (`/ebrmain/fonts/Roboto-Regular.ttf`) 4 ms, Ryo Gothic 22 ms.
- The `mt` build (vello_cpu thread pool) is slower: ja-vocab `frame_total` 401 ms, long-text 5.9 s. Do not use it on 2 cores. The log of its second run stops in long-text (the run was cut off).

Slint for comparison (same software-float build): full-screen card renders 26–51 ms.

Second run, same session, plain and softfp (`make blitz-bench-softfp`) builds. The plain build repeated the first run within 1%. Its run was cut off in `long-text 1264x1680` (no `done` line, one 6.1 s outlier).

softfp build, `1264x1557`:

| Fixture | `next_card_total` | `frame_total` | back `paint_record` | back `raster` | `convert_diff` |
|---|---|---|---|---|---|
| `ja-vocab` | 111 | 139 | 25 | 42 | 67 |
| `long-text` | 119 | 214 | 82 | 58 | 65 |
| `styled` | 128 | 179 | 52 | 51 | 65 |

- `paint_record` is about 10× faster (931 → 82 ms for long-text). Path flattening was mostly emulated float math.
- `raster` is 10–30% faster. `resolve` changes little.
- `convert_diff` (65–70 ms) did not change. It is now the largest part. It is bench code, not Blitz: a per-row compare of the RGBA frames, then conversion only of the changed rows, should cut most of it.
- `process_first`: 137 ms. `VmHWM`: 24.5 MB (same as plain).
- `1264x1680`: `frame_total` 144 / 232 / 193 ms.
- Launcher order: the glob runs `blitz-bench-softfp.bin` before `blitz-bench.bin`.

Third run, softfp only, with a one-pass `convert_diff` and a `multi-screen` fixture (about five screens of text on the back). `convert_diff` now converts each row to gray, compares it with the retained gray row, and copies the changed span. Both loops are NEON code; see the comments in `bench/blitz-render/src/main.rs`. The RGBA frame is read once, and no previous RGBA frame is kept.

softfp build, `1264x1557`:

| Fixture | `next_card_total` | `frame_total` | back `paint_record` | back `raster` | `convert_diff` |
|---|---|---|---|---|---|
| `ja-vocab` | 83 | 105 | 27 | 43 | 30 |
| `long-text` | 90 | 180 | 82 | 58 | 31 |
| `styled` | 98 | 147 | 52 | 51 | 33 |
| `multi-screen` | 92 | 195 | 75 | 56 | 31 |

- `convert_diff` fell from 52–70 ms to 29–33 ms. It reads 7.9 MB RGBA and 2 MB gray per frame, about 330 MB/s. This is the memory bandwidth limit, not the math. `raster` of a near-empty frame (36 ms) is the same limit for the 7.9 MB write.
- The old version compared two RGBA frames with `==` on slices. On the PB710 that is the generic glibc memcmp over 16 MB: 52–70 ms. On the desktop it was 0.2 ms, so desktop numbers do not show this.
- `multi-screen`: `paint_record` and `raster` are the same as `long-text`, so `blitz-paint` culls off-screen content. `resolve_change` is 33 ms (long-text 8 ms), because layout covers the full document.
- `process_first`: 107 ms. `VmHWM`: 21.8 MB at the end of `1264x1557` (was 37 MB with three RGBA buffers).
- In this run, `1264x1680` is not usable. In its first fixture all phases, also the integer-only `convert_diff`, became 3–5× slower and stayed slow. The plain build of the second run showed the same. From this run on, every `==` line logs the clock and the SoC temperature.

Fourth run, same build plus the clock log. No slowdown. The clock stayed at 1008 MHz (max 1008 MHz), and the temperature at 39–41 °C. `1264x1557` repeated the third run within 1%.

| `1264x1680` | `next_card_total` | `frame_total` | `convert_diff` |
|---|---|---|---|
| `ja-vocab` | 85 | 105 | 32 |
| `long-text` | 94 | 195 | 34 |
| `styled` | 102 | 159 | 35 |
| `multi-screen` | 97 | 206 | 33 |

- The cause of the slowdowns is still unknown. The clock is read once after each card, so a short drop can go unseen. Heat is not the cause at 40 °C. Another possibility: firmware work after the USB copy (a library scan, for example) that competes for the CPU and the memory bus.
