//! Decodes one sound file into the sink format: 48 kHz, stereo, S16.
//!
//! The conversion happens here and never in the ALSA plug chain. On the
//! PocketBook the plug chain can abort inside alsa-lib for a rate it does not
//! support, and the device PCM (`hwout_mix`) only takes 48 kHz stereo
//! (notes/audio.md).

use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use super::{CHANNELS, RATE};

/// Decode errors in a row before a file is given up.
const MAX_DECODE_ERRORS: u32 = 8;

/// A sound file being decoded.
pub(crate) struct ClipDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    gain: f32,
    resampler: Option<Resampler>,
    /// Interleaved samples of the last packet, in the file's layout.
    scratch: Vec<f32>,
    /// Stereo frames of the last packet, before resampling.
    frames: Vec<[f32; 2]>,
    errors: u32,
    finished: bool,
}

impl ClipDecoder {
    /// Opens the file and its first audio track. `gain` is a linear factor.
    pub fn open(path: &Path, gain: f32) -> Result<Self, String> {
        let file = File::open(path).map_err(|err| format!("open: {err}"))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|err| format!("unsupported format: {err}"))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or("no audio track")?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or("no audio codec parameters")?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|err| format!("unsupported codec: {err}"))?;
        let track_id = track.id;
        Ok(Self {
            format,
            decoder,
            track_id,
            gain,
            resampler: None,
            scratch: Vec::new(),
            frames: Vec::new(),
            errors: 0,
            finished: false,
        })
    }

    /// Decodes the next packet and appends its samples (48 kHz, interleaved
    /// stereo) to `out`. Returns false at the end of the file.
    pub fn decode_into(&mut self, out: &mut VecDeque<i16>) -> Result<bool, String> {
        if self.finished {
            return Ok(false);
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(Error::ResetRequired) => {
                    // The end, or a chained OGG stream: treat both as the end.
                    self.finish(out);
                    return Ok(false);
                }
                Err(Error::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.finish(out);
                    return Ok(false);
                }
                Err(err) => return Err(format!("read: {err}")),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(Error::DecodeError(err)) => {
                    self.errors += 1;
                    if self.errors > MAX_DECODE_ERRORS {
                        return Err(format!("decode: {err}"));
                    }
                    continue;
                }
                Err(Error::IoError(_)) => continue,
                Err(err) => return Err(format!("decode: {err}")),
            };
            self.errors = 0;
            let rate = decoded.spec().rate();
            let channels = decoded.spec().channels().count();
            if channels == 0 || rate == 0 || decoded.frames() == 0 {
                continue;
            }
            self.scratch.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(&mut self.scratch);
            to_stereo(&self.scratch, channels, &mut self.frames);

            if self.resampler.as_ref().is_none_or(|r| r.in_rate != rate) {
                // A rate change inside a file is rare; the old tail is lost.
                if let Some(old) = self.resampler.as_mut() {
                    old.flush(|frame| push_frame(out, frame, self.gain));
                }
                self.resampler = Some(Resampler::new(rate, RATE));
            }
            let gain = self.gain;
            if let Some(resampler) = self.resampler.as_mut() {
                resampler.process(&self.frames, |frame| push_frame(out, frame, gain));
            }
            return Ok(true);
        }
    }

    fn finish(&mut self, out: &mut VecDeque<i16>) {
        self.finished = true;
        let gain = self.gain;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.flush(|frame| push_frame(out, frame, gain));
        }
    }
}

/// The left and right channel of each frame. Mono is copied to both sides; of
/// more channels only the first two (front left and right) are kept.
fn to_stereo(interleaved: &[f32], channels: usize, out: &mut Vec<[f32; 2]>) {
    out.clear();
    out.extend(interleaved.chunks_exact(channels).map(|frame| {
        if channels == 1 {
            [frame[0], frame[0]]
        } else {
            [frame[0], frame[1]]
        }
    }));
}

