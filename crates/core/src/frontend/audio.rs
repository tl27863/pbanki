//! Card audio in the study screen (notes/audio.md).
//!
//! The question's sounds play when a card shows, the answer's on Show answer
//! (deck option "Don't play audio automatically" off). The replay button
//! plays the shown side again, or stops. Rating, leaving the study screen,
//! opening Settings and a sync stop the sound.
//!
//! The state lives on the UI thread, like the card pages (`card_pages`). The
//! audio thread reports through `AudioEvent`s, which arrive here on the event
//! loop; a stale event (an older clip) is ignored.

use std::cell::RefCell;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use anki_proto::card_rendering::AvTag;
use anki_proto::card_rendering::av_tag::Value;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::audio::{AudioEvent, AudioPlayer, volume_gain};
use crate::card_render::absolute_media_folder;
use crate::config::Config;
use crate::study::{AudioOptions, LoadedCard};

/// Card audio from the platform. `FrontendHooks::audio` is `None` on a device
/// without sound: no audio thread, no replay button, no audio settings.
#[derive(Clone)]
pub struct AudioSetup {
    pub player: AudioPlayer,
    /// ALSA PCM names to try, in order. `audio_device` in the config replaces
    /// them.
    pub devices: Vec<String>,
    /// Asked on the UI thread before each clip. False when the device has no
    /// output right now (a Bluetooth-only device without a headset): then no
    /// PCM is opened, so a write cannot stall on it.
    pub output_ready: Option<Rc<dyn Fn() -> bool>>,
}

/// Top bar line when the output check fails.
const NO_OUTPUT: &str = "No audio output";
/// Top bar line when audio is off for the session after failures.
const UNAVAILABLE: &str = "Audio unavailable";
/// Top bar line when one clip could not play.
const NOT_PLAYED: &str = "Sound could not play";

thread_local! {
    static STATE: RefCell<Option<AudioState>> = const { RefCell::new(None) };
}

struct AudioState {
    setup: AudioSetup,
    ui: slint::Weak<MainWindow>,
    media: PathBuf,
    enabled: bool,
    volume: u32,
    device: Option<String>,
    question: Vec<PathBuf>,
    answer: Vec<PathBuf>,
    options: AudioOptions,
    /// The token of the last clip asked for.
    token: u64,
}

/// Sets up card audio, if the platform has it.
pub(super) fn init(ui: &MainWindow, setup: Option<AudioSetup>, config: &Config, media: &Path) {
    ui.set_audio_supported(setup.is_some());
    let Some(setup) = setup else {
        tracing::info!("[audio] no audio on this device");
        return;
    };
    setup.player.set_listener(|event| {
        let _ = slint::invoke_from_event_loop(move || on_event(event));
    });
    let state = AudioState {
        setup,
        ui: ui.as_weak(),
        media: absolute_media_folder(media),
        enabled: config.general.audio_enabled,
        volume: config.general.audio_volume,
        device: config.general.audio_device.clone(),
        question: Vec::new(),
        answer: Vec::new(),
        options: AudioOptions::default(),
        token: 0,
    };
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
}

fn with_state<R>(f: impl FnOnce(&mut AudioState) -> R) -> Option<R> {
    STATE.with(|cell| cell.borrow_mut().as_mut().map(f))
}

/// The device settings changed (Settings > Device).
pub(super) fn apply_config(config: &Config) {
    with_state(|state| {
        state.volume = config.general.audio_volume;
        state.device = config.general.audio_device.clone();
        if state.enabled != config.general.audio_enabled {
            state.enabled = config.general.audio_enabled;
            if !state.enabled {
                state.stop();
            }
            state.update_flags();
        }
    });
}

/// A card was loaded: stops the sound and plays the question's if autoplay is
/// on. Call it before the card is shown.
pub(super) fn card_loaded(loaded: &LoadedCard) {
    with_state(|state| {
        state.stop();
        state.question = state.files(&loaded.question_av);
        state.answer = state.files(&loaded.answer_av);
        state.options = loaded.audio;
        state.set_message("");
        state.update_flags();
        if state.enabled && state.options.autoplay && !state.question.is_empty() {
            state.play(state.question.clone());
        }
    });
}

/// No card is shown (a message, an error, or the study screen closed).
pub(super) fn clear_card() {
    with_state(|state| {
        state.stop();
        state.question.clear();
        state.answer.clear();
        state.set_message("");
        state.update_flags();
    });
}

/// The answer side was opened.
pub(super) fn answer_shown() {
    with_state(|state| {
        state.stop();
        if state.enabled && state.options.autoplay && !state.answer.is_empty() {
            state.play(state.answer.clone());
        }
    });
}

