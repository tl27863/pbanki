//! Card audio: `[sound:…]` files decoded in Rust and written to ALSA.
//!
//! One thread, `pbanki-audio`, owns the decoder and the PCM. The UI thread
//! sends it commands through `AudioPlayer`; a new `Play` replaces whatever
//! plays, and `Stop` discards the queue at once (`snd_pcm_drop`). The thread
//! reports back through a listener (`AudioEvent`), which the frontend turns
//! into UI updates on the event loop.
//!
//! The device PCM (`hwout_mix` on the PocketBook) takes only 48 kHz stereo
//! S16 and holds 85 ms, so the thread keeps about 1 s decoded ahead in memory
//! and wakes every 10 ms to top up the PCM. A PCM that takes nothing for 1 s
//! is a stall (nothing drains it): the clip fails, and after
//! `STALLS_BEFORE_DISABLE` stalls in a row audio is off until the output
//! changes (`AudioPlayer::output_changed`). See notes/audio.md.

mod decode;
mod sink;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use decode::ClipDecoder;
use sink::{AlsaOpener, AudioSink, OpenError, SinkOpener};

/// The sink format. The PocketBook `hwout_mix` dmix accepts nothing else.
pub(crate) const RATE: u32 = 48_000;
pub(crate) const CHANNELS: usize = 2;

/// Decoded audio kept ahead of the PCM.
const AHEAD_FRAMES: usize = RATE as usize;
/// The most audio queued in the PCM. A drop discards it at once anyway; this
/// only bounds a large-buffer PCM (the desktop) to the same behavior.
const LEAD_FRAMES: usize = RATE as usize / 5;
/// How often the thread tops up the PCM while playing. The device PCM holds
/// 85 ms (4096 frames, periods of 1024).
const TICK: Duration = Duration::from_millis(10);
/// No frame taken and no frame played for this long: nothing drains the PCM.
const STALL: Duration = Duration::from_secs(1);
/// Stalls in a row before audio is off for the session.
const STALLS_BEFORE_DISABLE: u32 = 2;
/// The PCM is closed after this long without playing.
const IDLE_CLOSE: Duration = Duration::from_secs(2);
/// Some PCMs never report an empty queue. A clip has ended this long after
/// the queue should have drained.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// What the audio thread reports. `token` is the one of the `Play` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    /// The first file of the clip started.
    Started { token: u64 },
    /// The clip played to its end, or was stopped or replaced.
    Finished { token: u64 },
    /// The clip could not play. `disabled` is true when audio is off for the
    /// session (no library, no PCM opens, or repeated stalls) until the output
    /// changes.
    Failed {
        token: u64,
        reason: String,
        disabled: bool,
    },
}

type Listener = Arc<dyn Fn(AudioEvent) + Send + Sync>;

enum Command {
    Play {
        files: Vec<PathBuf>,
        devices: Vec<String>,
        gain: f32,
        token: u64,
    },
    Stop,
    OutputChanged,
    #[cfg(test)]
    Shutdown,
}

/// Linear gain for the volume setting 0..=10: silent at 0, then 3 dB steps up
/// to full scale at 10.
pub fn volume_gain(volume: u32) -> f32 {
    match volume.min(10) {
        0 => 0.0,
        v => 10f32.powf(-3.0 * (10 - v) as f32 / 20.0),
    }
}

/// A handle to the audio thread. Cloning is cheap; all clones drive the same
/// thread. The thread starts on the first `play`, so a device that never
/// plays never starts it.
#[derive(Clone)]
pub struct AudioPlayer {
    inner: Arc<Inner>,
}

