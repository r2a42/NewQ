//! Audio output: a cpal stream that mixes "voices", each fed by an ffmpeg
//! subprocess decoding to interleaved stereo f32 at the device sample rate.

use super::ffmpeg::Ffmpeg;
use super::video::spawn_stderr_drain;
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use std::io::Read;
use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Seconds of decoded audio buffered ahead of playback.
const BUFFER_SECONDS: usize = 2;

#[derive(Default)]
struct VoiceShared {
    gain_bits: AtomicU32,
    paused: AtomicBool,
    stopped: AtomicBool,
    eof: AtomicBool,
    drained: AtomicBool,
    played_frames: AtomicU64,
    received_frames: AtomicU64,
}

struct MixVoice {
    shared: Arc<VoiceShared>,
    consumer: rtrb::Consumer<f32>,
    gain: f32,
}

pub struct AudioOut {
    _stream: Option<cpal::Stream>,
    pub sample_rate: u32,
    pub device_name: String,
    pub error: Option<String>,
    voices_tx: Sender<MixVoice>,
}

impl AudioOut {
    pub fn new() -> AudioOut {
        let (voices_tx, voices_rx) = unbounded();
        match open_stream(voices_rx) {
            Ok((stream, sample_rate, device_name)) => AudioOut {
                _stream: Some(stream),
                sample_rate,
                device_name,
                error: None,
                voices_tx,
            },
            Err(e) => {
                log::error!("audio output unavailable: {e:#}");
                AudioOut {
                    _stream: None,
                    sample_rate: 48_000,
                    device_name: "none".into(),
                    error: Some(format!("{e:#}")),
                    voices_tx,
                }
            }
        }
    }

    /// An output with no device: voices can't be created (used by tests).
    #[cfg(test)]
    pub fn disabled() -> AudioOut {
        AudioOut {
            _stream: None,
            sample_rate: 48_000,
            device_name: "none".into(),
            error: Some("disabled".into()),
            voices_tx: unbounded().0,
        }
    }

    pub fn available(&self) -> bool {
        self._stream.is_some()
    }

