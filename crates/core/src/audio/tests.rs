//! The audio thread against a fake sink. The sink drains in fast-forward, so
//! a second of audio plays in a few milliseconds.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::sink::{AudioSink, OpenError, QueueState, SinkOpener};
use super::*;

/// Fake-sink playback speed relative to real time.
const SPEED: f64 = 50.0;
/// Fake PCM buffer, as the PocketBook `hwout_mix`.
const CAPACITY: usize = 4096;

#[derive(Default)]
struct SinkState {
    /// Every frame written, interleaved.
    written: Vec<i16>,
    /// `written.len()` at each drop.
    drops: Vec<usize>,
    queued: usize,
    last_drain: Option<Instant>,
    /// Nothing drains the queue.
    stalled: bool,
    opened: Vec<String>,
    /// Names that fail to open, and whether the library is missing.
    missing: Vec<String>,
    no_library: bool,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<SinkState>>);

struct FakeSink(Shared);

impl FakeSink {
    fn drain(state: &mut SinkState) {
        let now = Instant::now();
        let last = state.last_drain.replace(now).unwrap_or(now);
        if state.stalled {
            return;
        }
        let played = (now - last).as_secs_f64() * f64::from(RATE) * SPEED;
        state.queued = state.queued.saturating_sub(played as usize);
    }
}

impl AudioSink for FakeSink {
    fn queue(&mut self) -> Result<QueueState, String> {
        let mut state = self.0.0.lock().unwrap();
        Self::drain(&mut state);
        Ok(QueueState {
            queued: state.queued,
            underrun: false,
        })
    }

    fn write(&mut self, samples: &[i16]) -> Result<usize, String> {
        let mut state = self.0.0.lock().unwrap();
        Self::drain(&mut state);
        let frames = (samples.len() / CHANNELS).min(CAPACITY - state.queued);
        state
            .written
            .extend_from_slice(&samples[..frames * CHANNELS]);
        state.queued += frames;
        Ok(frames)
    }

    fn drop_queue(&mut self) {
        let mut state = self.0.0.lock().unwrap();
        state.queued = 0;
        let at = state.written.len();
        state.drops.push(at);
    }
}

struct FakeOpener(Shared);

impl SinkOpener for FakeOpener {
    fn open(&mut self, device: &str) -> Result<Box<dyn AudioSink>, OpenError> {
        let mut state = self.0.0.lock().unwrap();
        state.opened.push(device.to_owned());
        if state.no_library {
            return Err(OpenError::Library("libasound.so.2: not found".into()));
        }
        if state.missing.iter().any(|name| name == device) {
            return Err(OpenError::Device(format!(
                "open '{device}': no such device"
            )));
        }
        Ok(Box::new(FakeSink(self.0.clone())))
    }
}

struct Harness {
    player: AudioPlayer,
    sink: Shared,
    events: Receiver<AudioEvent>,
}

impl Harness {
    fn new() -> Self {
        let sink = Shared::default();
        let player = AudioPlayer::with_opener(Box::new(FakeOpener(sink.clone())));
        let (tx, events) = mpsc::channel();
        let tx = Mutex::new(tx);
        player.set_listener(move |event| {
            let _ = tx.lock().unwrap().send(event);
        });
        Self {
            player,
            sink,
            events,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SinkState> {
        self.sink.0.lock().unwrap()
    }

    fn play(&self, files: &[&Path], token: u64) {
        self.play_on(files, &["hwout_mix"], token);
    }

    fn play_on(&self, files: &[&Path], devices: &[&str], token: u64) {
        self.player.play(
            files.iter().map(|path| path.to_path_buf()).collect(),
            devices.iter().map(|name| name.to_string()).collect(),
            1.0,
            token,
        );
    }

    fn next(&self, within: Duration) -> AudioEvent {
        self.events
            .recv_timeout(within)
            .unwrap_or_else(|_| panic!("no audio event within {within:?}"))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.player.shutdown();
    }
}

/// A temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pbanki-audio-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A 16-bit PCM WAV file of `frames` frames, every sample `value`.
    fn wav(&self, name: &str, rate: u32, channels: u16, frames: usize, value: i16) -> PathBuf {
        let data_len = frames * usize::from(channels) * 2;
        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for _ in 0..frames * usize::from(channels) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/audio/fixtures")
        .join(name)
}

const QUICK: Duration = Duration::from_secs(2);

#[test]
fn a_wav_plays_to_its_end_at_48_khz_stereo() {
    let dir = TempDir::new();
    // 0.5 s mono at 22.05 kHz.
    let wav = dir.wav("a.wav", 22_050, 1, 11_025, 8_000);
    let h = Harness::new();
    h.play(&[&wav], 1);
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 1 });
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 1 });

    let state = h.state();
    let frames = state.written.len() / CHANNELS;
    assert!(frames.abs_diff(24_000) <= 4, "{frames} frames");
    // Mono on both sides; the constant level survives the resampler.
    let middle = &state.written[2000..frames * 2 - 2000];
    assert!(middle.chunks(2).all(|f| f[0] == f[1]));
    assert!(
        middle.iter().all(|s| (*s - 8_000).abs() <= 2),
        "level changed"
    );
}

