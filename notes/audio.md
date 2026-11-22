# Audio

State (2026-11-22): card audio is implemented and works on the PB710 over Bluetooth (session log `logs/pbanki-20261122-181717.log`, see "Device results"). The InkView side is `inkview::audio` and `Event::{StopPlaying, AudioChanged}` in the fork (`dff31bd` on `pbanki-device-panel`).

## What InkView gives (SDK 6.10 and 6.11)

Read from the bindings in `../inkview-rs` and from `inkview.h`, `iaudio_engine.h` and the ARM libraries in the SDK archive (`target/sdk/6.10/dl`). The 6.11 header was not read, only its bindings.

- No documented file playback. The old player API (`PlayFile`, `ClosePlayer`, `SetPlayerState`, `GetPlayerState`, `GetTrackPosition`, …) is in the 5.19 and 6.8 bindings. It is not in the 6.10 / 6.11 headers, not in the SDK's ARM `libinkview.so`, and not exported on the PB710 (probe).
- Still there: `OpenPlayer()` (starts `/ebrmain/bin/music_player.app`), `TogglePlaying()`, `SetVolume` / `GetVolume`, `Set/GetAudioPlayingInfo`, `GetAudioOutput`, `get_audio_status`, `QueryHeadphone`, `configureAudioOutput*` (Bluetooth), `device_has_audio()`, `device_audio()`, the AVRCP calls.
- `device_audio()`: 0 none, 1 OSS, 3 ALSA without headphone detection, 4 ALSA, 5 ALSA loopback, 6 ALSA loopback with speaker. `GetAudioOutput()`: 0 none, 1 headphones, 2 Bluetooth, 3 speaker, 4 USB. `GetAudioOutput` is only in the 6.10+ bindings; `get_audio_status` has the same value in all of them.
- Events: `EVT_STOP_PLAYING` (263), `EVT_AUDIO_CHANGED` (265, the output changed), `EVT_AVRCP_COMMAND` (264). `EVT_MP_STATECHANGED` and `EVT_MP_TRACKCHANGED` belong to the removed player.
- Firmware apps play through `/ebrmain/lib/libaudio-engine.so` (`IAudioEngine`), a C++ interface with `std::string` and `std::function`. Not usable: the zig build uses libc++, the device libstdc++.

## What other apps do

- KOReader has no audio on PocketBook. It only declares the event constants.
- The third-party `stradichenko/audiobook.koplugin` plays through the InkView player if the firmware exports it, or through a named ALSA PCM from `/etc/asound.conf` (`tts_sm`, `hwout_mix`), not `default`.
- Its hazards, all handled here:
  - With the codec's playback switches off, nothing drains the loopback and `snd_pcm_writei` blocks forever. pbAnki opens the PCM non-blocking and treats 1 s without progress as a stall.
  - The `tts_sm` plug chain can abort inside alsa-lib (`snd_interval_empty`) for a rate it does not support. pbAnki converts to the PCM's fixed format itself and never asks for a rate or channel count outside the range the PCM reports.
  - Mixer changes that were not restored broke the speaker path on the PB700C. pbAnki never writes mixer or control values (lint rule `pbanki-no-alsa-mixer-writes`).

## Probe results (PB710, fw U710.6.11.2124, B288, Bluetooth headset connected)

`logs/audio-probe.log`, from `../inkview-rs/inkview/examples/audio_probe.rs` (not committed). `device_has_audio() = 1`, `device_audio() = 5`, `GetAudioOutput() = 2` (Bluetooth), `GetVolume() = 0`. `alsaloop` copied `hw:Loopback,1` to the headset.

| PCM | Result |
|---|---|
| `hwout_mix` (dmix → `hw:Loopback,0`) | Heard. Only 48000 Hz, 2 ch, S16_LE. Buffer 4096 frames (85 ms), period 1024. Real time. `snd_pcm_drop` under 0.1 ms. |
| `tts_sm` (softvol `tts_volume` → plug → dmix) | Real time, but not heard: its softvol follows the system volume, which was 0. Not used. |
| `default` | Stalls: nothing drains it. Never used on the device. |