    /// Start decoding `path`. The voice starts paused; call `set_paused(false)` to play.
    pub fn spawn_voice(
        &self,
        ff: &Ffmpeg,
        path: &Path,
        start: f64,
        duration: Option<f64>,
        looping: bool,
        gain: f32,
    ) -> Result<Voice> {
        if !self.available() {
            return Err(anyhow!("no audio output device"));
        }
        let sr = self.sample_rate;
        let mut cmd = ff.command();
        if looping {
            cmd.args(["-stream_loop", "-1"]);
        }
        if start > 0.0 {
            cmd.args(["-ss", &format!("{start:.3}")]);
        }
        cmd.arg("-i").arg(path);
        if let Some(d) = duration.filter(|_| !looping) {
            cmd.args(["-t", &format!("{d:.3}")]);
        }
        cmd.args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "2",
            "-ar",
            &sr.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ]);
        let mut child = cmd.spawn().context("starting ffmpeg for audio")?;
        let mut stdout = child.stdout.take().context("ffmpeg stdout")?;
        let error = Arc::new(Mutex::new(None));
        spawn_stderr_drain(child.stderr.take(), error.clone());

        let shared = Arc::new(VoiceShared::default());
        shared.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
        shared.paused.store(true, Ordering::Relaxed);
        let (mut producer, consumer) = rtrb::RingBuffer::new(sr as usize * 2 * BUFFER_SECONDS);

        let shared2 = shared.clone();
        std::thread::Builder::new()
            .name("audio-reader".into())
            .spawn(move || {
                let mut bytes = vec![0u8; 16 * 1024];
                let mut carry: Vec<u8> = Vec::with_capacity(8);
                'outer: loop {
                    let n = match stdout.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    carry.extend_from_slice(&bytes[..n]);
                    let whole = carry.len() / 8 * 8; // whole stereo frames
                    for frame in carry[..whole].as_chunks::<8>().0 {
                        for s in frame.as_chunks::<4>().0 {
                            let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                            while producer.push(v).is_err() {
                                if shared2.stopped.load(Ordering::Relaxed) {
                                    break 'outer;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                        shared2.received_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    carry.drain(..whole);
                }
                shared2.eof.store(true, Ordering::Release);
            })?;

        let _ = self.voices_tx.send(MixVoice {
            shared: shared.clone(),
            consumer,
            gain,
        });
        Ok(Voice {
            shared,
            child,
            sample_rate: sr,
            error,
        })
    }
}

/// Handle to a playing voice. Dropping it stops playback and kills ffmpeg.
pub struct Voice {
    shared: Arc<VoiceShared>,
    child: Child,
    sample_rate: u32,
    error: Arc<Mutex<Option<String>>>,
}

impl Voice {
    pub fn set_gain(&self, gain: f32) {
        self.shared
            .gain_bits
            .store(gain.to_bits(), Ordering::Relaxed);
    }
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }
    /// Seconds of audio actually sent to the device. This is the master clock
    /// for video cues that have sound.
    pub fn clock(&self) -> f64 {
        self.shared.played_frames.load(Ordering::Relaxed) as f64 / self.sample_rate as f64
    }
    /// Some audio has been decoded and is ready to play.
    pub fn ready(&self) -> bool {
        self.shared.received_frames.load(Ordering::Relaxed) > 0 || self.finished()
    }
    pub fn finished(&self) -> bool {
        self.shared.drained.load(Ordering::Acquire)
            || (self.shared.eof.load(Ordering::Acquire)
                && self.shared.received_frames.load(Ordering::Relaxed) == 0)
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().clone()
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn open_stream(voices_rx: Receiver<MixVoice>) -> Result<(cpal::Stream, u32, String)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default audio output device"))?;
    let mut name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "audio device".into());
    // NEWQ_MUTE=1 keeps the audio clock running but outputs silence (for testing).
    let master = if std::env::var_os("NEWQ_MUTE").is_some() {
        name.push_str(" (muted)");
        0.0
    } else {
        1.0
    };
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config = supported.config();
    let sr = config.sample_rate;
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, config, voices_rx, master)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, config, voices_rx, master)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, config, voices_rx, master)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, config, voices_rx, master)?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    stream.play()?;
    Ok((stream, sr, name))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    voices_rx: Receiver<MixVoice>,
    master: f32,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut voices: Vec<MixVoice> = Vec::with_capacity(64);
    let mut mix: Vec<f32> = Vec::new();
    let stream = device.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _| {
            while let Ok(v) = voices_rx.try_recv() {
                voices.push(v);
            }
            let frames = out.len() / channels;
            mix.clear();
            mix.resize(frames * 2, 0.0);
            voices.retain_mut(|v| mix_voice(v, &mut mix, frames));
            for (f, frame) in out.chunks_exact_mut(channels).enumerate() {
                let (l, r) = (mix[f * 2] * master, mix[f * 2 + 1] * master);
                match channels {
                    1 => frame[0] = T::from_sample(((l + r) * 0.5).clamp(-1.0, 1.0)),
                    _ => {
                        frame[0] = T::from_sample(l.clamp(-1.0, 1.0));
                        frame[1] = T::from_sample(r.clamp(-1.0, 1.0));
                        for s in &mut frame[2..] {
                            *s = T::from_sample(0.0f32);
                        }
                    }
                }
            }
        },
        |e| log::error!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Mix one voice into the stereo buffer. Returns false when the voice should be dropped.
fn mix_voice(v: &mut MixVoice, mix: &mut [f32], frames: usize) -> bool {
    let sh = &v.shared;
    if sh.stopped.load(Ordering::Relaxed) {
        return false;
    }
    if sh.paused.load(Ordering::Relaxed) {
        return true;
    }
    let available = v.consumer.slots() / 2;
    let n = available.min(frames);
    if n == 0 {
        if sh.eof.load(Ordering::Acquire) && v.consumer.is_empty() {
            sh.drained.store(true, Ordering::Release);
            return false;
        }
        return true;
    }
    // Ramp gain linearly across the buffer to avoid zipper noise.
    let target = f32::from_bits(sh.gain_bits.load(Ordering::Relaxed));
    let step = (target - v.gain) / n as f32;
    if let Ok(chunk) = v.consumer.read_chunk(n * 2) {
        let (a, b) = chunk.as_slices();
        for (i, s) in a.iter().chain(b.iter()).enumerate() {
            if i % 2 == 0 {
                v.gain += step;
            }
            mix[i] += s * v.gain;
        }
        chunk.commit_all();
    }
    v.gain = target;
    sh.played_frames.fetch_add(n as u64, Ordering::Relaxed);
    true
}