fn push_frame(out: &mut VecDeque<i16>, frame: [f32; 2], gain: f32) {
    debug_assert_eq!(CHANNELS, 2);
    for sample in frame {
        out.push_back(to_i16(sample * gain));
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// Streaming sample-rate conversion with cubic (Catmull-Rom) interpolation.
///
/// Card audio is mostly short speech at 22.05, 24, 44.1 or 48 kHz. Cubic
/// interpolation is enough for that and costs a few multiplications per
/// sample. A rate equal to the output rate passes through unchanged.
pub(crate) struct Resampler {
    in_rate: u32,
    /// Input frames per output frame.
    step: f64,
    /// Position of the next output frame in `buf`, in input frames.
    pos: f64,
    /// Input frames not yet consumed. `buf[0]` is the frame before `pos`.
    buf: Vec<[f32; 2]>,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            in_rate,
            step: f64::from(in_rate) / f64::from(out_rate),
            pos: 1.0,
            // One frame of silence before the first input frame.
            buf: vec![[0.0; 2]],
        }
    }

    fn passthrough(&self) -> bool {
        self.step == 1.0
    }

    pub fn process(&mut self, input: &[[f32; 2]], mut emit: impl FnMut([f32; 2])) {
        if self.passthrough() {
            input.iter().copied().for_each(emit);
            return;
        }
        self.buf.extend_from_slice(input);
        // Each output frame needs the frames at floor(pos) - 1 ..= floor(pos) + 2.
        loop {
            let index = self.pos as usize;
            if index + 2 >= self.buf.len() {
                break;
            }
            let t = (self.pos - index as f64) as f32;
            let p = [
                self.buf[index - 1],
                self.buf[index],
                self.buf[index + 1],
                self.buf[index + 2],
            ];
            emit([
                catmull_rom(p[0][0], p[1][0], p[2][0], p[3][0], t),
                catmull_rom(p[0][1], p[1][1], p[2][1], p[3][1], t),
            ]);
            self.pos += self.step;
        }
        // Keep the frame before `pos` and everything after it.
        let keep_from = (self.pos as usize).saturating_sub(1);
        self.buf.drain(..keep_from);
        self.pos -= keep_from as f64;
    }

    /// Emits the frames still held back, as if two silent frames followed.
    pub fn flush(&mut self, emit: impl FnMut([f32; 2])) {
        if self.passthrough() {
            return;
        }
        // Only the frames that fall inside the real input: positions before
        // the end of `buf` as it is now (`process` drains it afterwards).
        let input_end = self.buf.len() as f64;
        let step = self.step;
        let mut pos = self.pos;
        let mut emit = emit;
        self.process(&[[0.0; 2], [0.0; 2]], |frame| {
            if pos < input_end {
                emit(frame);
            }
            pos += step;
        });
    }
}

fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, frames: usize) -> Vec<[f32; 2]> {
        (0..frames)
            .map(|i| {
                let v = (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                [v, -v]
            })
            .collect()
    }

    fn resample_all(input: &[[f32; 2]], in_rate: u32, chunk: usize) -> Vec<[f32; 2]> {
        let mut resampler = Resampler::new(in_rate, RATE);
        let mut out = Vec::new();
        for part in input.chunks(chunk) {
            resampler.process(part, |frame| out.push(frame));
        }
        resampler.flush(|frame| out.push(frame));
        out
    }

    #[test]
    fn resampled_length_follows_the_rate() {
        for in_rate in [8_000, 22_050, 24_000, 44_100, 96_000] {
            let input = sine(in_rate, 440.0, in_rate as usize); // 1 s
            let out = resample_all(&input, in_rate, 1000);
            let expected = RATE as usize;
            assert!(
                out.len().abs_diff(expected) <= 2,
                "{in_rate} Hz: {} frames, expected about {expected}",
                out.len()
            );
        }
    }

    #[test]
    fn resampling_does_not_depend_on_the_chunk_size() {
        let input = sine(44_100, 440.0, 10_000);
        let whole = resample_all(&input, 44_100, input.len());
        let small = resample_all(&input, 44_100, 7);
        assert_eq!(whole.len(), small.len());
        for (a, b) in whole.iter().zip(&small) {
            assert!((a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn resampled_sine_keeps_its_shape() {
        let input = sine(22_050, 440.0, 22_050);
        let out = resample_all(&input, 22_050, 512);
        let expected = sine(RATE, 440.0, out.len());
        // Skip the first frames (the silent frame before the input).
        let worst = out
            .iter()
            .zip(&expected)
            .skip(8)
            .take(out.len() - 16)
            .map(|(a, b)| (a[0] - b[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "largest error {worst}");
    }

    #[test]
    fn the_output_rate_passes_through() {
        let input = sine(RATE, 440.0, 1000);
        let out = resample_all(&input, RATE, 100);
        assert_eq!(out, input);
    }

    #[test]
    fn mono_goes_to_both_sides_and_extra_channels_are_dropped() {
        let mut out = Vec::new();
        to_stereo(&[0.1, 0.2], 1, &mut out);
        assert_eq!(out, vec![[0.1, 0.1], [0.2, 0.2]]);
        to_stereo(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 3, &mut out);
        assert_eq!(out, vec![[0.1, 0.2], [0.4, 0.5]]);
    }

    #[test]
    fn samples_are_clamped() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
        assert_eq!(to_i16(0.0), 0);
    }
}