- `hwout_mix` was audible with the system volume at 0, so the system volume does not reach it. The volume is a software gain in pbAnki; `SetVolume` is not called.
- `device_audio() = 5` means "loopback without speaker", yet the PB710 has an `External Speaker` mixer control. So pbAnki does not decide "speaker or not" from this value.

## Design

### Decode and sink (`crates/core/src/audio/`)

- `AudioPlayer` (`mod.rs`) is a cloneable handle to the `pbanki-audio` thread. Its thread starts on the first `play`, so a device that never plays never starts it. `stop` and `output_changed` are safe from any thread (the InkView event handler calls them).
- Commands: `play(files, devices, gain, token)` replaces whatever plays; `stop` drops the queue; `output_changed` turns audio on again after failures. The thread reports `AudioEvent::{Started, Finished, Failed { disabled }}` with the token.
- `decode.rs`: symphonia 0.6 (`mp3`, `aac`, `isomp4`, `ogg`, `vorbis`, `wav`, `pcm`, `flac`, ID3 tags). Mono goes to both sides; of more channels the first two are kept. A Catmull-Rom resampler converts to 48 kHz (no extra crate; enough for speech). The gain is applied, then the samples become i16. A file that fails to open or decode is skipped; the next one plays.
- `sink.rs`: `libasound.so.2` through `libloading`. Opened with `SND_PCM_NONBLOCK`; S16_LE, 2 ch, 48000 Hz exactly, after checking the reported ranges. `EPIPE` (underrun) → `snd_pcm_prepare`. `drop_queue` is `snd_pcm_drop` + `snd_pcm_prepare`.
- Thread loop: about 1 s decoded ahead in memory; at most 200 ms queued in the PCM (a large desktop buffer then behaves like the device); it wakes every 10 ms. A clip has ended when the PCM queue is empty, or 250 ms after it should have drained (some PCMs never report 0). The PCM closes after 2 s idle.
- Failures:
  - No `libasound.so.2`, or no PCM name opens: audio is off for the session (`Failed { disabled: true }`), logged once.
  - A PCM name that does not open: the next name. The name that worked is tried first next time.
  - Stall (the PCM took nothing and played nothing for 1 s): the clip fails and the PCM is closed. After 2 stalls in a row audio is off until `output_changed`. Then cycling cards does not cost 1 s per card.
- Volume: `audio_volume` 0..10 → `volume_gain`: silent at 0, 3 dB steps, full scale at 10. Default 5 (−15 dB).

### Devices without a usable output (`crates/pocketbook/src/main.rs`, `audio_setup`)

Checked once at start, from `inkview::audio`:

| State | What pbAnki does |
|---|---|
| `device_has_audio()` not 1, `device_audio()` missing, 0 (none) or 1 (OSS) | `FrontendHooks::audio = None`: no audio thread, no `libasound`, no replay button, no audio settings. |
| Loopback stack (5, 6) | PCM `hwout_mix`. Before each clip `GetAudioOutput()` is read on the UI thread; 0 (no output, e.g. a Bluetooth-only device without a headset) → nothing is opened, the study top bar shows "No audio output". |
| Plain ALSA (3, 4), or a value this build does not know | `hwout_mix`, then `default`. The stall check is the guard. |
| Desktop | `default`. Without a sound card the first clip shows "Audio unavailable". |

`audio_device` in the config replaces the device list (config file only).

- `device_has_audio` returns a C++ `bool`. On the PB710 firmware only the low byte of `r0` is defined. Through the bindings' `-> bool` this was undefined behavior: the first device build read it as false and crashed with a bus error right after. `inkview::audio::has_audio` calls it as returning `c_int` and tests `& 0xff` (as the probe did). `DeviceInfo`'s `device_has_*` calls still use `-> bool` and have not shown it.
- `get_audio_status` receives 36 bytes in the SDK libraries (6.10 ARM and x86, 6.11 x86). `audio_status` still passes a 1 KB buffer, in case a firmware sends more.