/// The replay button: stops a playing clip, or plays the shown side. On the
/// answer side the question's sounds come first unless the deck option "Skip
/// question when replaying answer" is on.
pub(super) fn replay(answer_side: bool) {
    with_state(|state| {
        let playing = state.ui.upgrade().is_some_and(|ui| ui.get_audio_playing());
        if playing {
            state.stop();
            return;
        }
        let files = if answer_side {
            let mut files = Vec::new();
            if state.options.replay_question {
                files.extend(state.question.iter().cloned());
            }
            files.extend(state.answer.iter().cloned());
            files
        } else {
            state.question.clone()
        };
        if state.enabled && !files.is_empty() {
            state.play(files);
        }
    });
}

/// Stops the sound, if any.
pub(super) fn stop() {
    with_state(AudioState::stop);
}

impl AudioState {
    /// The media files of `[sound:…]` tags. TTS tags are not supported, and a
    /// name that is not a plain file name in the media folder is skipped.
    fn files(&self, tags: &[AvTag]) -> Vec<PathBuf> {
        tags.iter()
            .filter_map(|tag| match &tag.value {
                Some(Value::SoundOrVideo(name)) => match media_file(&self.media, name) {
                    Some(path) => Some(path),
                    None => {
                        tracing::warn!("[audio] skipped sound '{name}': not a media file name");
                        None
                    }
                },
                Some(Value::Tts(_)) => {
                    tracing::info!("[audio] skipped a TTS tag: not supported");
                    None
                }
                None => None,
            })
            .collect()
    }

    fn devices(&self) -> Vec<String> {
        match self.device.as_deref().map(str::trim) {
            Some(device) if !device.is_empty() => vec![device.to_owned()],
            _ => self.setup.devices.clone(),
        }
    }

    fn play(&mut self, files: Vec<PathBuf>) {
        if let Some(ready) = &self.setup.output_ready
            && !ready()
        {
            tracing::info!("[audio] no audio output; not playing");
            self.set_message(NO_OUTPUT);
            return;
        }
        self.token += 1;
        self.setup
            .player
            .play(files, self.devices(), volume_gain(self.volume), self.token);
    }

    fn stop(&mut self) {
        self.setup.player.stop();
        // Events of the stopped clip are stale from here.
        self.token += 1;
        if let Some(ui) = self.ui.upgrade()
            && ui.get_audio_playing()
        {
            ui.set_audio_playing(false);
            ui.window().request_redraw();
        }
    }

    fn set_message(&self, message: &str) {
        if let Some(ui) = self.ui.upgrade()
            && ui.get_audio_message() != message
        {
            ui.set_audio_message(message.into());
            ui.window().request_redraw();
        }
    }

    /// Which sides show the replay button.
    fn update_flags(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_audio_question(self.enabled && !self.question.is_empty());
            ui.set_audio_answer(
                self.enabled
                    && (!self.answer.is_empty()
                        || (self.options.replay_question && !self.question.is_empty())),
            );
        }
    }
}

/// An event of the audio thread, on the UI thread.
fn on_event(event: AudioEvent) {
    with_state(|state| {
        let token = match &event {
            AudioEvent::Started { token }
            | AudioEvent::Finished { token }
            | AudioEvent::Failed { token, .. } => *token,
        };
        if token != state.token {
            return;
        }
        let Some(ui) = state.ui.upgrade() else {
            return;
        };
        let playing = matches!(event, AudioEvent::Started { .. });
        if ui.get_audio_playing() != playing {
            ui.set_audio_playing(playing);
            ui.window().request_redraw();
        }
        if let AudioEvent::Failed { disabled, .. } = event {
            state.set_message(if disabled { UNAVAILABLE } else { NOT_PLAYED });
        }
    });
}

/// `name` in the media folder, if it is a plain file name. Anki keeps media
/// flat, so a separator or `..` is not a media file.
fn media_file(media: &Path, name: &str) -> Option<PathBuf> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(file)), None) if !name.contains('\\') => Some(media.join(file)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_are_media_files() {
        let media = Path::new("/media");
        assert_eq!(
            media_file(media, "a b.mp3"),
            Some(PathBuf::from("/media/a b.mp3"))
        );
        for bad in [
            "",
            "..",
            ".",
            "../x.mp3",
            "/etc/passwd",
            "a/b.mp3",
            "a\\b.mp3",
        ] {
            assert_eq!(media_file(media, bad), None, "{bad}");
        }
    }
}
