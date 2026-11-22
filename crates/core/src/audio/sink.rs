//! Where decoded samples go: an ALSA PCM, or a recording sink in tests.
//!
//! `libasound.so.2` is loaded at run time, as the InkView bindings load
//! `libinkview.so`. If it is missing, audio is unavailable; the app still
//! runs. The PCM is opened non-blocking and never asked for a rate or channel
//! count outside the range it reports (notes/audio.md: the PocketBook plug
//! chains can abort inside alsa-lib). pbAnki never writes mixer controls
//! (`lint/rules/no-alsa-mixer-writes.yml`).

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::Arc;

use libloading::Library;

use super::{CHANNELS, RATE};

/// Why a PCM could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenError {
    /// No audio library: no PCM can open in this session.
    Library(String),
    /// This PCM does not exist or does not take 48 kHz stereo S16. Another
    /// name may work.
    Device(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Library(reason) | OpenError::Device(reason) => f.write_str(reason),
        }
    }
}

/// The state of the queue of an open sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueState {
    /// Frames written but not played yet.
    pub queued: usize,
    /// The sink ran dry since the last call and was made ready again.
    pub underrun: bool,
}

/// An open output for interleaved stereo S16 frames at `RATE`.
pub(crate) trait AudioSink {
    /// The queue, without blocking.
    fn queue(&mut self) -> Result<QueueState, String>;
    /// Writes as many frames as fit without blocking. Returns the number of
    /// frames taken; 0 when the queue is full.
    fn write(&mut self, samples: &[i16]) -> Result<usize, String>;
    /// Discards everything queued at once and makes the sink ready for new
    /// audio.
    fn drop_queue(&mut self);
}

/// Opens sinks by PCM name.
pub(crate) trait SinkOpener: Send {
    fn open(&mut self, device: &str) -> Result<Box<dyn AudioSink>, OpenError>;
}

// ---------------------------------------------------------------------------
// ALSA
// ---------------------------------------------------------------------------

type Pcm = *mut c_void;
type HwParams = *mut c_void;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_NONBLOCK: c_int = 1;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_STATE_PREPARED: c_int = 2;
const EAGAIN: c_long = 11;
const EPIPE: c_long = 32;

macro_rules! alsa_api {
    ($( $name:ident : fn($($arg:ty),*) $(-> $ret:ty)? ; )*) => {
        struct Alsa {
            _lib: Library,
            $( $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?, )*
        }

        impl Alsa {
            fn load() -> Result<Self, String> {
                // SAFETY: libasound's constructors have no preconditions, and
                // each symbol is read with the signature of `alsa/pcm.h`.
                unsafe {
                    let lib = Library::new("libasound.so.2")
                        .map_err(|err| format!("libasound.so.2: {err}"))?;
                    $(
                        let $name = *lib
                            .get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(
                                concat!(stringify!($name), "\0").as_bytes(),
                            )
                            .map_err(|err| format!("{}: {err}", stringify!($name)))?;
                    )*
                    Ok(Self { _lib: lib, $( $name, )* })
                }
            }
        }
    };
}

alsa_api! {
    snd_strerror: fn(c_int) -> *const c_char;
    snd_pcm_open: fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int;
    snd_pcm_close: fn(Pcm) -> c_int;
    snd_pcm_hw_params_malloc: fn(*mut HwParams) -> c_int;
    snd_pcm_hw_params_free: fn(HwParams);
    snd_pcm_hw_params_any: fn(Pcm, HwParams) -> c_int;
    snd_pcm_hw_params_get_rate_min: fn(HwParams, *mut c_uint, *mut c_int) -> c_int;
    snd_pcm_hw_params_get_rate_max: fn(HwParams, *mut c_uint, *mut c_int) -> c_int;
    snd_pcm_hw_params_get_channels_min: fn(HwParams, *mut c_uint) -> c_int;
    snd_pcm_hw_params_get_channels_max: fn(HwParams, *mut c_uint) -> c_int;
    snd_pcm_hw_params_test_format: fn(Pcm, HwParams, c_int) -> c_int;
    snd_pcm_hw_params_set_access: fn(Pcm, HwParams, c_int) -> c_int;
    snd_pcm_hw_params_set_format: fn(Pcm, HwParams, c_int) -> c_int;
    snd_pcm_hw_params_set_channels: fn(Pcm, HwParams, c_uint) -> c_int;
    snd_pcm_hw_params_set_rate: fn(Pcm, HwParams, c_uint, c_int) -> c_int;
    snd_pcm_hw_params: fn(Pcm, HwParams) -> c_int;
    snd_pcm_get_params: fn(Pcm, *mut c_ulong, *mut c_ulong) -> c_int;
    snd_pcm_prepare: fn(Pcm) -> c_int;
    snd_pcm_start: fn(Pcm) -> c_int;
    snd_pcm_drop: fn(Pcm) -> c_int;
    snd_pcm_writei: fn(Pcm, *const c_void, c_ulong) -> c_long;
    snd_pcm_delay: fn(Pcm, *mut c_long) -> c_int;
    snd_pcm_state: fn(Pcm) -> c_int;
}

