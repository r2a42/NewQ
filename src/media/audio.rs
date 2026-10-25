//! Audio output: one cpal stream per audio device in use, each mixing
//! "voices" fed by ffmpeg subprocesses that decode to interleaved stereo f32
//! at that device's sample rate. A voice's stereo pair is routed to any
//! channels of its device.

use super::ffmpeg::Ffmpeg;
use super::playback::{Children, PassFeed, Playback};
use crate::show::{AudioOutputConfig, AudioOutputId};
use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
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
    /// The device channel the left channel plays on; the right is next.
    first_channel: usize,
}

/// The most device channels a stream is opened with.
const MAX_CHANNELS: u16 = 64;

/// One open audio device.
struct Device {
    /// Kept alive while the device is in use; dropping it stops the stream.
    _stream: Option<cpal::Stream>,
    /// Voices can be played on it (its stream is running).
    open: bool,
    sample_rate: u32,
    channels: u16,
    /// What the device calls itself.
    name: String,
    error: Option<String>,
    voices_tx: Sender<MixVoice>,
}

impl Device {
    /// Open the named device, or the system's default. A device that can't
    /// be opened is kept, with its error, so it can be shown and retried.
    fn open(name: Option<&str>) -> Device {
        let (voices_tx, voices_rx) = unbounded();
        match open_stream(name, voices_rx) {
            Ok((stream, sample_rate, channels, device)) => Device {
                _stream: Some(stream),
                open: true,
                sample_rate,
                channels,
                name: device,
                error: None,
                voices_tx,
            },
            Err(e) => {
                log::error!("audio output unavailable: {e:#}");
                Device {
                    _stream: None,
                    open: false,
                    sample_rate: 48_000,
                    channels: 2,
                    name: name.unwrap_or("default").to_string(),
                    error: Some(format!("{e:#}")),
                    voices_tx,
                }
            }
        }
    }
}

#[cfg(test)]
impl Device {
    /// A device with no stream behind it, whose voices the test mixes itself.
    fn for_test(channels: u16) -> (Device, Receiver<MixVoice>) {
        let (voices_tx, voices_rx) = unbounded();
        let device = Device {
            _stream: None,
            open: true,
            sample_rate: 48_000,
            channels,
            name: "test device".into(),
            error: None,
            voices_tx,
        };
        (device, voices_rx)
    }
}

/// What a device is doing, for the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceStatus {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub error: Option<String>,
}

pub struct AudioOut {
    /// Open devices, by the device name a show output asks for (None = the
    /// system default).
    devices: HashMap<Option<String>, Device>,
    /// The device each of the show's audio outputs is on.
    routes: HashMap<AudioOutputId, Option<String>>,
    /// The device for cues that don't choose an output: the first output's.
    default_key: Option<String>,
    /// The outputs the devices were last opened for.
    synced: Option<Vec<(AudioOutputId, Option<String>)>>,
}

impl AudioOut {
    /// No device is open until `sync` says which are wanted.
    pub fn new() -> AudioOut {
        AudioOut {
            devices: HashMap::new(),
            routes: HashMap::new(),
            default_key: None,
            synced: None,
        }
    }

    /// An output with no device: voices can't be created (used by tests).
    #[cfg(test)]
    pub fn disabled() -> AudioOut {
        AudioOut::new()
    }

    /// A default device that works, and `broken` routed to one that failed to
    /// open (an unplugged interface), for tests.
    #[cfg(test)]
    pub fn with_broken_output(broken: AudioOutputId) -> AudioOut {
        let (good, _) = Device::for_test(2);
        let (mut bad, _) = Device::for_test(2);
        bad.open = false;
        bad.error = Some("the interface was unplugged".into());
        let mut out = AudioOut::new();
        out.devices.insert(None, good);
        out.devices.insert(Some("usb".into()), bad);
        out.routes.insert(broken, Some("usb".into()));
        out
    }