#[test]
fn compressed_formats_decode() {
    for (name, seconds) in [
        ("tone.mp3", 0.5),
        ("tone.ogg", 0.5),
        ("tone.m4a", 0.5),
        ("tone.flac", 0.5),
    ] {
        let h = Harness::new();
        h.play(&[&fixture(name)], 7);
        assert_eq!(h.next(QUICK), AudioEvent::Started { token: 7 }, "{name}");
        assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 7 }, "{name}");
        let frames = h.state().written.len() / CHANNELS;
        let expected = (seconds * f64::from(RATE)) as usize;
        // Encoder delay and padding add a few thousand frames at most.
        assert!(
            frames > expected * 9 / 10 && frames < expected + 4_000,
            "{name}: {frames} frames, expected about {expected}"
        );
        let peak = h.state().written.iter().map(|s| s.unsigned_abs()).max();
        assert!(peak.unwrap_or(0) > 1_000, "{name}: silent");
    }
}

#[test]
fn files_play_one_after_the_other_and_bad_files_are_skipped() {
    let dir = TempDir::new();
    let first = dir.wav("1.wav", RATE, 2, 4_800, 100);
    let second = dir.wav("2.wav", RATE, 2, 4_800, 200);
    let bad = dir.0.join("bad.mp3");
    std::fs::write(&bad, b"not audio").unwrap();
    let missing = dir.0.join("missing.wav");
    let h = Harness::new();
    h.play(&[&first, &bad, &missing, &second], 3);
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 3 });
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 3 });
    let state = h.state();
    assert_eq!(state.written.len(), 2 * 4_800 * CHANNELS);
    assert!(state.written[..9_600].iter().all(|s| *s == 100));
    assert!(state.written[9_600..].iter().all(|s| *s == 200));
}

#[test]
fn a_clip_without_audio_fails() {
    let dir = TempDir::new();
    let bad = dir.0.join("bad.ogg");
    std::fs::write(&bad, b"OggS but not really").unwrap();
    let h = Harness::new();
    h.play(&[&bad], 4);
    match h.next(QUICK) {
        AudioEvent::Failed {
            token: 4, disabled, ..
        } => assert!(!disabled),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn no_files_finish_at_once() {
    let h = Harness::new();
    h.play(&[], 5);
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 5 });
    assert!(h.state().opened.is_empty());
}

#[test]
fn play_replaces_the_clip_without_its_samples_after_the_switch() {
    let dir = TempDir::new();
    // 20 s: still decoding and playing when the second clip comes.
    let long = dir.wav("long.wav", RATE, 2, 20 * RATE as usize, 1_000);
    let short = dir.wav("short.wav", RATE, 2, 4_800, -1_000);
    let h = Harness::new();
    h.play(&[&long], 1);
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 1 });
    h.play(&[&short], 2);
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 1 });
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 2 });
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 2 });

    let state = h.state();
    let switch = state.drops[0];
    assert!(switch > 0 && switch < 20 * RATE as usize * CHANNELS);
    let after = &state.written[switch..];
    assert_eq!(after.len(), 4_800 * CHANNELS);
    assert!(
        after.iter().all(|s| *s == -1_000),
        "samples of the first clip"
    );
}

#[test]
fn stop_ends_the_clip_within_a_tick() {
    let dir = TempDir::new();
    let long = dir.wav("long.wav", RATE, 2, 20 * RATE as usize, 1_000);
    let h = Harness::new();
    h.play(&[&long], 1);
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 1 });
    let asked = Instant::now();
    h.player.stop();
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 1 });
    assert!(asked.elapsed() < Duration::from_millis(100));
    assert_eq!(h.state().drops.len(), 1);
    assert_eq!(h.state().queued, 0);
}

#[test]
fn stop_and_output_changed_before_any_play_do_nothing() {
    let h = Harness::new();
    h.player.stop();
    h.player.output_changed();
    assert!(h.player.inner.sender.get().is_none(), "thread started");
}