impl Alsa {
    fn err(&self, code: c_int) -> String {
        // SAFETY: snd_strerror returns a static string or null.
        let text = unsafe { (self.snd_strerror)(code) };
        if text.is_null() {
            return format!("error {code}");
        }
        let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        format!("{text} ({code})")
    }
}

/// Opens ALSA PCMs. The library is loaded on the first `open`; if that fails,
/// every `open` fails with `OpenError::Library`.
#[derive(Default)]
pub(crate) struct AlsaOpener {
    alsa: Option<Result<Arc<Alsa>, String>>,
}

// SAFETY: `Alsa` holds a library handle and function pointers. alsa-lib's PCM
// calls are safe from any thread as long as one PCM is used by one thread at
// a time, which the audio thread guarantees.
unsafe impl Send for AlsaOpener {}

impl SinkOpener for AlsaOpener {
    fn open(&mut self, device: &str) -> Result<Box<dyn AudioSink>, OpenError> {
        let alsa = self
            .alsa
            .get_or_insert_with(|| Alsa::load().map(Arc::new))
            .clone()
            .map_err(OpenError::Library)?;
        AlsaSink::open(alsa, device).map(|sink| Box::new(sink) as Box<dyn AudioSink>)
    }
}

struct AlsaSink {
    alsa: Arc<Alsa>,
    pcm: Pcm,
    name: String,
}

impl AlsaSink {
    fn open(alsa: Arc<Alsa>, name: &str) -> Result<Self, OpenError> {
        let c_name = CString::new(name).map_err(|_| OpenError::Device("bad PCM name".into()))?;
        let mut pcm: Pcm = std::ptr::null_mut();
        // SAFETY: valid out pointer and NUL-terminated name.
        let rc = unsafe {
            (alsa.snd_pcm_open)(
                &mut pcm,
                c_name.as_ptr(),
                SND_PCM_STREAM_PLAYBACK,
                SND_PCM_NONBLOCK,
            )
        };
        if rc < 0 {
            return Err(OpenError::Device(format!(
                "open '{name}': {}",
                alsa.err(rc)
            )));
        }
        let sink = Self {
            alsa,
            pcm,
            name: name.to_owned(),
        };
        // SAFETY: `pcm` is open; `configure` frees its own params.
        unsafe { sink.configure() }.map_err(|err| OpenError::Device(format!("'{name}': {err}")))?;
        Ok(sink)
    }

    /// Sets S16_LE, 2 channels, 48 kHz. Fails (without asking alsa-lib) if
    /// the PCM reports a range without them.
    unsafe fn configure(&self) -> Result<(), String> {
        let alsa = &self.alsa;
        let pcm = self.pcm;
        let mut hw: HwParams = std::ptr::null_mut();
        let rc = unsafe { (alsa.snd_pcm_hw_params_malloc)(&mut hw) };
        if rc < 0 {
            return Err(format!("hw_params_malloc: {}", alsa.err(rc)));
        }
        let result = (|| unsafe {
            let rc = (alsa.snd_pcm_hw_params_any)(pcm, hw);
            if rc < 0 {
                return Err(format!("hw_params_any: {}", alsa.err(rc)));
            }
            let (mut rate_min, mut rate_max, mut dir) = (0, 0, 0);
            (alsa.snd_pcm_hw_params_get_rate_min)(hw, &mut rate_min, &mut dir);
            (alsa.snd_pcm_hw_params_get_rate_max)(hw, &mut rate_max, &mut dir);
            let (mut ch_min, mut ch_max) = (0, 0);
            (alsa.snd_pcm_hw_params_get_channels_min)(hw, &mut ch_min);
            (alsa.snd_pcm_hw_params_get_channels_max)(hw, &mut ch_max);
            let s16 = (alsa.snd_pcm_hw_params_test_format)(pcm, hw, SND_PCM_FORMAT_S16_LE);
            if s16 != 0 {
                return Err("S16_LE not accepted".into());
            }
            if !(rate_min..=rate_max).contains(&RATE) {
                return Err(format!("takes {rate_min}..{rate_max} Hz, not {RATE}"));
            }
            if !(ch_min..=ch_max).contains(&(CHANNELS as c_uint)) {
                return Err(format!("takes {ch_min}..{ch_max} channels, not {CHANNELS}"));
            }
            let steps: [(&str, c_int); 4] = [
                (
                    "set_access",
                    (alsa.snd_pcm_hw_params_set_access)(pcm, hw, SND_PCM_ACCESS_RW_INTERLEAVED),
                ),
                (
                    "set_format",
                    (alsa.snd_pcm_hw_params_set_format)(pcm, hw, SND_PCM_FORMAT_S16_LE),
                ),
                (
                    "set_channels",
                    (alsa.snd_pcm_hw_params_set_channels)(pcm, hw, CHANNELS as c_uint),
                ),
                (
                    "set_rate",
                    (alsa.snd_pcm_hw_params_set_rate)(pcm, hw, RATE, 0),
                ),
            ];
            for (step, rc) in steps {
                if rc < 0 {
                    return Err(format!("{step}: {}", alsa.err(rc)));
                }
            }
            let rc = (alsa.snd_pcm_hw_params)(pcm, hw);
            if rc < 0 {
                return Err(format!("hw_params: {}", alsa.err(rc)));
            }
            Ok(())
        })();
        unsafe { (alsa.snd_pcm_hw_params_free)(hw) };
        result?;

        let (mut buffer, mut period) = (0, 0);
        // SAFETY: `pcm` is open and configured.
        unsafe { (alsa.snd_pcm_get_params)(pcm, &mut buffer, &mut period) };
        tracing::info!(
            "[audio] opened '{}': {RATE} Hz, {CHANNELS} ch, S16_LE, buffer {buffer} frames ({} ms), period {period}",
            self.name,
            u64::from(buffer as u32) * 1000 / u64::from(RATE),
        );
        Ok(())
    }

