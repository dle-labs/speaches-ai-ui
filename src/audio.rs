//! Bounded, in-memory microphone capture. No audio leaves this module automatically.

use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cpal::{
    SampleFormat, SizedSample, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

const MAX_DURATION: Duration = Duration::from_secs(5 * 60);
const CHIME_SECONDS: f32 = 0.10;
const MAX_SAMPLE_BYTES: usize = 32 * 1024 * 1024;
const STREAM_FAILED: u8 = 1;
const BUFFER_FAILED: u8 = 2;
const INVALID_FRAME: u8 = 3;

pub(crate) fn devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let mut names = host
        .input_devices()
        .context("Could not enumerate microphones")?
        .map(|device| device.name().context("Could not read microphone name"))
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    names.dedup();
    Ok(names)
}

struct Samples {
    data: Vec<i16>,
    written: usize,
}

struct Capture {
    samples: Mutex<Samples>,
    level: AtomicU32,
    failure: AtomicU8,
    limited: AtomicBool,
    stopped: AtomicBool,
    started: Instant,
}

impl Capture {
    fn new(capacity: usize) -> Result<Self> {
        let mut data = Vec::new();
        data.try_reserve_exact(capacity)
            .context("Could not allocate microphone recording buffer")?;
        // Initialize outside the callback, so capture never grows the allocation.
        data.resize(capacity, 0);
        Ok(Self {
            samples: Mutex::new(Samples { data, written: 0 }),
            level: AtomicU32::new(0.0_f32.to_bits()),
            failure: AtomicU8::new(0),
            limited: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            started: Instant::now(),
        })
    }

    fn fail(&self, code: u8) {
        let _ = self
            .failure
            .compare_exchange(0, code, Ordering::Relaxed, Ordering::Relaxed);
        self.level.store(0.0_f32.to_bits(), Ordering::Relaxed);
    }

    fn limit_reached(&self) -> bool {
        if self.started.elapsed() >= MAX_DURATION {
            self.limited.store(true, Ordering::Relaxed);
        }
        self.limited.load(Ordering::Relaxed)
    }

    fn error(&self) -> Option<String> {
        let message = match self.failure.load(Ordering::Relaxed) {
            0 => return None,
            STREAM_FAILED => {
                "Microphone stream failed or the device disconnected; recording was discarded"
            }
            BUFFER_FAILED => {
                "Microphone buffer was unavailable; recording was discarded to avoid missing audio"
            }
            INVALID_FRAME => {
                "Microphone supplied an incomplete audio frame; recording was discarded"
            }
            _ => "Microphone capture failed; recording was discarded",
        };
        Some(message.to_owned())
    }

    fn record<T: Copy>(&self, data: &[T], channels: usize, convert: impl Fn(T) -> f32) {
        if self.stopped.load(Ordering::Relaxed)
            || self.failure.load(Ordering::Relaxed) != 0
            || self.limit_reached()
        {
            self.level.store(0.0_f32.to_bits(), Ordering::Relaxed);
            return;
        }
        if channels == 0 || !data.len().is_multiple_of(channels) {
            self.fail(INVALID_FRAME);
            return;
        }
        // Never wait for a UI/finish operation or another callback on the audio thread.
        let Ok(mut samples) = self.samples.try_lock() else {
            self.fail(BUFFER_FAILED);
            return;
        };
        let remaining = samples.data.len().saturating_sub(samples.written);
        let mut peak = 0.0_f32;
        for frame in data.chunks_exact(channels).take(remaining) {
            let mono = downmix(frame, &convert);
            peak = peak.max(mono.abs());
            let index = samples.written;
            samples.data[index] = pcm_i16(mono);
            samples.written += 1;
        }
        if samples.written == samples.data.len() {
            self.limited.store(true, Ordering::Relaxed);
            peak = 0.0;
        }
        self.level.store(peak.to_bits(), Ordering::Relaxed);
    }
}

/// Owns the microphone stream; dropping this value cancels and releases it.
/// Keep this value on its owning thread (CPAL streams are not portable across threads).
pub(crate) struct Recording {
    stream: Option<cpal::Stream>,
    capture: Arc<Capture>,
    sample_rate: u32,
}

pub(crate) struct CapturedAudio {
    pub(crate) wav: Vec<u8>,
    pub(crate) limited: bool,
}

