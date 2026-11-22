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

// The resampler writes interleaved stereo.
const _: () = assert!(CHANNELS == 2);

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

            if self
                .resampler
                .as_ref()
                .is_none_or(|r| r.in_rate != rate || r.channels != channels)
            {
                // A format change inside a file is rare; the old tail is lost.
                if let Some(old) = self.resampler.as_mut() {
                    old.flush(out);
                }
                self.resampler = Some(Resampler::new(rate, RATE, channels, self.gain));
            }
            if let Some(resampler) = self.resampler.as_mut() {
                resampler.process(&self.scratch, out);
            }
            return Ok(true);
        }
    }

    fn finish(&mut self, out: &mut VecDeque<i16>) {
        self.finished = true;
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.flush(out);
        }
    }
}

/// Converts a sample to S16, rounding half away from zero like `f32::round`.
/// `round` itself is a libm `roundf` call on armv7 (no VRINTA before ARMv8);
/// adding ±0.5 and truncating with `as` is not.
fn to_i16(sample: f32) -> i16 {
    let v = sample.clamp(-1.0, 1.0) * i16::MAX as f32;
    (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i16
}

/// The Catmull-Rom weights of the four points around `t` (0..1 between the
/// second and the third point).
fn weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

/// Streaming conversion of interleaved f32 in the file's layout to 48 kHz
/// interleaved stereo S16, with cubic (Catmull-Rom) interpolation and the
/// gain applied.
///
/// Card audio is mostly short speech at 22.05, 24, 44.1 or 48 kHz. Cubic
/// interpolation is enough for that (notes/audio.md, "Resampler"). Mono is
/// interpolated once and written to both sides; of more channels the first
/// two (front left and right) are kept. A rate equal to the output rate
/// passes through unchanged.
pub(crate) struct Resampler {
    in_rate: u32,
    /// Channels of the input.
    channels: usize,
    /// Channels kept in `buf`: 1 for mono input, else 2.
    kept: usize,
    gain: f32,
    passthrough: bool,
    /// Input frames per output frame: `step_int + step_num / den`, exact.
    step_int: usize,
    step_num: u32,
    den: u32,
    /// Position of the next output frame: `idx + num / den`, in frames of
    /// `buf`.
    idx: usize,
    num: u32,
    /// Input frames not yet consumed, `kept` samples each. Frame 0 is the one
    /// before the position.
    buf: Vec<f32>,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize, gain: f32) -> Self {
        let g = gcd(in_rate, out_rate);
        let (num, den) = (in_rate / g, out_rate / g);
        let kept = if channels == 1 { 1 } else { 2 };
        Self {
            in_rate,
            channels,
            kept,
            gain,
            passthrough: in_rate == out_rate,
            step_int: (num / den) as usize,
            step_num: num % den,
            den,
            idx: 1,
            num: 0,
            // One frame of silence before the first input frame.
            buf: vec![0.0; kept],
        }
    }

    /// Converts `interleaved` (whole frames of `channels` samples) and
    /// appends the result to `out`.
    pub fn process(&mut self, interleaved: &[f32], out: &mut VecDeque<i16>) {
        let channels = self.channels;
        if self.passthrough {
            let gain = self.gain;
            out.reserve(interleaved.len() / channels * 2);
            for frame in interleaved.chunks_exact(channels) {
                let left = to_i16(frame[0] * gain);
                out.push_back(left);
                out.push_back(if channels == 1 {
                    left
                } else {
                    to_i16(frame[1] * gain)
                });
            }
            return;
        }
        if channels == 1 {
            self.buf.extend_from_slice(interleaved);
            self.run::<1>(usize::MAX, out);
        } else {
            self.buf.extend(
                interleaved
                    .chunks_exact(channels)
                    .flat_map(|frame| [frame[0], frame[1]]),
            );
            self.run::<2>(usize::MAX, out);
        }
    }

    /// Emits the frames still held back, as if two silent frames followed.
    pub fn flush(&mut self, out: &mut VecDeque<i16>) {
        if self.passthrough {
            return;
        }
        // Only the frames that fall inside the real input.
        let input_end = self.buf.len() / self.kept;
        self.buf.extend(std::iter::repeat_n(0.0, 2 * self.kept));
        if self.kept == 1 {
            self.run::<1>(input_end, out);
        } else {
            self.run::<2>(input_end, out);
        }
    }

    /// Emits every output frame whose four points are in `buf`, but only
    /// those before frame `limit`. `C` is `self.kept`.
    fn run<const C: usize>(&mut self, limit: usize, out: &mut VecDeque<i16>) {
        let frames = self.buf.len() / C;
        let inv_den = 1.0 / self.den as f32;
        // Each output frame needs the frames idx - 1 ..= idx + 2.
        while self.idx + 2 < frames {
            if self.idx < limit {
                let w = weights(self.num as f32 * inv_den).map(|w| w * self.gain);
                let p = &self.buf[(self.idx - 1) * C..(self.idx + 3) * C];
                let left = to_i16(w[0] * p[0] + w[1] * p[C] + w[2] * p[2 * C] + w[3] * p[3 * C]);
                let right = if C == 1 {
                    left
                } else {
                    to_i16(
                        w[0] * p[1] + w[1] * p[C + 1] + w[2] * p[2 * C + 1] + w[3] * p[3 * C + 1],
                    )
                };
                out.push_back(left);
                out.push_back(right);
            }
            self.idx += self.step_int;
            self.num += self.step_num;
            if self.num >= self.den {
                self.num -= self.den;
                self.idx += 1;
            }
        }
        // Keep the frame before the position and everything after it.
        let keep_from = self.idx - 1;
        self.buf.drain(..keep_from * C);
        self.idx -= keep_from;
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect()
    }

    /// Interleaved stereo with `-v` on the right.
    fn stereo(mono: &[f32]) -> Vec<f32> {
        mono.iter().flat_map(|&v| [v, -v]).collect()
    }

    fn resample_all(input: &[f32], channels: usize, in_rate: u32, chunk: usize) -> Vec<i16> {
        let mut resampler = Resampler::new(in_rate, RATE, channels, 1.0);
        let mut out = VecDeque::new();
        for part in input.chunks(chunk * channels) {
            resampler.process(part, &mut out);
        }
        resampler.flush(&mut out);
        out.into()
    }

    /// The direct formula: output frame k at input position k * step, with
    /// silence before and after the input, and only positions inside it.
    fn reference(input: &[f32], in_rate: u32) -> Vec<f32> {
        let at = |i: isize| {
            usize::try_from(i)
                .ok()
                .and_then(|i| input.get(i))
                .copied()
                .unwrap_or(0.0)
        };
        let step = f64::from(in_rate) / f64::from(RATE);
        let mut out = Vec::new();
        let mut k = 0u32;
        loop {
            let pos = f64::from(k) * step;
            if pos >= input.len() as f64 {
                break;
            }
            let i = pos as isize;
            let t = (pos - i as f64) as f32;
            let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
            let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
            let c = -0.5 * p0 + 0.5 * p2;
            out.push(((a * t + b) * t + c) * t + p1);
            k += 1;
        }
        out
    }

    #[test]
    fn output_matches_the_catmull_rom_formula() {
        for in_rate in [16_000, 22_050, 24_000, 44_100] {
            let input = sine(in_rate, 440.0, 5_000);
            let expected = reference(&input, in_rate);
            let out = resample_all(&input, 1, in_rate, 1152);
            assert_eq!(out.len(), expected.len() * 2, "{in_rate} Hz");
            for (frame, want) in out.chunks(2).zip(&expected) {
                let want = to_i16(*want);
                assert!(
                    (frame[0] - want).abs() <= 1 && frame[0] == frame[1],
                    "{in_rate} Hz: {frame:?}, expected {want}"
                );
            }
        }
    }

    #[test]
    fn resampled_length_follows_the_rate() {
        for in_rate in [8_000, 22_050, 24_000, 44_100, 96_000] {
            let input = stereo(&sine(in_rate, 440.0, in_rate as usize)); // 1 s
            let frames = resample_all(&input, 2, in_rate, 1000).len() / 2;
            let expected = RATE as usize;
            assert!(
                frames.abs_diff(expected) <= 2,
                "{in_rate} Hz: {frames} frames, expected about {expected}"
            );
        }
    }

    #[test]
    fn resampling_does_not_depend_on_the_chunk_size() {
        let input = stereo(&sine(44_100, 440.0, 10_000));
        let whole = resample_all(&input, 2, 44_100, 10_000);
        let small = resample_all(&input, 2, 44_100, 7);
        assert_eq!(whole, small);
    }

    #[test]
    fn resampled_sine_keeps_its_shape() {
        let input = stereo(&sine(22_050, 440.0, 22_050));
        let out = resample_all(&input, 2, 22_050, 512);
        let expected = sine(RATE, 440.0, out.len() / 2);
        // Skip the first frames (the silent frame before the input).
        let worst = out
            .chunks(2)
            .zip(&expected)
            .skip(8)
            .take(expected.len() - 16)
            .map(|(frame, want)| {
                assert_eq!(frame[0], -frame[1]);
                (f32::from(frame[0]) / f32::from(i16::MAX) - want).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "largest error {worst}");
    }

    #[test]
    fn the_output_rate_passes_through() {
        let input = stereo(&sine(RATE, 440.0, 1000));
        let out = resample_all(&input, 2, RATE, 100);
        let expected: Vec<i16> = input.iter().map(|&v| to_i16(v)).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn mono_goes_to_both_sides_and_matches_stereo() {
        let mono = sine(22_050, 440.0, 3_000);
        let both: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
        let from_mono = resample_all(&mono, 1, 22_050, 1152);
        assert_eq!(from_mono, resample_all(&both, 2, 22_050, 1152));
        // Passthrough too.
        let mut resampler = Resampler::new(RATE, RATE, 1, 1.0);
        let mut out = VecDeque::new();
        resampler.process(&[0.25, -0.5], &mut out);
        assert_eq!(
            out,
            [to_i16(0.25), to_i16(0.25), to_i16(-0.5), to_i16(-0.5)]
        );
    }

    #[test]
    fn extra_channels_are_dropped() {
        // Three channels: the third is loud and must not show.
        let input: Vec<f32> = sine(RATE, 440.0, 500)
            .iter()
            .flat_map(|&v| [v, -v, 1.0])
            .collect();
        let out = resample_all(&input, 3, RATE, 100);
        let expected: Vec<i16> = sine(RATE, 440.0, 500)
            .iter()
            .flat_map(|&v| [to_i16(v), to_i16(-v)])
            .collect();
        assert_eq!(out, expected);
        // And through the interpolation.
        let resampled = resample_all(&input, 3, 22_050, 128);
        let stereo_only: Vec<f32> = input
            .chunks(3)
            .flat_map(|frame| [frame[0], frame[1]])
            .collect();
        assert_eq!(resampled, resample_all(&stereo_only, 2, 22_050, 128));
    }

    #[test]
    fn the_gain_is_applied() {
        let input = vec![0.5; 2_000];
        let mut resampler = Resampler::new(22_050, RATE, 1, 0.5);
        let mut out = VecDeque::new();
        resampler.process(&input, &mut out);
        let middle: Vec<i16> = out.iter().skip(100).take(1000).copied().collect();
        assert!(middle.iter().all(|&s| s == to_i16(0.25)), "{middle:?}");
    }

    #[test]
    fn samples_are_clamped_and_rounded_like_f32_round() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
        assert_eq!(to_i16(0.0), 0);
        for i in -70_000..=70_000 {
            // Includes the halfway points between steps.
            let sample = i as f32 / 2.0 / f32::from(i16::MAX);
            let want = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            assert_eq!(to_i16(sample), want, "{sample}");
        }
    }
}