    fn prepare(&mut self) {
        // SAFETY: `pcm` is open.
        let rc = unsafe { (self.alsa.snd_pcm_prepare)(self.pcm) };
        if rc < 0 {
            tracing::warn!("[audio] '{}': prepare: {}", self.name, self.alsa.err(rc));
        }
    }
}

impl AudioSink for AlsaSink {
    fn queue(&mut self) -> Result<QueueState, String> {
        let mut frames: c_long = 0;
        // SAFETY: `pcm` is open; valid out pointer.
        let rc = unsafe { (self.alsa.snd_pcm_delay)(self.pcm, &mut frames) };
        if rc == -(EPIPE as c_int) {
            self.prepare();
            return Ok(QueueState {
                queued: 0,
                underrun: true,
            });
        }
        if rc < 0 {
            // A PREPARED PCM with nothing written may report an error on some
            // plugins; it has nothing queued.
            // SAFETY: `pcm` is open.
            if unsafe { (self.alsa.snd_pcm_state)(self.pcm) } == SND_PCM_STATE_PREPARED {
                return Ok(QueueState {
                    queued: 0,
                    underrun: false,
                });
            }
            return Err(format!("delay: {}", self.alsa.err(rc)));
        }
        Ok(QueueState {
            queued: frames.max(0) as usize,
            underrun: false,
        })
    }

    fn write(&mut self, samples: &[i16]) -> Result<usize, String> {
        let frames = samples.len() / CHANNELS;
        if frames == 0 {
            return Ok(0);
        }
        // SAFETY: `samples` holds `frames` interleaved stereo frames.
        let written = unsafe {
            (self.alsa.snd_pcm_writei)(self.pcm, samples.as_ptr().cast(), frames as c_ulong)
        };
        if written == -EAGAIN {
            return Ok(0);
        }
        if written == -EPIPE {
            self.prepare();
            return Ok(0);
        }
        if written < 0 {
            return Err(format!("write: {}", self.alsa.err(written as c_int)));
        }
        // A PCM with a high start threshold waits for more; start it, so a
        // short clip plays too.
        // SAFETY: `pcm` is open.
        unsafe {
            if (self.alsa.snd_pcm_state)(self.pcm) == SND_PCM_STATE_PREPARED {
                (self.alsa.snd_pcm_start)(self.pcm);
            }
        }
        Ok(written as usize)
    }

    fn drop_queue(&mut self) {
        // SAFETY: `pcm` is open.
        unsafe { (self.alsa.snd_pcm_drop)(self.pcm) };
        self.prepare();
    }
}

impl Drop for AlsaSink {
    fn drop(&mut self) {
        // SAFETY: `pcm` is open and not used after this.
        unsafe {
            (self.alsa.snd_pcm_drop)(self.pcm);
            (self.alsa.snd_pcm_close)(self.pcm);
        }
        tracing::info!("[audio] closed '{}'", self.name);
    }
}