struct Inner {
    sender: OnceLock<Option<Mutex<Sender<Command>>>>,
    listener: Mutex<Option<Listener>>,
    opener: Mutex<Option<Box<dyn SinkOpener>>>,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPlayer {
    /// A player that writes to ALSA (`libasound.so.2`).
    pub fn new() -> Self {
        Self::with_opener(Box::new(AlsaOpener::default()))
    }

    fn with_opener(opener: Box<dyn SinkOpener>) -> Self {
        Self {
            inner: Arc::new(Inner {
                sender: OnceLock::new(),
                listener: Mutex::new(None),
                opener: Mutex::new(Some(opener)),
            }),
        }
    }

    /// Sets the receiver of `AudioEvent`s. It runs on the audio thread.
    pub fn set_listener(&self, listener: impl Fn(AudioEvent) + Send + Sync + 'static) {
        *self.inner.listener.lock().unwrap() = Some(Arc::new(listener));
    }

    /// Plays `files` one after the other on the first PCM of `devices` that
    /// opens, replacing whatever plays.
    pub fn play(&self, files: Vec<PathBuf>, devices: Vec<String>, gain: f32, token: u64) {
        self.send(
            Command::Play {
                files,
                devices,
                gain,
                token,
            },
            true,
        );
    }

    /// Stops at once. Safe from any thread, and a no-op before the first
    /// `play`.
    pub fn stop(&self) {
        self.send(Command::Stop, false);
    }

    /// The audio output changed (InkView `EVT_AUDIO_CHANGED`): audio that was
    /// off after failures may work again.
    pub fn output_changed(&self) {
        self.send(Command::OutputChanged, false);
    }

    fn send(&self, command: Command, start: bool) {
        let sender = if start {
            self.inner.sender.get_or_init(|| self.spawn())
        } else {
            match self.inner.sender.get() {
                Some(sender) => sender,
                None => return,
            }
        };
        if let Some(sender) = sender {
            let _ = sender.lock().unwrap().send(command);
        }
    }

    fn spawn(&self) -> Option<Mutex<Sender<Command>>> {
        let opener = self.inner.opener.lock().unwrap().take()?;
        let (sender, receiver) = mpsc::channel();
        let inner = self.inner.clone();
        let emit: Listener = Arc::new(move |event| {
            let listener = inner.listener.lock().unwrap().clone();
            if let Some(listener) = listener {
                listener(event);
            }
        });
        let spawned = std::thread::Builder::new()
            .name("pbanki-audio".into())
            .spawn(move || Worker::new(opener, emit).run(receiver));
        match spawned {
            Ok(_) => Some(Mutex::new(sender)),
            Err(err) => {
                tracing::error!("[audio] could not start the audio thread: {err}");
                None
            }
        }
    }

    #[cfg(test)]
    fn shutdown(&self) {
        self.send(Command::Shutdown, false);
    }
}

/// The clip being played.
struct Clip {
    token: u64,
    files: VecDeque<PathBuf>,
    gain: f32,
    decoder: Option<ClipDecoder>,
    /// Decoded interleaved samples not yet written.
    pending: VecDeque<i16>,
    /// All files are decoded.
    decoded: bool,
    started: bool,
    /// When the PCM last took frames or its queue shrank.
    progress: Instant,
    last_queued: usize,
    /// When the last frames were written, and how many were queued then.
    last_write: Option<(Instant, usize)>,
    underruns: u32,
    opened_at: Instant,
}

struct Worker {
    opener: Box<dyn SinkOpener>,
    emit: Listener,
    sink: Option<Box<dyn AudioSink>>,
    /// The PCM name that opened, kept for the session.
    device: Option<String>,
    clip: Option<Clip>,
    idle_since: Instant,
    stalls: u32,
    /// Why audio is off for the session, until the output changes.
    disabled: Option<String>,
}

impl Worker {
    fn new(opener: Box<dyn SinkOpener>, emit: Listener) -> Self {
        Self {
            opener,
            emit,
            sink: None,
            device: None,
            clip: None,
            idle_since: Instant::now(),
            stalls: 0,
            disabled: None,
        }
    }