impl Recording {
    /// Uses the default microphone for `None`, otherwise an exact device-name match.
    pub(crate) fn start(device_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            host.input_devices()
                .context("Could not enumerate microphones")?
                .find(|device| device.name().is_ok_and(|candidate| candidate == name))
                .ok_or_else(|| anyhow!("Microphone {name:?} is no longer available"))?
        } else {
            host.default_input_device()
                .context("No default microphone is available")?
        };
        let supported = device.default_input_config().context(
            "Could not read microphone format; check microphone permissions and connection",
        )?;
        let config = supported.config();
        ensure!(
            config.channels > 0 && config.sample_rate.0 > 0,
            "Microphone reported an invalid audio configuration"
        );
        let sample_rate = config.sample_rate.0;
        let capture = Arc::new(Capture::new(sample_limit(sample_rate))?);
        let stream = match supported.sample_format() {
            SampleFormat::F32 => input_stream(&device, &config, Arc::clone(&capture), |s: f32| s),
            SampleFormat::I16 => input_stream(&device, &config, Arc::clone(&capture), |s: i16| {
                f32::from(s) / 32768.0
            }),
            SampleFormat::U16 => input_stream(&device, &config, Arc::clone(&capture), |s: u16| {
                (f32::from(s) - 32768.0) / 32768.0
            }),
            format => {
                bail!("Unsupported microphone sample format: {format:?}; choose another microphone")
            }
        }?;
        stream.play().context("Could not start microphone; check permissions and whether another application is using it")?;
        Ok(Self {
            stream: Some(stream),
            capture,
            sample_rate,
        })
    }

    /// Peak mono amplitude in the latest callback, normalized to 0..=1.
    pub(crate) fn level(&self) -> f32 {
        if self.capture.failure.load(Ordering::Relaxed) != 0 || self.limit_reached() {
            return 0.0;
        }
        f32::from_bits(self.capture.level.load(Ordering::Relaxed))
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.capture.started.elapsed().min(MAX_DURATION)
    }

    pub(crate) fn limit_reached(&self) -> bool {
        self.capture.limit_reached()
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.capture.error()
    }

    /// Stops capture and encodes mono PCM16 WAV at the native input sample rate.
    /// Failed or empty recordings return errors. At the limit, the caller must
    /// retain the bounded recording for explicit consent rather than upload it.
    pub(crate) fn finish(mut self) -> Result<CapturedAudio> {
        self.capture.stopped.store(true, Ordering::Relaxed);
        drop(self.stream.take());
        if let Some(error) = self.error() {
            bail!(error);
        }
        let samples = self
            .capture
            .samples
            .lock()
            .map_err(|_| anyhow!("Microphone buffer could not be read"))?;
        ensure!(samples.written > 0, "No microphone audio was captured");
        Ok(CapturedAudio {
            wav: encode_wav(&samples.data[..samples.written], self.sample_rate)?,
            limited: self.capture.limit_reached(),
        })
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.capture.stopped.store(true, Ordering::Relaxed);
        drop(self.stream.take());
    }
}

fn sample_limit(sample_rate: u32) -> usize {
    (u64::from(sample_rate) * MAX_DURATION.as_secs())
        .min((MAX_SAMPLE_BYTES / size_of::<i16>()) as u64) as usize
}

fn downmix<T: Copy>(frame: &[T], convert: impl Fn(T) -> f32) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame
        .iter()
        .map(|&sample| {
            let value = convert(sample);
            if value.is_finite() {
                f64::from(value.clamp(-1.0, 1.0))
            } else {
                0.0
            }
        })
        .sum();
    (sum / frame.len() as f64) as f32
}

fn pcm_i16(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * 32768.0).round() as i16
}

fn input_stream<T: SizedSample + Copy>(
    device: &cpal::Device,
    config: &StreamConfig,
    capture: Arc<Capture>,
    convert: impl Fn(T) -> f32 + Send + 'static,
) -> Result<cpal::Stream> {
    let errors = Arc::clone(&capture);
    let channels = usize::from(config.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| capture.record(data, channels, &convert),
            move |_| errors.fail(STREAM_FAILED),
            None,
        )
        .context("Could not open microphone stream")
}