    /// Open the devices the show's audio outputs use (the system default if
    /// it has none), and close those no longer used. Cheap when nothing changed.
    pub fn sync(&mut self, outputs: &[AudioOutputConfig]) {
        let key: Vec<_> = outputs.iter().map(|o| (o.id, o.device.clone())).collect();
        if self.synced.as_ref() == Some(&key) {
            return;
        }
        self.default_key = outputs.first().and_then(|o| o.device.clone());
        let mut wanted: HashSet<Option<String>> =
            outputs.iter().map(|o| o.device.clone()).collect();
        wanted.insert(self.default_key.clone());
        self.devices.retain(|k, _| wanted.contains(k));
        for k in wanted {
            self.devices
                .entry(k.clone())
                .or_insert_with(|| Device::open(k.as_deref()));
        }
        self.routes = key.iter().cloned().collect();
        self.synced = Some(key);
    }

    /// Try again to open devices that failed (a USB interface plugged in later).
    pub fn retry_failed(&mut self) {
        for (k, d) in &mut self.devices {
            if d.error.is_some() {
                *d = Device::open(k.as_deref());
            }
        }
    }

    /// The device a cue's output name resolves to.
    fn device_for(&self, output: Option<AudioOutputId>) -> Option<&Device> {
        let key = output
            .and_then(|id| self.routes.get(&id))
            .unwrap_or(&self.default_key);
        self.devices.get(key)
    }

    /// Whether any device is open to play on.
    pub fn available(&self) -> bool {
        self.devices.values().any(|d| d.open)
    }

    /// The state of the device behind an audio output (None = the default).
    pub fn device_status(&self, output: Option<AudioOutputId>) -> Option<DeviceStatus> {
        self.device_for(output).map(|d| DeviceStatus {
            name: d.name.clone(),
            sample_rate: d.sample_rate,
            channels: d.channels,
            error: d.error.clone(),
        })
    }

    /// One line for the status bar: Ok with what is playing out, or Err with why not.
    pub fn summary(&self) -> Result<String, String> {
        match self.device_for(None) {
            None => Ok("starting audio…".into()),
            Some(d) => match &d.error {
                Some(e) => Err(e.clone()),
                None => {
                    let more = self.devices.len().saturating_sub(1);
                    let extra = if more > 0 {
                        format!(" (+{more} more)")
                    } else {
                        String::new()
                    };
                    Ok(format!("{} @ {} Hz{extra}", d.name, d.sample_rate))
                }
            },
        }
    }