InkView events (handled in the `iv_main` closure, before the event goes to the backend): `StopPlaying`, `Hide`, `Background` and `Exit` stop the sound. `AudioChanged` logs the new output and calls `output_changed`.

### Study screen (`crates/core/src/frontend/audio.rs`)

- A card load stops the sound, then plays the question's sounds if autoplay is on (deck option "Don't play audio automatically" off) and audio is enabled.
- Show answer (button or key) plays the answer's own sounds. `card_documents` renders with `partial_render = true` and fills `{{FrontSide}}` with the question after AV extraction, as Anki desktop does, so the answer list does not repeat the question's sounds. A template that names the same field on both sides (the Yap deck) does play it again, as in Anki.
- The replay button (speaker icon in the study top bar, only if the side has sounds and audio is on) plays the shown side; on the answer side the question's sounds first unless "Skip question when replaying answer" is on. While a clip plays the icon is a stop square, and a tap stops. The icon changes only on `Started` and `Finished`: one e-ink update each.
- Stop on: rating, leaving the study screen, deck entry, opening Settings, a sync start, and the InkView events above.
- The top bar line: "No audio output", "Audio unavailable" (off for the session), "Sound could not play" (one clip). It is cleared on the next card.
- Sound file names must be plain names in the media folder; anything with a separator or `..` is skipped. TTS tags are logged and skipped.
- Settings > Device, only if the device has audio: "Play card sounds" (`audio_enabled`) and "Volume" (`audio_volume`, a stepper that does not wrap). A change applies to the next clip.

## Tests

- `cargo test -p pbanki-core --lib audio`: the thread against a fake sink that drains 50× faster than real time. WAV made in the test and the fixtures in `src/audio/fixtures/` (mp3 22.05 kHz mono with ID3v2, ogg 44.1 kHz stereo, m4a 24 kHz, flac 16 kHz). Play order and skipped files, `Play` during `Play` (no samples of the first clip after the drop), `Stop` within 100 ms, stalls → off → `output_changed`, PCM fallback, missing library, the resampler.
- `cargo test -p pbanki-core --lib alsa_default -- --ignored`: a half-second tone on the host's real ALSA `default`.
- `study::tests::answer_audio_excludes_the_front_side`: the FrontSide fix.
- Desktop check (2026-11-22, `yap-jpn-eng.apkg` in a temporary collection): autoplay on deck entry, the stop icon while playing, the answer's sound on Show answer, rating during a replay stops it and the next card's sound starts 8 ms later, volume stops at 10, audio off hides the button and plays nothing.

## Device results (PB710, 2026-11-22, Bluetooth headset)

- `has_audio=true stack=ALSA loopback (5) output=Bluetooth (2)`; PCM `hwout_mix`.
- About 40 clips over 5 minutes of study: each starts 33–119 ms after the request (PCM open included), with no underrun, stall or failure, also while cards render. The PCM closes 2 s after each clip and opens again for the next.
- Rating or Show answer during a clip replaces it (no `closed` line between the clips).
- Headset off: `output changed: none (0)`, and the next card logs `no audio output; not playing`. Headset on again: `output changed: Bluetooth (2)`, and sound plays without a restart.
- With the headset off the output is `none`: the PB710 has no built-in speaker (`device_audio() = 5`, loopback without speaker).
- The Home key during a clip stops it (`Hide` / `Background`).
- Volume: the default was lowered from 6 to 5 after listening on the device.

## Open device checks

- A model with a speaker (`device_audio() = 6`): whether `GetAudioOutput()` then reports `speaker (3)` and the sound plays. The PB710 has no built-in speaker (confirmed by the user), so `none` without a headset is correct there.
- `[audio] clip N: K underrun(s)` lines on longer sessions and other decks. None so far.
- The binary size change from symphonia.