fn encode_wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    ensure!(
        sample_rate > 0 && sample_rate <= u32::MAX / 2,
        "Invalid WAV sample rate"
    );
    ensure!(
        samples.len() <= MAX_SAMPLE_BYTES / size_of::<i16>(),
        "Audio exceeds the WAV size limit"
    );
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(std::mem::size_of_val(samples) + 64)
        .context("Could not allocate WAV output")?;
    let mut cursor = Cursor::new(bytes);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::new(&mut cursor, spec).context("Could not create WAV header")?;
    for &sample in samples {
        writer
            .write_sample(sample)
            .context("Could not encode microphone audio")?;
    }
    writer
        .finalize()
        .context("Could not finalize microphone WAV")?;
    Ok(cursor.into_inner())
}

/// Plays a quiet completion tone. Call on a worker: this waits for playback to finish.
pub(crate) fn chime() -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .context("No default speaker is available for the completion sound")?;
    let supported = device
        .default_output_config()
        .context("Could not read speaker format")?;
    let config = supported.config();
    ensure!(
        config.channels > 0 && config.sample_rate.0 > 0,
        "Speaker reported an invalid audio configuration"
    );
    let done = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));
    let stream = match supported.sample_format() {
        SampleFormat::F32 => output_stream(
            &device,
            &config,
            Arc::clone(&done),
            Arc::clone(&failed),
            |s| s,
        ),
        SampleFormat::I16 => output_stream(
            &device,
            &config,
            Arc::clone(&done),
            Arc::clone(&failed),
            pcm_i16,
        ),
        SampleFormat::U16 => output_stream(
            &device,
            &config,
            Arc::clone(&done),
            Arc::clone(&failed),
            |s| (i32::from(pcm_i16(s)) + 32768) as u16,
        ),
        format => bail!("Unsupported speaker sample format: {format:?}"),
    }?;
    stream.play().context("Could not play completion sound")?;
    let started = Instant::now();
    while !done.load(Ordering::Relaxed) {
        ensure!(
            !failed.load(Ordering::Relaxed),
            "Speaker disconnected or completion sound failed"
        );
        ensure!(
            started.elapsed() < Duration::from_secs(2),
            "Completion sound playback timed out"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // The last generated frames may still be queued by the backend.
    thread::sleep(Duration::from_millis(150));
    drop(stream);
    ensure!(
        !failed.load(Ordering::Relaxed),
        "Completion sound playback failed"
    );
    Ok(())
}

fn output_stream<T: SizedSample + Copy>(
    device: &cpal::Device,
    config: &StreamConfig,
    done: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    convert: impl Fn(f32) -> T + Send + 'static,
) -> Result<cpal::Stream> {
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0 as f32;
    let length = (rate * CHIME_SECONDS) as usize;
    let mut position = 0_usize;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels) {
                    let value = if position < length {
                        let time = position as f32 / rate;
                        position += 1;
                        chime_sample(time)
                    } else {
                        done.store(true, Ordering::Relaxed);
                        0.0
                    };
                    frame.fill(convert(value));
                }
            },
            move |_| failed.store(true, Ordering::Relaxed),
            None,
        )
        .context("Could not open speaker for completion sound")
}