    fn run(mut self, receiver: Receiver<Command>) {
        loop {
            let command = if self.clip.is_some() {
                receiver.recv_timeout(TICK)
            } else if self.sink.is_some() {
                let left = IDLE_CLOSE.saturating_sub(self.idle_since.elapsed());
                receiver.recv_timeout(left.max(TICK))
            } else {
                receiver.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            match command {
                Ok(command) => {
                    if !self.handle(command) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.clip.is_some() {
                self.pump();
            } else if self.sink.is_some() && self.idle_since.elapsed() >= IDLE_CLOSE {
                self.sink = None;
            }
        }
        self.end_clip(None);
    }

    /// False to end the thread.
    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Play {
                files,
                devices,
                gain,
                token,
            } => {
                self.end_clip(None);
                self.start_clip(files, devices, gain, token);
            }
            Command::Stop => self.end_clip(None),
            Command::OutputChanged => {
                self.end_clip(None);
                if let Some(reason) = self.disabled.take() {
                    tracing::info!("[audio] output changed; audio on again (was off: {reason})");
                }
                self.stalls = 0;
                // The device behind the PCM may have changed.
                self.sink = None;
                self.device = None;
            }
            #[cfg(test)]
            Command::Shutdown => return false,
        }
        true
    }

    fn start_clip(&mut self, files: Vec<PathBuf>, devices: Vec<String>, gain: f32, token: u64) {
        if let Some(reason) = &self.disabled {
            (self.emit)(AudioEvent::Failed {
                token,
                reason: reason.clone(),
                disabled: true,
            });
            return;
        }
        if files.is_empty() {
            (self.emit)(AudioEvent::Finished { token });
            return;
        }
        if let Err(reason) = self.ensure_sink(&devices) {
            (self.emit)(AudioEvent::Failed {
                token,
                reason,
                disabled: self.disabled.is_some(),
            });
            return;
        }
        let now = Instant::now();
        self.clip = Some(Clip {
            token,
            files: files.into(),
            gain,
            decoder: None,
            pending: VecDeque::with_capacity(AHEAD_FRAMES * CHANNELS),
            decoded: false,
            started: false,
            progress: now,
            last_queued: 0,
            last_write: None,
            underruns: 0,
            opened_at: now,
        });
        self.pump();
    }

    /// Opens the first PCM of `devices` that works, unless one is open.
    fn ensure_sink(&mut self, devices: &[String]) -> Result<(), String> {
        if self.sink.is_some() {
            return Ok(());
        }
        // The name that worked before goes first.
        let mut names: Vec<&String> = Vec::new();
        if let Some(known) = self.device.as_ref().filter(|d| devices.contains(d)) {
            names.push(known);
        }
        names.extend(devices.iter().filter(|d| Some(*d) != self.device.as_ref()));
        let mut errors = Vec::new();
        for name in names {
            match self.opener.open(name) {
                Ok(sink) => {
                    self.sink = Some(sink);
                    self.device = Some(name.clone());
                    return Ok(());
                }
                Err(OpenError::Library(reason)) => {
                    tracing::warn!("[audio] off for this session: {reason}");
                    self.disabled = Some(reason.clone());
                    return Err(reason);
                }
                Err(OpenError::Device(reason)) => {
                    tracing::warn!("[audio] {reason}");
                    errors.push(reason);
                }
            }
        }
        let reason = if errors.is_empty() {
            "no audio device configured".to_string()
        } else {
            format!("no PCM opens: {}", errors.join("; "))
        };
        tracing::warn!("[audio] off for this session: {reason}");
        self.disabled = Some(reason.clone());
        Err(reason)
    }

    /// Ends the clip: discards what is queued and reports it. `failure` is
    /// `(reason, stalled)`.
    fn end_clip(&mut self, failure: Option<(String, bool)>) {
        let Some(clip) = self.clip.take() else {
            return;
        };
        self.idle_since = Instant::now();
        if let Some(sink) = self.sink.as_mut() {
            sink.drop_queue();
        }
        if clip.underruns > 0 {
            tracing::warn!(
                "[audio] clip {}: {} underrun(s)",
                clip.token,
                clip.underruns
            );
        }
        match failure {
            None => {
                self.stalls = 0;
                (self.emit)(AudioEvent::Finished { token: clip.token });
            }
            Some((reason, stalled)) => {
                if stalled {
                    self.stalls += 1;
                    // The PCM may be wedged; open it again next time.
                    self.sink = None;
                    if self.stalls >= STALLS_BEFORE_DISABLE {
                        let off = format!("{} stalls in a row: {reason}", self.stalls);
                        tracing::warn!("[audio] off until the output changes: {off}");
                        self.disabled = Some(off);
                    }
                }
                tracing::warn!("[audio] clip {} failed: {reason}", clip.token);
                (self.emit)(AudioEvent::Failed {
                    token: clip.token,
                    reason,
                    disabled: self.disabled.is_some(),
                });
            }
        }
    }

    /// Decodes ahead and tops up the PCM. Ends the clip when it has played,
    /// failed, or stalled.
    fn pump(&mut self) {
        match self.try_pump() {
            Ok(false) => {}
            Ok(true) => self.end_clip(None),
            Err(failure) => self.end_clip(Some(failure)),
        }
    }

    /// True when the clip has played to its end.
    fn try_pump(&mut self) -> Result<bool, (String, bool)> {
        let Some(clip) = self.clip.as_mut() else {
            return Ok(false);
        };
        let Some(sink) = self.sink.as_mut() else {
            return Err(("no PCM".into(), false));
        };

        fill(clip);

        let state = sink.queue().map_err(|err| (err, false))?;
        if state.underrun && clip.started && !clip.pending.is_empty() {
            clip.underruns += 1;
        }
        let mut queued = state.queued;
        if queued < clip.last_queued {
            clip.progress = Instant::now();
        }

        while queued < LEAD_FRAMES && !clip.pending.is_empty() {
            let room = (LEAD_FRAMES - queued) * CHANNELS;
            let (front, _) = clip.pending.as_slices();
            let take = front.len().min(room);
            let written = sink.write(&front[..take]).map_err(|err| (err, false))?;
            if written == 0 {
                break;
            }
            clip.pending.drain(..written * CHANNELS);
            queued += written;
            let now = Instant::now();
            clip.progress = now;
            clip.last_write = Some((now, queued));
            if !clip.started {
                clip.started = true;
                tracing::info!(
                    "[audio] clip {} started after {} ms",
                    clip.token,
                    clip.opened_at.elapsed().as_millis()
                );
                (self.emit)(AudioEvent::Started { token: clip.token });
            }
            fill(clip);
        }
        clip.last_queued = queued;

        if clip.decoded && clip.pending.is_empty() {
            if !clip.started {
                // Nothing decodable in any file.
                return Err(("no audio decoded".into(), false));
            }
            let drained = queued == 0
                || clip.last_write.is_some_and(|(at, queued_then)| {
                    at.elapsed() > frames_duration(queued_then) + DRAIN_GRACE
                });
            if drained {
                return Ok(true);
            }
        }

        if clip.progress.elapsed() > STALL && (queued > 0 || !clip.pending.is_empty()) {
            return Err((
                format!(
                    "stall: the PCM took nothing for {} ms (nothing drains it)",
                    STALL.as_millis()
                ),
                true,
            ));
        }
        Ok(false)
    }
}

fn frames_duration(frames: usize) -> Duration {
    Duration::from_secs_f64(frames as f64 / f64::from(RATE))
}

/// Decodes until `AHEAD_FRAMES` are pending or all files are decoded. A file
/// that fails to open or decode is skipped.
fn fill(clip: &mut Clip) {
    while !clip.decoded && clip.pending.len() < AHEAD_FRAMES * CHANNELS {
        if clip.decoder.is_none() {
            let Some(path) = clip.files.pop_front() else {
                clip.decoded = true;
                break;
            };
            let started = Instant::now();
            match ClipDecoder::open(&path, clip.gain) {
                Ok(decoder) => {
                    tracing::info!(
                        "[audio] clip {}: {} (open {} ms)",
                        clip.token,
                        path.display(),
                        started.elapsed().as_millis()
                    );
                    clip.decoder = Some(decoder);
                }
                Err(err) => {
                    tracing::warn!("[audio] skipped {}: {err}", path.display());
                    continue;
                }
            }
        }
        let Some(decoder) = clip.decoder.as_mut() else {
            continue;
        };
        match decoder.decode_into(&mut clip.pending) {
            Ok(true) => {}
            Ok(false) => clip.decoder = None,
            Err(err) => {
                tracing::warn!("[audio] clip {}: {err}; next file", clip.token);
                clip.decoder = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