    /// Start decoding `path`. The voice starts paused; call `set_paused(false)` to play.
    pub fn spawn_voice(
        &self,
        ff: &Ffmpeg,
        path: &Path,
        start: f64,
        playback: Playback,
        gain: f32,
        route: (Option<AudioOutputId>, u16),
    ) -> Result<Voice> {
        let device = self
            .device_for(route.0)
            .ok_or_else(|| anyhow!("no audio output device"))?;
        if !device.open {
            return Err(anyhow!(
                "{}",
                device.error.as_deref().unwrap_or("no audio output device")
            ));
        }
        let sr = device.sample_rate;
        let make = {
            let (ff, path) = (ff.clone(), path.to_path_buf());
            move || {
                let mut cmd = ff.command();
                cmd.args(playback.input_args(start));
                cmd.arg("-i").arg(&path);
                cmd.args(playback.limit_args());
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
                cmd
            }
        };
        let children = Children::new();
        let error = Arc::new(Mutex::new(None));
        let mut feed = PassFeed::start(&children, playback.passes(), &error, make)?;
        // Looping passes are cut or padded to an exact length, so repeats
        // don't drift and stay locked to the video's frame count.
        let per_pass = playback
            .pass_len()
            .map(|l| (l * sr as f64).round().max(1.0) as u64);

        let shared = Arc::new(VoiceShared::default());
        shared.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
        shared.paused.store(true, Ordering::Relaxed);
        let (mut producer, consumer) = rtrb::RingBuffer::new(sr as usize * 2 * BUFFER_SECONDS);

        let shared2 = shared.clone();
        std::thread::Builder::new()
            .name("audio-reader".into())
            .spawn(move || {
                // Queue one stereo frame; false if the voice was stopped meanwhile.
                let mut push = |frame: [f32; 2]| -> bool {
                    for v in frame {
                        while producer.push(v).is_err() {
                            if shared2.stopped.load(Ordering::Relaxed) {
                                return false;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    shared2.received_frames.fetch_add(1, Ordering::Relaxed);
                    true
                };
                let mut bytes = vec![0u8; 16 * 1024];
                let mut carry: Vec<u8> = Vec::with_capacity(8);
                'outer: loop {
                    let mut n: u64 = 0; // stereo frames queued this pass
                    carry.clear();
                    'pass: loop {
                        let read = match feed.stdout().read(&mut bytes) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => read,
                        };
                        carry.extend_from_slice(&bytes[..read]);
                        let whole = carry.len() / 8 * 8; // whole stereo frames
                        for frame in carry[..whole].as_chunks::<8>().0 {
                            if per_pass.is_some_and(|p| n >= p) {
                                break 'pass;
                            }
                            if per_pass.is_some_and(|p| n * 2 >= p) {
                                feed.prefetch();
                            }
                            let f32_at = |i: usize| {
                                f32::from_le_bytes([
                                    frame[i],
                                    frame[i + 1],
                                    frame[i + 2],
                                    frame[i + 3],
                                ])
                            };
                            if !push([f32_at(0), f32_at(4)]) {
                                break 'outer;
                            }
                            n += 1;
                        }
                        carry.drain(..whole);
                    }
                    let decoded = n;
                    if let Some(p) = per_pass {
                        while n < p {
                            if !push([0.0; 2]) {
                                break 'outer;
                            }
                            n += 1;
                        }
                    }
                    // A pass that decodes nothing is an error, not a reason to retry forever.
                    if decoded == 0 || !feed.advance() {
                        break;
                    }
                }
                shared2.eof.store(true, Ordering::Release);
            })?;

        let _ = device.voices_tx.send(MixVoice {
            shared: shared.clone(),
            consumer,
            gain,
            first_channel: route.1 as usize,
        });
        Ok(Voice {
            shared,
            children,
            sample_rate: sr,
            error,
        })
    }
}

/// Handle to a playing voice. Dropping it stops playback and kills ffmpeg.
pub struct Voice {
    shared: Arc<VoiceShared>,
    children: Children,
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
        self.children.close();
    }
}

/// Names of the audio output devices that are connected now.
pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|d| d.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The device called `name` (the default device for None).
fn find_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default audio output device")),
        Some(n) => host
            .output_devices()?
            .find(|d| d.description().is_ok_and(|d| d.to_string() == n))
            .ok_or_else(|| anyhow!("audio device “{n}” is not connected")),
    }
}

/// The device's default configuration, widened to the most channels it offers
/// at the same sample rate, so a cue can be sent to any of them.
fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let default = device.default_output_config()?;
    let rate = default.sample_rate();
    let widest = device
        .supported_output_configs()
        .ok()
        .into_iter()
        .flatten()
        .filter(|c| c.channels() <= MAX_CHANNELS)
        .filter_map(|c| c.try_with_sample_rate(rate))
        .filter(|c| {
            matches!(
                c.sample_format(),
                cpal::SampleFormat::F32
                    | cpal::SampleFormat::I16
                    | cpal::SampleFormat::U16
                    | cpal::SampleFormat::I32
            )
        })
        .max_by_key(|c| c.channels());
    Ok(match widest {
        Some(w) if w.channels() > default.channels() => w,
        _ => default,
    })
}

fn open_stream(
    name: Option<&str>,
    voices_rx: Receiver<MixVoice>,
) -> Result<(cpal::Stream, u32, u16, String)> {
    let device = find_device(name)?;
    let mut device_name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "audio device".into());
    // NEWQ_MUTE=1 keeps the audio clock running but outputs silence (for testing).
    let master = if std::env::var_os("NEWQ_MUTE").is_some() {
        device_name.push_str(" (muted)");
        0.0
    } else {
        1.0
    };
    let supported = pick_config(&device)?;
    let format = supported.sample_format();
    let config = supported.config();
    let (sr, channels) = (config.sample_rate, config.channels);
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, config, voices_rx, master)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, config, voices_rx, master)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, config, voices_rx, master)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, config, voices_rx, master)?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    stream.play()?;
    Ok((stream, sr, channels, device_name))
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
            mix.resize(frames * channels, 0.0);
            voices.retain_mut(|v| mix_voice(v, &mut mix, frames, channels));
            for (s, m) in out.iter_mut().zip(&mix) {
                *s = T::from_sample((m * master).clamp(-1.0, 1.0));
            }
        },
        |e| log::error!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Add one stereo frame to a device frame, the left channel on channel