fn chime_sample(time: f32) -> f32 {
    if !(0.0..CHIME_SECONDS).contains(&time) {
        return 0.0;
    }
    let envelope = (time / 0.005).min(1.0) * ((CHIME_SECONDS - time) / 0.025).clamp(0.0, 1.0);
    (std::f32::consts::TAU * 880.0 * time).sin() * envelope * 0.12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_silent_after_one_hundred_milliseconds() {
        assert_eq!(chime_sample(0.10), 0.0);
        assert_eq!(chime_sample(0.18), 0.0);
        assert!(chime_sample(0.012).abs() > 0.01);
    }
    #[test]
    fn chime_has_soft_edges_and_bounded_volume() {
        assert_eq!(chime_sample(0.0), 0.0);
        assert!(chime_sample(0.09999).abs() < 0.001);
        assert!((0..4800).all(|frame| chime_sample(frame as f32 / 48000.0).abs() <= 0.12));
    }
    #[test]
    fn wav_round_trips_mono_pcm_at_native_rate() {
        let samples = [i16::MIN, -123, 0, 123, i16::MAX];
        let bytes = encode_wav(&samples, 48_000).expect("encode WAV");
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).expect("read WAV");
        assert_eq!(
            (
                reader.spec().channels,
                reader.spec().sample_rate,
                reader.spec().bits_per_sample
            ),
            (1, 48_000, 16)
        );
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .expect("read samples"),
            samples
        );
    }

    #[test]
    fn downmix_averages_channels_without_integer_overflow() {
        assert_eq!(
            downmix(&[i16::MAX; 64], |s| f32::from(s) / 32768.0),
            32767.0 / 32768.0
        );
    }

    #[test]
    fn downmix_sanitizes_nonfinite_and_clips_input() {
        assert_eq!(downmix(&[f32::NAN, f32::INFINITY, 4.0, -2.0], |s| s), 0.0);
    }

    #[test]
    fn unsigned_silence_is_centered() {
        assert_eq!(
            downmix(&[32768_u16, 32768], |s| (f32::from(s) - 32768.0) / 32768.0),
            0.0
        );
    }

    #[test]
    fn pcm_preserves_full_scale_endpoints() {
        assert_eq!(
            [pcm_i16(-1.0), pcm_i16(0.0), pcm_i16(1.0)],
            [i16::MIN, 0, i16::MAX]
        );
    }

    #[test]
    fn limits_choose_smaller_of_duration_and_memory() {
        assert_eq!(
            (sample_limit(48_000), sample_limit(192_000)),
            (14_400_000, 16_777_216)
        );
    }

    #[test]
    fn callback_stops_at_preallocated_capacity() {
        let capture = Capture::new(2).expect("allocate");
        capture.record(&[0.5_f32; 8], 2, |s| s);
        capture.record(&[1.0_f32; 8], 2, |s| s);
        assert!(capture.limit_reached());
        let samples = capture.samples.lock().expect("samples");
        assert_eq!(
            (&samples.data[..], samples.written),
            (&[16384, 16384][..], 2)
        );
    }

    #[test]
    fn callback_does_not_wait_on_contended_buffer() {
        let capture = Capture::new(2).expect("allocate");
        let _guard = capture.samples.lock().expect("hold buffer");
        capture.record(&[0.0_f32], 1, |s| s);
        assert_eq!(capture.failure.load(Ordering::Relaxed), BUFFER_FAILED);
    }

    #[test]
    fn callback_rejects_incomplete_frames() {
        let capture = Capture::new(2).expect("allocate");
        capture.record(&[0.0_f32], 2, |s| s);
        assert_eq!(capture.failure.load(Ordering::Relaxed), INVALID_FRAME);
    }

    #[test]
    fn callback_rejects_audio_after_wall_clock_limit() {
        let mut capture = Capture::new(2).expect("allocate");
        capture.started = Instant::now() - MAX_DURATION;
        capture.record(&[1.0_f32], 1, |s| s);
        assert_eq!(capture.samples.lock().expect("samples").written, 0);
        assert!(capture.limit_reached());
    }

    #[test]
    fn finish_retains_limit_reached_audio_for_explicit_submission() {
        let capture = Arc::new(Capture::new(1).expect("allocate"));
        capture.record(&[0.5_f32], 1, |s| s);
        let recording = Recording {
            stream: None,
            capture,
            sample_rate: 48_000,
        };
        assert!(recording.limit_reached());
        let captured = recording.finish().expect("retain bounded audio");
        assert!(captured.limited);
        let reader = hound::WavReader::new(Cursor::new(captured.wav)).expect("valid WAV");
        assert_eq!(reader.duration(), 1);
    }

    #[test]
    fn finish_refuses_failed_audio() {
        let capture = Arc::new(Capture::new(2).expect("allocate"));
        capture.record(&[0.5_f32], 1, |s| s);
        capture.fail(STREAM_FAILED);
        let recording = Recording {
            stream: None,
            capture,
            sample_rate: 48_000,
        };
        assert!(
            recording
                .finish()
                .err()
                .expect("failure must discard audio")
                .to_string()
                .contains("disconnected")
        );
    }

    #[test]
    fn stream_failure_prevents_further_capture() {
        let capture = Capture::new(2).expect("allocate");
        capture.fail(STREAM_FAILED);
        capture.record(&[1.0_f32], 1, |s| s);
        assert_eq!(capture.samples.lock().expect("samples").written, 0);
        assert!(capture.error().expect("failure").contains("disconnected"));
    }
}