#[test]
fn stalls_fail_then_turn_audio_off_until_the_output_changes() {
    let dir = TempDir::new();
    let wav = dir.wav("a.wav", RATE, 2, RATE as usize, 500);
    let h = Harness::new();
    h.state().stalled = true;

    for attempt in 1..=STALLS_BEFORE_DISABLE {
        let asked = Instant::now();
        h.play(&[&wav], u64::from(attempt));
        assert_eq!(
            h.next(QUICK),
            AudioEvent::Started {
                token: u64::from(attempt)
            }
        );
        match h.next(Duration::from_secs(3)) {
            AudioEvent::Failed {
                token,
                reason,
                disabled,
            } => {
                assert_eq!(token, u64::from(attempt));
                assert!(reason.contains("stall"), "{reason}");
                assert_eq!(disabled, attempt == STALLS_BEFORE_DISABLE);
            }
            other => panic!("unexpected {other:?}"),
        }
        let took = asked.elapsed();
        assert!(took >= STALL && took < STALL * 2, "stall after {took:?}");
    }

    // Off: fails at once, without opening a PCM.
    let opened = h.state().opened.len();
    h.play(&[&wav], 10);
    match h.next(Duration::from_millis(200)) {
        AudioEvent::Failed {
            token: 10,
            disabled: true,
            ..
        } => {}
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(h.state().opened.len(), opened);

    // A new output: plays again.
    h.state().stalled = false;
    h.player.output_changed();
    h.play(&[&wav], 11);
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 11 });
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 11 });
}

#[test]
fn a_missing_pcm_falls_back_to_the_next_name_and_is_remembered() {
    let dir = TempDir::new();
    let wav = dir.wav("a.wav", RATE, 2, 4_800, 1);
    let h = Harness::new();
    h.state().missing = vec!["hwout_mix".into()];
    h.play_on(&[&wav], &["hwout_mix", "default"], 1);
    assert_eq!(h.next(QUICK), AudioEvent::Started { token: 1 });
    assert_eq!(h.next(QUICK), AudioEvent::Finished { token: 1 });
    assert_eq!(h.state().opened, vec!["hwout_mix", "default"]);
}

#[test]
fn no_pcm_opens_turns_audio_off() {
    let dir = TempDir::new();
    let wav = dir.wav("a.wav", RATE, 2, 4_800, 1);
    let h = Harness::new();
    h.state().missing = vec!["hwout_mix".into(), "default".into()];
    h.play_on(&[&wav], &["hwout_mix", "default"], 1);
    match h.next(QUICK) {
        AudioEvent::Failed {
            token: 1,
            disabled: true,
            reason,
        } => assert!(reason.contains("no PCM opens"), "{reason}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_missing_library_turns_audio_off_without_retrying() {
    let dir = TempDir::new();
    let wav = dir.wav("a.wav", RATE, 2, 4_800, 1);
    let h = Harness::new();
    h.state().no_library = true;
    for token in 1..=3 {
        h.play_on(&[&wav], &["hwout_mix", "default"], token);
        match h.next(QUICK) {
            AudioEvent::Failed { disabled: true, .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(h.state().opened, vec!["hwout_mix"]);
}

#[test]
fn volume_steps() {
    assert_eq!(volume_gain(0), 0.0);
    assert_eq!(volume_gain(10), 1.0);
    assert_eq!(volume_gain(99), 1.0);
    let half = volume_gain(9);
    assert!((half - 0.708).abs() < 0.01, "{half}");
    assert!((1..10).all(|v| volume_gain(v) < volume_gain(v + 1)));
}

/// Real ALSA on the host: `cargo test -p pbanki-core --lib alsa_default -- --ignored`.
/// Plays a quiet half-second tone on `default`.
#[test]
#[ignore = "needs a sound card"]
fn alsa_default_plays_a_fixture() {
    let player = AudioPlayer::new();
    let (tx, events) = mpsc::channel();
    let tx = Mutex::new(tx);
    player.set_listener(move |event| {
        let _ = tx.lock().unwrap().send(event);
    });
    let asked = Instant::now();
    player.play(
        vec![fixture("tone.mp3")],
        vec!["default".into()],
        volume_gain(4),
        1,
    );
    assert_eq!(
        events.recv_timeout(QUICK).unwrap(),
        AudioEvent::Started { token: 1 }
    );
    assert_eq!(
        events.recv_timeout(QUICK).unwrap(),
        AudioEvent::Finished { token: 1 }
    );
    let took = asked.elapsed();
    // Real time: the half second of audio, not much more.
    assert!(
        took > Duration::from_millis(400) && took < Duration::from_millis(1500),
        "{took:?}"
    );
    player.shutdown();
}