/// `first` and the right on the next. With one channel left to play on, the
/// two are summed to mono; with none, the frame is dropped.
fn route_frame(frame: &mut [f32], first: usize, l: f32, r: f32) {
    match frame.len().saturating_sub(first) {
        0 => {}
        1 => frame[first] += (l + r) * 0.5,
        _ => {
            frame[first] += l;
            frame[first + 1] += r;
        }
    }
}

/// Mix one voice into the interleaved `channels`-wide buffer. Returns false
/// when the voice should be dropped.
fn mix_voice(v: &mut MixVoice, mix: &mut [f32], frames: usize, channels: usize) -> bool {
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
        let mut left = 0.0;
        for (i, s) in a.iter().chain(b.iter()).enumerate() {
            if i % 2 == 0 {
                v.gain += step;
                left = s * v.gain;
            } else {
                let f = i / 2;
                route_frame(
                    &mut mix[f * channels..(f + 1) * channels],
                    v.first_channel,
                    left,
                    s * v.gain,
                );
            }
        }
        chunk.commit_all();
    }
    v.gain = target;
    sh.played_frames.fetch_add(n as u64, Ordering::Relaxed);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_routed_to_the_chosen_channels() {
        let mut f = [0.0; 4];
        route_frame(&mut f, 0, 0.5, 0.25);
        assert_eq!(f, [0.5, 0.25, 0.0, 0.0]);
        let mut f = [0.0; 4];
        route_frame(&mut f, 2, 0.5, 0.25);
        assert_eq!(f, [0.0, 0.0, 0.5, 0.25]);
        // Routes add, so voices on the same channels mix.
        route_frame(&mut f, 2, 0.5, 0.25);
        assert_eq!(f, [0.0, 0.0, 1.0, 0.5]);
        // One channel left: the pair is summed to mono.
        let mut f = [0.0; 3];
        route_frame(&mut f, 2, 0.5, 0.25);
        assert_eq!(f, [0.0, 0.0, 0.375]);
        // A one-channel device gets the mono mix, as before.
        let mut f = [0.0; 1];
        route_frame(&mut f, 0, 1.0, 0.0);
        assert_eq!(f, [0.5]);
        // Past the end of the device nothing plays and nothing breaks.
        let mut f = [0.0; 2];
        route_frame(&mut f, 2, 1.0, 1.0);
        route_frame(&mut f, 9, 1.0, 1.0);
        assert_eq!(f, [0.0, 0.0]);
    }

    /// A voice with `samples` queued (interleaved L R), playing at unity gain.
    fn voice(samples: &[f32], first_channel: usize) -> MixVoice {
        let (mut producer, consumer) = rtrb::RingBuffer::new(samples.len().max(2));
        for s in samples {
            producer.push(*s).unwrap();
        }
        let shared = Arc::new(VoiceShared::default());
        shared.gain_bits.store(1.0f32.to_bits(), Ordering::Relaxed);
        MixVoice {
            shared,
            consumer,
            gain: 1.0,
            first_channel,
        }
    }

    #[test]
    fn a_voice_mixes_onto_its_channels_of_a_wide_device() {
        let mut v = voice(&[1.0, -1.0, 0.5, -0.5], 2);
        let mut mix = vec![0.0; 3 * 4];
        assert!(mix_voice(&mut v, &mut mix, 3, 4));
        // Two frames played on channels 3 and 4 (index 2 and 3); the third is silent.
        assert_eq!(
            mix,
            [0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.5, -0.5, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(v.shared.played_frames.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn two_voices_on_different_channels_do_not_mix_together() {
        let mut a = voice(&[1.0, 1.0], 0);
        let mut b = voice(&[0.25, 0.25], 2);
        let mut mix = vec![0.0; 4];
        mix_voice(&mut a, &mut mix, 1, 4);
        mix_voice(&mut b, &mut mix, 1, 4);
        assert_eq!(mix, [1.0, 1.0, 0.25, 0.25]);
    }

    #[test]
    fn gain_applies_and_ramps_across_the_buffer() {
        let mut v = voice(&[1.0, 1.0, 1.0, 1.0], 0);
        v.gain = 0.0;
        let mut mix = vec![0.0; 4];
        mix_voice(&mut v, &mut mix, 2, 2);
        // From silence up to unity across two frames.
        assert!(
            (mix[0] - 0.5).abs() < 1e-6 && (mix[2] - 1.0).abs() < 1e-6,
            "{mix:?}"
        );
        assert_eq!(v.gain, 1.0);
    }

    #[test]
    fn paused_and_stopped_voices() {
        let mut v = voice(&[1.0, 1.0], 0);
        v.shared.paused.store(true, Ordering::Relaxed);
        let mut mix = vec![0.0; 2];
        assert!(mix_voice(&mut v, &mut mix, 1, 2), "a paused voice is kept");
        assert_eq!(mix, [0.0, 0.0]);
        v.shared.stopped.store(true, Ordering::Relaxed);
        assert!(
            !mix_voice(&mut v, &mut mix, 1, 2),
            "a stopped voice is dropped"
        );
    }

    #[test]
    fn a_finished_voice_reports_drained_once_its_audio_is_played() {
        let mut v = voice(&[0.5, 0.5], 0);
        v.shared.eof.store(true, Ordering::Release);
        let mut mix = vec![0.0; 2];
        assert!(mix_voice(&mut v, &mut mix, 1, 2));
        assert!(!v.shared.drained.load(Ordering::Acquire));
        assert!(!mix_voice(&mut v, &mut mix, 1, 2));
        assert!(v.shared.drained.load(Ordering::Acquire));
    }

    fn output(id: u64, device: Option<&str>) -> AudioOutputConfig {
        AudioOutputConfig {
            id: AudioOutputId(id),
            name: format!("Out {id}"),
            device: device.map(String::from),
        }
    }

    #[test]
    fn a_missing_device_is_reported_not_fatal() {
        let mut audio = AudioOut::new();
        assert!(!audio.available());
        audio.sync(&[output(1, Some("No Such Interface"))]);
        let status = audio.device_status(Some(AudioOutputId(1))).unwrap();
        let err = status.error.clone().expect("the device can't be there");
        assert!(err.contains("No Such Interface"), "{err}");
        assert!(!audio.available());
        assert!(audio.summary().is_err());
        // Cues that didn't pick an output use the first one's device, so they fail the same way.
        assert_eq!(audio.device_status(None), Some(status));
        // An output that isn't in the show falls back to the default too.
        assert!(audio.device_status(Some(AudioOutputId(99))).is_some());
        // Retrying doesn't panic or lose the device.
        audio.retry_failed();
        assert!(
            audio
                .device_status(Some(AudioOutputId(1)))
                .unwrap()
                .error
                .is_some()
        );
    }

    #[test]
    fn syncing_opens_one_device_per_name_and_closes_unused_ones() {
        let mut audio = AudioOut::new();
        audio.sync(&[
            output(1, Some("Missing A")),
            output(2, Some("Missing A")),
            output(3, Some("Missing B")),
        ]);
        assert_eq!(audio.devices.len(), 2, "outputs on one device share it");
        assert_eq!(audio.default_key, Some("Missing A".into()));
        audio.sync(&[output(3, Some("Missing B"))]);
        assert_eq!(audio.devices.len(), 1);
        assert_eq!(audio.default_key, Some("Missing B".into()));
        // The same outputs again change nothing (names aren't part of the key).
        let mut renamed = output(3, Some("Missing B"));
        renamed.name = "Renamed".into();
        audio.sync(&[renamed]);
        assert_eq!(audio.devices.len(), 1);
        // No outputs means the system default device, whether or not one exists here.
        audio.sync(&[]);
        assert_eq!(
            audio.devices.keys().cloned().collect::<Vec<_>>(),
            vec![None]
        );
    }

    /// Wait for a voice to have decoded `frames` frames.
    fn wait_for(v: &MixVoice, frames: usize) {
        let t0 = std::time::Instant::now();
        while v.consumer.slots() < frames * 2 {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "voice never decoded"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn peak(mix: &[f32], channels: usize, c: usize) -> f32 {
        mix.iter()
            .skip(c)
            .step_by(channels)
            .fold(0.0, |a, b| a.max(b.abs()))
    }

    #[test]
    fn a_decoded_file_plays_only_on_its_routed_channels() {
        let Some((ff, wav)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        let (device, rx) = Device::for_test(6);
        let mut audio = AudioOut::new();
        audio.devices.insert(None, device);
        let voice = audio
            .spawn_voice(
                &ff,
                &wav,
                0.0,
                Playback::Once {
                    duration: Some(1.0),
                },
                1.0,
                (None, 3),
            )
            .unwrap();
        voice.set_paused(false);
        let mut mv = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(mv.first_channel, 3);
        wait_for(&mv, 4800);
        let mut mix = vec![0.0; 4800 * 6];
        assert!(mix_voice(&mut mv, &mut mix, 4800, 6));
        assert!(
            peak(&mix, 6, 3) > 0.05 && peak(&mix, 6, 4) > 0.05,
            "no sound on channels 4 and 5"
        );
        for silent in [0, 1, 2, 5] {
            assert_eq!(
                peak(&mix, 6, silent),
                0.0,
                "sound leaked onto channel {}",
                silent + 1
            );
        }
        // Channel 0 puts it on the first pair, as it always did.
        let (device, rx) = Device::for_test(6);
        audio.devices.insert(None, device);
        let voice = audio
            .spawn_voice(
                &ff,
                &wav,
                0.0,
                Playback::Once {
                    duration: Some(1.0),
                },
                1.0,
                (None, 0),
            )
            .unwrap();
        voice.set_paused(false);
        let mut mv = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        wait_for(&mv, 4800);
        let mut mix = vec![0.0; 4800 * 6];
        mix_voice(&mut mv, &mut mix, 4800, 6);
        assert!(peak(&mix, 6, 0) > 0.05 && peak(&mix, 6, 1) > 0.05);
        assert_eq!(peak(&mix, 6, 2), 0.0);
    }

    #[test]
    fn outputs_on_different_devices_play_on_their_own_device() {
        let Some((ff, wav)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        let (a, rx_a) = Device::for_test(2);
        let (b, rx_b) = Device::for_test(2);
        let mut audio = AudioOut::new();
        audio.devices.insert(Some("A".into()), a);
        audio.devices.insert(Some("B".into()), b);
        audio.routes.insert(AudioOutputId(1), Some("A".into()));
        audio.routes.insert(AudioOutputId(2), Some("B".into()));
        audio.default_key = Some("A".into());
        let play = |output| {
            audio
                .spawn_voice(
                    &ff,
                    &wav,
                    0.0,
                    Playback::Once {
                        duration: Some(0.5),
                    },
                    1.0,
                    (output, 0),
                )
                .unwrap()
        };
        let _v1 = play(Some(AudioOutputId(2)));
        assert!(rx_b.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(rx_a.try_recv().is_err(), "output 2 played on device A");
        // Cues that don't choose, and unknown outputs, use the first output's device.
        let _v2 = play(None);
        let _v3 = play(Some(AudioOutputId(77)));
        assert!(rx_a.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(rx_a.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(rx_b.try_recv().is_err());
    }

    #[test]
    fn spawning_on_a_missing_device_explains_why() {
        let mut audio = AudioOut::new();
        audio.sync(&[output(1, Some("Missing Interface"))]);
        let ff = crate::media::ffmpeg::Ffmpeg {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        };
        let err = audio
            .spawn_voice(
                &ff,
                std::path::Path::new("x.wav"),
                0.0,
                Playback::Once { duration: None },
                1.0,
                (Some(AudioOutputId(1)), 0),
            )
            .err()
            .expect("no device to play on");
        assert!(err.to_string().contains("Missing Interface"), "{err}");
    }
}
