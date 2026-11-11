//! Sending a display's picture out of NewQ as a video stream: to a virtual
//! camera (Linux, through v4l2loopback), over the network, or wherever an
//! FFmpeg output can go. Frames are handed to an FFmpeg process on its
//! standard input and it does the rest.
//!
//! Windows and macOS have no virtual camera NewQ can make without a signed
//! driver or system extension; the network stream goes to OBS, whose own
//! virtual camera Zoom and Teams can use.

use crate::media::ffmpeg::Ffmpeg;
use crate::show::{ServerProtocol, StreamKind};
use std::io::Write;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::Arc;
use std::sync::OnceLock;

/// The size to send a picture of `canvas` at, `height` tall, with the same
/// shape and even sides (video wants that).
pub fn size_for(canvas: (u32, u32), height: u32) -> (u32, u32) {
    let mut height = height.clamp(16, 4320) & !1;
    let mut width =
        (canvas.0.max(1) as f64 * height as f64 / canvas.1.max(1) as f64).round() as u32;
    // Hardware H.264 encoders stop at 4096 wide: a very wide canvas is sent smaller.
    if width > 4096 {
        height = ((height as f64 * 4096.0 / width as f64) as u32).max(16) & !1;
        width = 4096;
    }
    (width.clamp(16, 4096) & !1, height)
}

/// Split a line of arguments the way a shell would, to the extent that
/// double and single quotes keep words together.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if started || !current.is_empty() {
        args.push(current);
    }
    args
}

/// What goes before `-i` and after, for frames of `size` at `fps`.
pub fn input_args(size: (u32, u32), fps: u32) -> Vec<String> {
    [
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        &format!("{}x{}", size.0, size.1),
        "-framerate",
        &fps.to_string(),
        "-i",
        "pipe:0",
    ]
    .map(String::from)
    .to_vec()
}

/// How long a stream's settings must stay as they are before they are acted
/// on, so typing an address doesn't start a stream for each letter.
pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(1200);

/// Hold back what has changed lately. `seen` is what each key last wanted and
/// since when; a `wanted` entry that hasn't stayed the same for `SETTLE` is put
/// back to what `running` has (so it goes on as it was) or, with nothing
/// running, dropped (so it isn't started yet).
pub fn settle<K, V>(
    seen: &mut std::collections::HashMap<K, (V, std::time::Instant)>,
    wanted: &mut std::collections::HashMap<K, V>,
    running: &std::collections::HashMap<K, V>,
    now: std::time::Instant,
) where
    K: std::hash::Hash + Eq + Copy,
    V: Clone + PartialEq,
{
    seen.retain(|k, _| wanted.contains_key(k));
    let keys: Vec<K> = wanted.keys().copied().collect();
    for k in keys {
        let value = wanted[&k].clone();
        let entry = seen.entry(k).or_insert_with(|| (value.clone(), now));
        if entry.0 != value {
            *entry = (value, now);
        }
        if now.duration_since(entry.1) < SETTLE {
            match running.get(&k) {
                Some(r) => {
                    wanted.insert(k, r.clone());
                }
                None => {
                    wanted.remove(&k);
                }
            }
        }
    }
}

/// A server stream that ran this long without stopping starts its retries over.
pub const STEADY: std::time::Duration = std::time::Duration::from_secs(30);

/// How long to wait before starting a server stream again after it stopped
/// `failures` times in a row: 2 s, then longer, to 30 s.
pub fn retry_delay(failures: u32) -> std::time::Duration {
    std::time::Duration::from_secs(match failures {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        4 => 15,
        _ => 30,
    })
}

/// H.264 encoders to try, best first: the software one, then each platform's
/// hardware encoder (VideoToolbox on a Mac, then NVIDIA, Intel and AMD).
const H264_ENCODERS: [&str; 6] = [
    "libx264",
    "h264_videotoolbox",
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    // Cisco's, which an LGPL build (NewQ's Windows and Linux ones have no libx264) can have.
    "libopenh264",
];

/// The arguments that make H.264 with `encoder` at about `kbps`, with a
/// keyframe every two seconds so a viewer joining late waits no longer.
pub fn h264_args(encoder: &str, kbps: u32, fps: u32) -> Vec<String> {
    let kbps = kbps.clamp(200, 100_000);
    let rate = format!("{kbps}k");
    let buf = format!("{}k", kbps * 2);
    let gop = (fps.max(1) * 2).to_string();
    let mut a: Vec<String> = ["-c:v", encoder].map(String::from).to_vec();
    let mut add = |more: &[&str]| a.extend(more.iter().map(|s| s.to_string()));
    match encoder {
        "libx264" => add(&[
            "-preset",
            "veryfast",
            "-tune",
            "zerolatency",
            "-pix_fmt",
            "yuv420p",
        ]),
        "h264_videotoolbox" => add(&[
            "-realtime",
            "1",
            "-profile:v",
            "main",
            "-pix_fmt",
            "yuv420p",
        ]),
        "h264_nvenc" => add(&["-preset", "p4", "-tune", "ll", "-pix_fmt", "yuv420p"]),
        "h264_qsv" => add(&["-preset", "veryfast", "-pix_fmt", "nv12"]),
        // Not `lowlatency`/`ultralowlatency`: those ignore `-g` and send ONE keyframe at the
        // start (300 frames, 1 keyframe, FFmpeg 9 on a Radeon 780M), so a viewer who joins
        // later, or whose connection drops, never gets a picture. `webcam` is still low
        // latency and keeps the keyframe every two seconds.
        "h264_amf" => add(&["-usage", "webcam", "-pix_fmt", "yuv420p"]),
        "libopenh264" => add(&["-allow_skip_frames", "0", "-pix_fmt", "yuv420p"]),
        _ => add(&["-pix_fmt", "yuv420p"]),
    }
    // No B-frames: they reorder frames, which a browser's WebRTC player
    // (MediaMTX's web player) can't show, and some hardware encoders use them
    // unless told not to.
    add(&[
        "-bf", "0", "-b:v", &rate, "-maxrate", &rate, "-bufsize", &buf, "-g", &gop,
    ]);
    a
}

/// H.265 encoders to try, best first, as for `H264_ENCODERS`.
const HEVC_ENCODERS: [&str; 5] = [
    "libx265",
    "hevc_videotoolbox",
    "hevc_nvenc",
    "hevc_qsv",
    "hevc_amf",
];

/// The arguments that make H.265 with `encoder` at about `kbps`, for a file
/// (so, unlike `h264_args`, nothing is tuned for latency).
pub fn hevc_args(encoder: &str, kbps: u32, fps: u32) -> Vec<String> {
    let kbps = kbps.clamp(200, 200_000);
    let rate = format!("{kbps}k");
    let buf = format!("{}k", kbps * 2);
    let gop = (fps.max(1) * 2).to_string();
    let mut a: Vec<String> = ["-c:v", encoder].map(String::from).to_vec();
    let mut add = |more: &[&str]| a.extend(more.iter().map(|s| s.to_string()));
    match encoder {
        "libx265" => add(&["-preset", "veryfast", "-pix_fmt", "yuv420p"]),
        "hevc_videotoolbox" => add(&["-pix_fmt", "yuv420p"]),
        "hevc_nvenc" => add(&["-preset", "p4", "-pix_fmt", "yuv420p"]),
        "hevc_qsv" => add(&["-preset", "veryfast", "-pix_fmt", "nv12"]),
        "hevc_amf" => add(&["-usage", "transcoding", "-pix_fmt", "yuv420p"]),
        _ => add(&["-pix_fmt", "yuv420p"]),
    }
    // `hvc1` is the tag QuickTime and Apple's players need to open H.265.
    add(&[
        "-tag:v", "hvc1", "-b:v", &rate, "-maxrate", &rate, "-bufsize", &buf, "-g", &gop,
    ]);
    a
}

/// Where a recording is written: the folder (Downloads if empty), and a name
/// from the day and time it started.
pub fn record_path(
    folder: &str,
    container: crate::show::RecordContainer,
    started: &jiff::Zoned,
) -> std::path::PathBuf {
    let folder = folder.trim();
    let dir = if folder.is_empty() {
        default_record_folder()
    } else {
        std::path::PathBuf::from(folder)
    };
    dir.join(format!(
        "NewQ {}.{}",
        started.strftime("%Y-%m-%d %H.%M.%S"),
        container.extension()
    ))
}

/// `h264_args` for a file: the same encoder settings without what a live viewer
/// needs (zero-latency tuning, no B-frames), which cost a recording quality.
pub fn record_h264_args(encoder: &str, kbps: u32, fps: u32) -> Vec<String> {
    let mut a = h264_args(encoder, kbps, fps);
    for (key, value) in [("-tune", None), ("-bf", Some("0")), ("-realtime", None)] {
        while let Some(at) = a.iter().position(|x| x == key) {
            // A flag with a value, unless it only goes when the value is `value`.
            if value.is_some_and(|v| a.get(at + 1).map(String::as_str) != Some(v)) {
                break;
            }
            a.drain(at..(at + 2).min(a.len()));
        }
    }
    a
}

/// Make the file a recording is written to: in its folder (made if it isn't
/// there), named by the time it started, with ` (2)`, ` (3)` after it if that
/// name is taken, so two recordings that start together keep to their own files.
/// The empty file is left for FFmpeg to write over.
pub fn reserve_record_file(
    folder: &str,
    container: crate::show::RecordContainer,
) -> Result<std::path::PathBuf, String> {
    let first = record_path(folder, container, &jiff::Zoned::now());
    if let Some(dir) = first.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not make the folder {}: {e}", dir.display()))?;
    }
    for n in 1..100 {
        let path = if n == 1 {
            first.clone()
        } else {
            first.with_file_name(format!(
                "{} ({n}).{}",
                first.file_stem().unwrap_or_default().to_string_lossy(),
                container.extension()
            ))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not make {}: {e}", path.display())),
        }
    }
    Err("too many recordings with the same name".into())
}

/// The Downloads folder of whoever is using this computer.
pub fn default_record_folder() -> std::path::PathBuf {
    crate::relink::home_dir()
        .map(|h| h.join("Downloads"))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// FFmpeg's `-vf crop` for `area` of the picture, with even sides (encoders
/// want that) worked out from the picture's own size.
pub fn crop_filter(area: crate::show::RecordArea) -> String {
    let a = area.clamped();
    format!(
        "crop=trunc(iw*{:.4}/2)*2:trunc(ih*{:.4}/2)*2:trunc(iw*{:.4}/2)*2:trunc(ih*{:.4}/2)*2",
        a.w, a.h, a.x, a.y
    )
}

/// Where a server stream goes, as the URL FFmpeg publishes to.
pub fn server_url(
    protocol: ServerProtocol,
    host: &str,
    port: u16,
    path: &str,
) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("no address for the streaming server".into());
    }
    let path = path.trim().trim_matches('/');
    if path.is_empty() {
        return Err("no stream name (the path on the server)".into());
    }
    if path
        .chars()
        .any(|c| c.is_whitespace() || "?#&:@".contains(c))
    {
        return Err("the stream name can't have spaces or any of ? # & : @".into());
    }
    let host = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else if host
        .chars()
        .any(|c| c.is_whitespace() || ":/@?#[]".contains(c))
    {
        return Err("the server address is a name or number only: the port has its own box".into());
    } else {
        host.to_string()
    };
    let port = if port == 0 {
        protocol.default_port()
    } else {
        port
    };
    Ok(match protocol {
        ServerProtocol::Rtsp => format!("rtsp://{host}:{port}/{path}"),
        ServerProtocol::Rtmp => format!("rtmp://{host}:{port}/{path}"),
        // MediaMTX takes the path from the stream id.
        ServerProtocol::Srt => {
            format!("srt://{host}:{port}?streamid=publish:{path}&pkt_size=1316")
        }
    })
}

/// FFmpeg's output arguments for `kind`, for a picture at `fps`. `h264` is the
/// H.264 encoder this FFmpeg has and that works on this computer (`encoder`),
/// which the network and server streams use.
#[cfg_attr(not(test), allow(dead_code))]
pub fn output_args(kind: &StreamKind, h264: Option<&str>, fps: u32) -> Result<Vec<String>, String> {
    output_args_to(kind, h264, fps, None)
}

/// `output_args`, with the file a recording is written to when it has been chosen
/// already (see `reserve_record_file`).
pub fn output_args_to(
    kind: &StreamKind,
    h264: Option<&str>,
    fps: u32,
    record_file: Option<&std::path::Path>,
) -> Result<Vec<String>, String> {
    let list = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match kind {
        StreamKind::VirtualCamera { device } => {
            if !cfg!(target_os = "linux") {
                return Err(
                    "A virtual camera is only made on Linux: on Windows and macOS send the \
                     stream to OBS and use its virtual camera"
                        .into(),
                );
            }
            let device = device.trim();
            if device.is_empty() {
                return Err("no virtual camera chosen".into());
            }
            let mut a = list(&["-pix_fmt", "yuyv422", "-f", "v4l2"]);
            a.push(device.to_string());
            Ok(a)
        }
        StreamKind::Udp { host, port } => {
            let host = host.trim();
            if host.is_empty() {
                return Err("no address to send to".into());
            }
            let mut a = match h264 {
                Some(enc) => h264_args(enc, 4000, fps),
                None => list(&["-c:v", "mpeg4", "-q:v", "3", "-pix_fmt", "yuv420p"]),
            };
            a.extend(list(&["-f", "mpegts"]));
            a.push(format!("udp://{host}:{port}?pkt_size=1316"));
            Ok(a)
        }
        StreamKind::Server {
            protocol,
            host,
            port,
            path,
            kbps,
            adapter,
            host_here,
            ..
        } => {
            // A server NewQ runs is on this computer, whatever the box says.
            let host = if *host_here {
                "127.0.0.1"
            } else {
                host.as_str()
            };
            let host =
                crate::streamlink::publish_host(host, adapter, &crate::streamlink::adapters());
            let url = server_url(*protocol, &host, *port, path)?;
            let Some(enc) = h264 else {
                return Err(NO_H264.into());
            };
            let mut a = h264_args(enc, *kbps, fps);
            match protocol {
                ServerProtocol::Rtsp => a.extend(list(&["-f", "rtsp", "-rtsp_transport", "tcp"])),
                ServerProtocol::Rtmp => a.extend(list(&["-f", "flv"])),
                ServerProtocol::Srt => a.extend(list(&["-f", "mpegts"])),
            }
            a.push(url);
            Ok(a)
        }
        StreamKind::Ndi { .. } => Err("an NDI source is sent by NewQ itself, not FFmpeg".into()),
        StreamKind::Record {
            folder,
            codec,
            container,
            kbps,
            area,
            audio,
        } => {
            // `encoder` is the one for the codec (see `Sink::start`).
            let Some(enc) = h264 else {
                return Err(match codec {
                    crate::show::RecordCodec::H264 => NO_H264,
                    crate::show::RecordCodec::H265 => NO_HEVC,
                }
                .into());
            };
            // The file is made before FFmpeg starts (so two recordings never share a
            // name), so it is there, empty, to be written over. Timestamps are kept
            // constant-rate from the clock, so a frame the capture skipped does not
            // shorten the picture against the sound.
            let mut a = list(&["-y", "-fps_mode", "cfr"]);
            if let Some(area) = area {
                a.extend(list(&["-vf", &crop_filter(*area)]));
            }
            match codec {
                crate::show::RecordCodec::H264 => a.extend(record_h264_args(enc, *kbps, fps)),
                crate::show::RecordCodec::H265 => a.extend(hevc_args(enc, *kbps, fps)),
            }
            if audio.is_some() {
                a.extend(list(&[
                    "-map", "0:v", "-map", "1:a", "-c:a", "aac", "-b:a", "192k",
                ]));
            }
            let path = record_file
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| record_path(folder, *container, &jiff::Zoned::now()));
            a.push(path.to_string_lossy().into_owned());
            Ok(a)
        }
        StreamKind::DeckLink { device, size, ntsc } => {
            let device = device.trim();
            if device.is_empty() {
                return Err("no Blackmagic output chosen".into());
            }
            // The device wants UYVY, and a size and rate that are one of its
            // modes; a size is asked for here so FFmpeg scales to it.
            let mut a = list(&["-pix_fmt", "uyvy422"]);
            let size = size.trim();
            if !size.is_empty() {
                let ok = size.split_once('x').is_some_and(|(w, h)| {
                    w.parse::<u32>().is_ok_and(|w| w > 0) && h.parse::<u32>().is_ok_and(|h| h > 0)
                });
                if !ok {
                    return Err(format!("\"{size}\" isn't a size like 1920x1080"));
                }
                a.extend(list(&["-s", size]));
            }
            if *ntsc {
                a.extend(list(&["-r", &format!("{}/1001", fps.max(1) * 1000)]));
            }
            a.extend(list(&["-f", "decklink"]));
            a.push(device.to_string());
            Ok(a)
        }
        StreamKind::Custom { args } => {
            let a = split_args(args);
            if a.is_empty() {
                Err("no FFmpeg output given".into())
            } else {
                Ok(a)
            }
        }
    }
}

const NO_H264: &str = "This computer's FFmpeg has no H.264 encoder that works here (libx264, or \
    VideoToolbox, NVENC, Quick Sync or AMF with its hardware): use a Custom FFmpeg output or \
    install an FFmpeg with libx264 and point NEWQ_FFMPEG_DIR at it";

const NO_HEVC: &str = "This computer's FFmpeg has no H.265 encoder that works here (libx265, or \
    VideoToolbox, NVENC, Quick Sync or AMF with its hardware): record as H.264, or install an FFmpeg \
    with libx265 and point NEWQ_FFMPEG_DIR at it";

/// What this FFmpeg can do, asked once: its encoders, muxers and protocols.
struct Caps {
    encoders: String,
    muxers: String,
    protocols: String,
}

/// Whether a listing from `ffmpeg -encoders|-muxers|-protocols` has `name`
/// (the second word of a line, or a line of that word alone).
fn lists(listing: &str, name: &str) -> bool {
    listing.lines().any(|l| {
        let mut w = l.split_whitespace();
        match (w.next(), w.next()) {
            (Some(only), None) => only == name,
            (Some(_), Some(second)) => second == name,
            _ => false,
        }
    })
}

/// The first of `H264_ENCODERS` that `listed` and `works` both allow.
fn pick_encoder(
    listed: impl Fn(&str) -> bool,
    works: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    H264_ENCODERS.into_iter().find(|e| listed(e) && works(e))
}

fn ask(ff: &Ffmpeg, flag: &str) -> String {
    let mut cmd = ff.command();
    cmd.arg(flag);
    cmd.output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Whether `encoder` really encodes here: a hardware one is listed whether or
/// not the computer has the hardware, so try a few frames.
fn encoder_works(ff: &Ffmpeg, encoder: &str) -> bool {
    let mut cmd = ff.command();
    cmd.args(["-f", "lavfi", "-i", "color=c=black:s=640x360:r=30"])
        .args(["-frames:v", "8"])
        .args(if HEVC_ENCODERS.contains(&encoder) {
            hevc_args(encoder, 1000, 30)
        } else {
            h264_args(encoder, 1000, 30)
        })
        .args(["-f", "null", "-"]);
    // Nothing is read from it, and a driver that hangs must not hang a stream.
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn caps(ff: &Ffmpeg) -> &'static Caps {
    static CACHE: OnceLock<Caps> = OnceLock::new();
    CACHE.get_or_init(|| Caps {
        encoders: ask(ff, "-encoders"),
        muxers: ask(ff, "-muxers"),
        protocols: ask(ff, "-protocols"),
    })
}

/// The H.264 encoder to use with `ff`, if there is one that works. A working
/// one is remembered; no answer is tried again at the next start (a busy
/// graphics card or a driver resetting can pass later).
pub fn encoder(ff: &Ffmpeg) -> Option<&'static str> {
    static FOUND: OnceLock<&'static str> = OnceLock::new();
    if let Some(e) = FOUND.get() {
        return Some(e);
    }
    let c = caps(ff);
    let e = pick_encoder(|e| lists(&c.encoders, e), |e| encoder_works(ff, e))?;
    Some(FOUND.get_or_init(|| e))
}

/// The H.265 encoder to use with `ff`, if there is one that works (as `encoder`).
pub fn hevc_encoder(ff: &Ffmpeg) -> Option<&'static str> {
    static FOUND: OnceLock<&'static str> = OnceLock::new();
    if let Some(e) = FOUND.get() {
        return Some(e);
    }
    let c = caps(ff);
    let e = HEVC_ENCODERS
        .into_iter()
        .find(|e| lists(&c.encoders, e) && encoder_works(ff, e))?;
    Some(FOUND.get_or_init(|| e))
}

/// The H.264 encoder the user asked for by FFmpeg's name, if this FFmpeg has
/// it. It is not test-encoded: a hardware encoder that isn't there says so
/// itself when the stream starts.
pub fn encoder_named(ff: &Ffmpeg, wanted: &str) -> Result<&'static str, String> {
    let wanted = wanted.trim();
    let Some(known) = H264_ENCODERS.into_iter().find(|e| *e == wanted) else {
        return Err(format!("{wanted} is not an H.264 encoder NewQ knows"));
    };
    if lists(&caps(ff).encoders, known) {
        Ok(known)
    } else {
        Err(format!(
            "this FFmpeg has no {known} encoder: choose another"
        ))
    }
}

/// What each H.264 encoder is, as a person would say it.
pub fn encoder_label(name: &str) -> &'static str {
    match name {
        "libx264" => "x264 (software)",
        "h264_videotoolbox" => "Apple VideoToolbox (hardware)",
        "h264_nvenc" => "NVIDIA NVENC (hardware)",
        "h264_qsv" => "Intel Quick Sync (hardware)",
        "h264_amf" => "AMD AMF (hardware)",
        "libopenh264" => "OpenH264 (software)",
        _ => "",
    }
}

/// The names `encoder_named` accepts, best first.
pub fn encoder_names() -> [&'static str; 6] {
    H264_ENCODERS
}

/// Why `ff` can't publish `protocol`, if it can't.
fn missing_for(ff: &Ffmpeg, protocol: ServerProtocol) -> Option<String> {
    let c = caps(ff);
    let (muxer, protocol_name) = match protocol {
        ServerProtocol::Rtsp => ("rtsp", None),
        ServerProtocol::Rtmp => ("flv", Some("rtmp")),
        ServerProtocol::Srt => ("mpegts", Some("srt")),
    };
    if !lists(&c.muxers, muxer) {
        return Some(format!(
            "This FFmpeg can't write {}: use a build with the {muxer} muxer",
            protocol.label()
        ));
    }
    if let Some(p) = protocol_name
        && !lists(&c.protocols, p)
    {
        return Some(format!(
            "This FFmpeg has no {} support{}: send RTSP or RTMP instead",
            protocol.label(),
            if protocol == ServerProtocol::Srt {
                " (it needs libsrt)"
            } else {
                ""
            }
        ));
    }
    None
}

/// Why this FFmpeg can't record to `container`, if it can't (the Mac bundle
/// of 0.20.0 and before had no Matroska muxer).
fn missing_container(ff: &Ffmpeg, container: crate::show::RecordContainer) -> Option<String> {
    let muxer = container.muxer();
    let listed = &caps(ff).muxers;
    // An FFmpeg that didn't answer is left to say for itself what is wrong.
    (!listed.trim().is_empty() && !lists(listed, muxer)).then(|| {
        format!(
            "This FFmpeg can't write .{} files (it has no {muxer} muxer): choose another container",
            container.extension()
        )
    })
}

/// Keep only the end of what FFmpeg has said, once it has said a lot. Cut as
/// bytes, not as a string: FFmpeg's messages have characters of more than one
/// byte in them (quotes, file names), and a string cut inside one is a panic.
fn keep_tail(text: &mut Vec<u8>) {
    if text.len() > 4000 {
        text.drain(..text.len() - 2000);
    }
}

/// The line of what FFmpeg said that tells why it stopped: its last line is
/// often only the consequence ("Nothing was written") of an earlier one
/// ("Connection refused").
fn reason(said: &str) -> String {
    const CAUSES: [&str; 8] = [
        "refused",
        "timed out",
        "No route",
        "Could not",
        "Unable to",
        "unauthorized",
        "Unauthorized",
        "Connection",
    ];
    let lines: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).collect();
    lines
        .iter()
        .rev()
        .find(|l| CAUSES.iter().any(|c| l.contains(c)))
        .or(lines.last())
        .map_or_else(|| "FFmpeg stopped".to_string(), |l| l.trim().to_string())
}

/// A running FFmpeg taking frames on its standard input.
pub struct Sink {
    child: Child,
    stdin: Option<ChildStdin>,
    /// What FFmpeg has said on its error output.
    said: Arc<parking_lot::Mutex<String>>,
    /// Ends FFmpeg with NewQ, however NewQ ends (a crash left them running).
    #[cfg(windows)]
    _job: Option<crate::mediamtx::JobGuard>,
    /// How long `finish` waits before killing FFmpeg (`finish_patience`).
    patience: std::time::Duration,
    /// Feeds a recording's sound to FFmpeg.
    audio: Option<AudioPump>,
    /// The file a recording is written to; removed if nothing was written to it.
    record_file: Option<std::path::PathBuf>,
}

/// Remove `file` if it is empty: a recording that never began.
fn drop_empty(file: Option<&std::path::Path>) {
    if let Some(f) = file
        && std::fs::metadata(f).is_ok_and(|m| m.len() == 0)
    {
        let _ = std::fs::remove_file(f);
    }
}

/// The thread that hands a recording's sound to FFmpeg over a local socket.
struct AudioPump {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AudioPump {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl AudioPump {
    /// Listen on a free local port: FFmpeg connects to it as it opens its sound
    /// input. Returns the pump and the port.
    fn start(tap: crate::media::audio::AudioTap) -> Result<(AudioPump, u16), String> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("could not set up the sound for the recording: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name("record-audio".into())
            .spawn(move || pump_audio(listener, tap, flag))
            .map_err(|e| e.to_string())?;
        Ok((
            AudioPump {
                stop,
                handle: Some(handle),
            },
            port,
        ))
    }
}

/// Wait for FFmpeg to connect, then send it the sound as 32-bit float samples,
/// as it is played; silence while the device gives none, so FFmpeg is never
/// left waiting for sound that does not come.
fn pump_audio(
    listener: std::net::TcpListener,
    mut tap: crate::media::audio::AudioTap,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::io::Write;
    use std::sync::atomic::Ordering::Relaxed;
    let began = std::time::Instant::now();
    let mut stream = loop {
        if stop.load(Relaxed) || began.elapsed() > std::time::Duration::from_secs(20) {
            return;
        }
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    };
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    // FFmpeg that has stopped reading must not hold this thread, nor the
    // recording's end, for good.
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    // What was played before FFmpeg was ready is not wanted: it would put the
    // sound ahead of the picture.
    if let Ok(old) = tap.consumer.read_chunk(tap.consumer.slots()) {
        old.commit_all();
    }
    let rate = tap.sample_rate.max(8000) as usize;
    let mut last = std::time::Instant::now();
    let mut bytes: Vec<u8> = Vec::new();
    while !stop.load(Relaxed) {
        bytes.clear();
        let ready = tap.consumer.slots() / 2 * 2;
        if ready > 0 {
            if let Ok(chunk) = tap.consumer.read_chunk(ready) {
                let (a, b) = chunk.as_slices();
                for s in a.iter().chain(b) {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                chunk.commit_all();
            }
        } else if last.elapsed() > std::time::Duration::from_millis(300) {
            let frames = (last.elapsed().as_secs_f64() * rate as f64) as usize;
            bytes.resize(frames * 2 * 4, 0);
        }
        if bytes.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }
        if stream.write_all(&bytes).is_err() {
            return;
        }
        last = std::time::Instant::now();
    }
}

/// How long `Sink::finish` lets FFmpeg wind up (flush the encoder, say goodbye to
/// the server) before it is killed. A server that has stopped answering would
/// otherwise hold it, and the thread that waits for it, for good.
pub const FINISH_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// How long `kind`'s FFmpeg may take to finish. A Custom output may be writing a file
/// (closing an mp4 can take a while), so it is given much longer than a live stream.
pub fn finish_patience(kind: &StreamKind) -> std::time::Duration {
    match kind {
        StreamKind::Custom { .. } | StreamKind::Record { .. } => std::time::Duration::from_secs(60),
        _ => FINISH_PATIENCE,
    }
}

/// A way to end a `Sink`'s FFmpeg from another thread while the thread that owns the
/// sink is stuck in a write to it: the write then fails and the owner gives up.
#[derive(Clone, Debug)]
pub struct Killer {
    pid: u32,
    started: u64,
}

impl Killer {
    /// End the process, if it is still the one this was made for (a process
    /// number can be given to another program once the first has ended).
    pub fn kill(&self) -> bool {
        use sysinfo::{Pid, ProcessesToUpdate, System};
        let pid = Pid::from_u32(self.pid);
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        match sys.process(pid) {
            Some(p) if p.start_time() == self.started => p.kill(),
            _ => false,
        }
    }
}

impl Sink {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn start(
        ff: &Ffmpeg,
        size: (u32, u32),
        fps: u32,
        kind: &StreamKind,
    ) -> Result<Sink, String> {
        Self::start_with_audio(ff, size, fps, kind, None)
    }

    /// `start`, for a recording that has sound: `audio` is the copy of what is played.
    pub fn start_with_audio(
        ff: &Ffmpeg,
        size: (u32, u32),
        fps: u32,
        kind: &StreamKind,
        audio: Option<crate::media::audio::AudioTap>,
    ) -> Result<Sink, String> {
        let wants_audio = matches!(kind, StreamKind::Record { audio: Some(_), .. });
        if wants_audio != audio.is_some() {
            return Err(if wants_audio {
                "no sound to record".into()
            } else {
                "sound was given to a stream that doesn't record it".into()
            });
        }
        let mut pump = None;
        let mut record_file: Option<std::path::PathBuf> = None;
        let mut cmd = if let StreamKind::Ndi { name } = kind {
            let name = name.trim();
            if name.is_empty() {
                return Err("no name for the NDI source".into());
            }
            if !crate::ndi::runtime_installed() {
                return Err(crate::ndi::NOT_INSTALLED.into());
            }
            crate::ndi::helper(&[
                "--ndi-send".into(),
                name.into(),
                size.0.to_string(),
                size.1.to_string(),
                fps.to_string(),
            ])?
        } else {
            if let StreamKind::Server { protocol, .. } = kind
                && let Some(why) = missing_for(ff, *protocol)
            {
                return Err(why);
            }
            if let StreamKind::Record { container, .. } = kind
                && let Some(why) = missing_container(ff, *container)
            {
                return Err(why);
            }
            if matches!(kind, StreamKind::DeckLink { .. })
                && !crate::media::camera::ffmpeg_has_decklink_output(ff)
            {
                return Err(crate::media::camera::DECKLINK_OUTPUT_UNSUPPORTED.into());
            }
            // An encoder the user chose is used as it is; otherwise the first
            // that works.
            let chosen = match kind {
                StreamKind::Server {
                    encoder: wanted, ..
                } if !wanted.trim().is_empty() => Some(encoder_named(ff, wanted)?),
                StreamKind::Record {
                    codec: crate::show::RecordCodec::H265,
                    ..
                } => hevc_encoder(ff),
                _ => encoder(ff),
            };
            if let Some(name) = chosen {
                log::info!("stream encoder: {name}");
            }
            if let StreamKind::Record {
                folder, container, ..
            } = kind
            {
                record_file = Some(reserve_record_file(folder, *container)?);
            }
            let output = output_args_to(kind, chosen, fps, record_file.as_deref());
            let output = match output {
                Ok(o) => o,
                Err(e) => {
                    drop_empty(record_file.as_deref());
                    return Err(e);
                }
            };
            let mut cmd = ff.command();
            let mut video = input_args(size, fps);
            if record_file.is_some()
                && let Some(at) = video.iter().position(|a| a == "-i")
            {
                // Frames are stamped by the clock as they arrive.
                video.splice(at..at, ["-use_wallclock_as_timestamps".into(), "1".into()]);
            }
            cmd.args(video);
            if let Some(tap) = audio {
                let rate = tap.sample_rate;
                let (p, port) = AudioPump::start(tap)?;
                pump = Some(p);
                cmd.args(["-f", "f32le", "-ar", &rate.to_string(), "-ac", "2"])
                    .args(["-i", &format!("tcp://127.0.0.1:{port}")]);
            }
            cmd.args(output);
            cmd
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::null());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                drop_empty(record_file.as_deref());
                return Err(format!("could not start FFmpeg: {e}"));
            }
        };
        if log::log_enabled!(log::Level::Debug) {
            let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
            log::debug!("stream ffmpeg {} started: {}", child.id(), args.join(" "));
        }
        let stdin = child.stdin.take();
        let said = Arc::new(parking_lot::Mutex::new(String::new()));
        if let Some(mut err) = child.stderr.take() {
            let said = said.clone();
            let _ = std::thread::Builder::new()
                .name("stream-ffmpeg-log".into())
                .spawn(move || {
                    let mut text: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = std::io::Read::read(&mut err, &mut buf) {
                        if n == 0 {
                            break;
                        }
                        text.extend_from_slice(&buf[..n]);
                        keep_tail(&mut text);
                        *said.lock() = String::from_utf8_lossy(&text).into_owned();
                    }
                });
        }
        #[cfg(windows)]
        let _job = crate::mediamtx::tie_to_us(&child);
        Ok(Sink {
            child,
            stdin,
            said,
            #[cfg(windows)]
            _job,
            patience: finish_patience(kind),
            audio: pump,
            record_file,
        })
    }

    /// A handle that can end this FFmpeg from another thread.
    pub fn killer(&self) -> Killer {
        use sysinfo::{Pid, ProcessesToUpdate, System};
        let pid = Pid::from_u32(self.child.id());
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        Killer {
            pid: self.child.id(),
            started: sys.process(pid).map_or(0, |p| p.start_time()),
        }
    }

    /// Send one frame (`size.0 * size.1 * 4` bytes of RGBA). An error says why
    /// FFmpeg stopped, as far as it said.
    pub fn write(&mut self, frame: &[u8]) -> Result<(), String> {
        let Some(stdin) = &mut self.stdin else {
            return Err("FFmpeg is not running".into());
        };
        match stdin.write_all(frame) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Give the process a moment to say why it stopped.
                std::thread::sleep(std::time::Duration::from_millis(200));
                let said = self.said.lock().trim().to_string();
                Err(if said.is_empty() {
                    format!("FFmpeg stopped: {e}")
                } else {
                    format!("FFmpeg stopped: {}", reason(&said))
                })
            }
        }
    }

    /// Whether the process is still going, and if not what it said.
    pub fn failed(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                let said = self.said.lock().trim().to_string();
                Some(reason(&said))
            }
            _ => None,
        }
    }

    /// Close the input so FFmpeg finishes what it has, and wait for it, but only
    /// for `finish_patience`: then it is killed.
    pub fn finish(mut self) {
        self.stdin = None;
        if let Some(mut p) = self.audio.take() {
            p.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = p.handle.take() {
                let _ = h.join();
            }
        }
        let patience = self.patience;
        let deadline = std::time::Instant::now() + patience;
        while matches!(self.child.try_wait(), Ok(None)) {
            if std::time::Instant::now() >= deadline {
                log::warn!("FFmpeg did not finish in {patience:?}: ending it");
                let _ = self.child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        match self.child.wait() {
            Ok(status) => log::debug!("stream ffmpeg {} finished ({status})", self.child.id()),
            Err(e) => log::debug!("stream ffmpeg could not be waited for: {e}"),
        }
        drop_empty(self.record_file.as_deref());
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.stdin = None;
        if let Some(p) = &self.audio {
            p.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop_empty(self.record_file.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_keep_the_shape_with_even_sides() {
        assert_eq!(size_for((1920, 1080), 720), (1280, 720));
        assert_eq!(size_for((3552, 1080), 720), (2368, 720));
        assert_eq!(size_for((1000, 777), 333), (426, 332));
        // Hardware encoders stop at 4096 wide, so a very wide canvas is sent smaller.
        let (w, h) = size_for((5760, 1080), 1080);
        assert!(w == 4096 && h == 768 && h % 2 == 0, "{w}x{h}");
        // Nonsense is kept in range.
        let (w, h) = size_for((0, 0), 0);
        assert!(w >= 16 && h >= 16 && w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn what_ffmpeg_said_is_cut_to_its_end_between_characters() {
        // Quotes of three bytes each: a cut by string length would land
        // inside one and panic the thread that listens to FFmpeg.
        let mut text: Vec<u8> = "“x”".repeat(1000).into_bytes();
        keep_tail(&mut text);
        assert_eq!(text.len(), 2000);
        let tail = String::from_utf8_lossy(&text);
        assert!(tail.ends_with("x”"));
        let mut short: Vec<u8> = b"fine".to_vec();
        keep_tail(&mut short);
        assert_eq!(short, b"fine");
    }

    #[test]
    fn arguments_split_like_a_shell_would_for_quotes() {
        assert_eq!(split_args("-f flv  out.flv"), ["-f", "flv", "out.flv"]);
        assert_eq!(
            split_args(r#"-metadata title="two words" 'a b' c"#),
            ["-metadata", "title=two words", "a b", "c"]
        );
        assert_eq!(split_args(r#"-x "" y"#), ["-x", "", "y"]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn a_container_is_recorded_only_with_its_muxer() {
        use crate::show::RecordContainer;
        // `ffmpeg -muxers` lines, as FFmpeg 7 to 9 print them.
        let mac_0_20 =
            " E mov             QuickTime / MOV\n E mp4             MP4 (MPEG-4 Part 14)\n";
        assert!(!lists(mac_0_20, RecordContainer::Mkv.muxer()));
        assert!(lists(mac_0_20, RecordContainer::Mov.muxer()));
        assert!(lists(mac_0_20, RecordContainer::Mp4.muxer()));
        assert!(lists(
            " E matroska        Matroska\n",
            RecordContainer::Mkv.muxer()
        ));
        // The FFmpeg the tests use writes all three.
        let Some(ff) = crate::media::ffmpeg::Ffmpeg::locate() else {
            return;
        };
        for c in [
            RecordContainer::Mkv,
            RecordContainer::Mov,
            RecordContainer::Mp4,
        ] {
            assert_eq!(missing_container(&ff, c), None, "{c:?}");
        }
    }

    #[test]
    fn frames_go_in_as_raw_rgba_of_the_size_given() {
        let a = input_args((1280, 720), 30);
        let line = a.join(" ");
        assert_eq!(
            line,
            "-f rawvideo -pix_fmt rgba -s 1280x720 -framerate 30 -i pipe:0"
        );
    }

    #[test]
    fn a_blackmagic_output_is_sent_as_uyvy_to_the_device() {
        let k = |device: &str, size: &str| StreamKind::DeckLink {
            device: device.into(),
            size: size.into(),
            ntsc: false,
        };
        let a = output_args(&k(" UltraStudio Express Monitor ", ""), None, 30).unwrap();
        assert_eq!(
            a,
            [
                "-pix_fmt",
                "uyvy422",
                "-f",
                "decklink",
                "UltraStudio Express Monitor"
            ]
        );
        let b = output_args(&k("Mini Monitor", "1920x1080"), None, 60).unwrap();
        assert_eq!(
            b,
            [
                "-pix_fmt",
                "uyvy422",
                "-s",
                "1920x1080",
                "-f",
                "decklink",
                "Mini Monitor"
            ]
        );
        assert!(output_args(&k("", ""), None, 30).is_err());
        assert!(output_args(&k("Mini Monitor", "big"), None, 30).is_err());
        assert!(output_args(&k("Mini Monitor", "0x1080"), None, 30).is_err());
    }

    #[test]
    fn a_recording_has_its_codec_area_and_a_dated_file() {
        use crate::show::{RecordArea, RecordCodec, RecordContainer};
        let kind = |codec, container, area| StreamKind::Record {
            folder: "C:/Rec".into(),
            codec,
            container,
            kbps: 8000,
            area,
            audio: None,
        };
        let a = output_args(
            &kind(RecordCodec::H264, RecordContainer::Mkv, None),
            Some("libx264"),
            30,
        )
        .unwrap();
        assert!(a.contains(&"-y".to_string()) && a.join(" ").contains("-fps_mode cfr"));
        assert!(!a.contains(&"-vf".to_string()));
        assert!(a.join(" ").contains("-c:v libx264"));
        let file = a.last().unwrap();
        assert!(
            file.replace('\\', "/").starts_with("C:/Rec/NewQ 20") && file.ends_with(".mkv"),
            "{file}"
        );
        let b = output_args(
            &kind(
                RecordCodec::H265,
                RecordContainer::Mov,
                Some(RecordArea {
                    x: 0.25,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                }),
            ),
            Some("libx265"),
            30,
        )
        .unwrap();
        let line = b.join(" ");
        assert!(
            line.contains("-c:v libx265") && line.contains("-tag:v hvc1"),
            "{line}"
        );
        assert!(
            line.contains("-vf crop=trunc(iw*0.5000/2)*2:trunc(ih*1.0000/2)*2:trunc(iw*0.2500/2)*2:trunc(ih*0.0000/2)*2"),
            "{line}"
        );
        assert!(b.last().unwrap().ends_with(".mov"));
        // No encoder of the codec: it says which.
        let none = output_args(
            &kind(RecordCodec::H265, RecordContainer::Mov, None),
            None,
            30,
        );
        assert!(none.unwrap_err().contains("H.265"));
        // An area is kept inside the picture.
        let wild = RecordArea {
            x: 0.9,
            y: -1.0,
            w: 5.0,
            h: 0.0,
        }
        .clamped();
        assert!(wild.x + wild.w <= 1.0 && wild.y == 0.0 && wild.h >= 0.01);
        // Closing a file takes time, so a recording is given as long as a Custom output.
        assert_eq!(
            finish_patience(&kind(RecordCodec::H264, RecordContainer::Mov, None)),
            finish_patience(&StreamKind::Custom { args: "x".into() })
        );
    }

    #[test]
    fn a_recording_with_no_folder_goes_to_downloads() {
        let at: jiff::Zoned = "2026-11-09T14:05:09-05:00[America/New_York]"
            .parse()
            .unwrap();
        let p = record_path("", crate::show::RecordContainer::Mp4, &at);
        assert_eq!(p.file_name().unwrap(), "NewQ 2026-11-09 14.05.09.mp4");
        assert!(
            p.parent().unwrap().ends_with("Downloads")
                || p.parent().unwrap() == std::path::Path::new(".")
        );
    }

    /// Record a second of frames for real, in each codec this computer can
    /// encode, and ask ffprobe what came out.
    #[test]
    fn a_recording_is_a_playable_file_of_the_size_asked() {
        use crate::show::{RecordArea, RecordCodec, RecordContainer};
        let Some(ff) = Ffmpeg::locate() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_test_record_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (codec, container, area) in [
            (RecordCodec::H264, RecordContainer::Mkv, None),
            (
                RecordCodec::H264,
                RecordContainer::Mov,
                Some(RecordArea {
                    x: 0.25,
                    y: 0.25,
                    w: 0.5,
                    h: 0.5,
                }),
            ),
            (RecordCodec::H265, RecordContainer::Mp4, None),
        ] {
            let have = match codec {
                RecordCodec::H264 => encoder(&ff),
                RecordCodec::H265 => hevc_encoder(&ff),
            };
            if have.is_none() {
                eprintln!("no {} encoder here; skipping", codec.label());
                continue;
            }
            let sub = dir.join(format!("{codec:?}{container:?}"));
            let kind = StreamKind::Record {
                folder: sub.to_string_lossy().into_owned(),
                codec,
                container,
                kbps: 4000,
                area,
                audio: None,
            };
            let mut sink = Sink::start(&ff, (640, 360), 30, &kind).expect("start");
            let frame = vec![128u8; 640 * 360 * 4];
            for _ in 0..45 {
                sink.write(&frame).expect("write");
            }
            sink.finish();
            let file = std::fs::read_dir(&sub)
                .unwrap()
                .next()
                .expect("a file")
                .unwrap()
                .path();
            let mut probe = ff.command();
            probe
                .args(["-loglevel", "info", "-i"])
                .arg(&file)
                .args(["-f", "null", "-"]);
            let out = probe.output().unwrap();
            assert!(
                out.status.success(),
                "{file:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8_lossy(&out.stderr);
            let want = if area.is_some() { "320x180" } else { "640x360" };
            assert!(
                text.contains(want) && !text.contains("rror"),
                "{file:?} should be {want}: {text}"
            );
            let size = std::fs::metadata(&file).unwrap().len();
            assert!(size > 1000, "{file:?} is {size} bytes");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// Record a second of picture with a tone as its sound, and read back that
    /// the file has both, the sound about as long as the picture.
    #[test]
    fn a_recording_can_carry_sound() {
        use crate::show::{RecordAudio, RecordCodec, RecordContainer};
        let Some(ff) = Ffmpeg::locate() else {
            return;
        };
        if encoder(&ff).is_none() {
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("newq_test_record_audio_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for container in [RecordContainer::Mkv, RecordContainer::Mov] {
            let sub = dir.join(format!("{container:?}"));
            let kind = StreamKind::Record {
                folder: sub.to_string_lossy().into_owned(),
                codec: RecordCodec::H264,
                container,
                kbps: 4000,
                area: None,
                audio: Some(RecordAudio::default()),
            };
            // Without sound to give it, it says so.
            assert!(Sink::start(&ff, (320, 180), 30, &kind).is_err());
            let (mut tone, tap) = crate::media::audio::AudioTap::for_test(48_000);
            let feeder = std::thread::spawn(move || {
                // A 440 Hz tone, in real time, for a bit more than the picture.
                let began = std::time::Instant::now();
                let mut n = 0u64;
                while began.elapsed() < std::time::Duration::from_millis(2200) {
                    let due = (began.elapsed().as_secs_f64() * 48_000.0) as u64;
                    while n < due && tone.slots() >= 2 {
                        let s = (n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
                        let _ = tone.push(s);
                        let _ = tone.push(s);
                        n += 1;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            });
            let mut sink =
                Sink::start_with_audio(&ff, (320, 180), 30, &kind, Some(tap)).expect("start");
            let frame = vec![90u8; 320 * 180 * 4];
            let began = std::time::Instant::now();
            for i in 0..60u32 {
                sink.write(&frame).expect("write");
                // In real time, as the capture thread is.
                let due = std::time::Duration::from_millis(u64::from(i + 1) * 33);
                if let Some(wait) = due.checked_sub(began.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            sink.finish();
            feeder.join().unwrap();
            let file = std::fs::read_dir(&sub)
                .unwrap()
                .next()
                .expect("a file")
                .unwrap()
                .path();
            let mut probe = ff.command();
            probe
                .args(["-loglevel", "info", "-i"])
                .arg(&file)
                .args(["-f", "null", "-"]);
            let out = probe.output().unwrap();
            let text = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(
                out.status.success() && !text.contains("rror"),
                "{file:?}: {text}"
            );
            assert!(
                text.contains("Video: h264") && text.contains("Audio: aac"),
                "{file:?}: {text}"
            );
            // About two seconds long (60 frames at 30 a second).
            let secs = text
                .split("Duration: 00:00:")
                .nth(1)
                .and_then(|r| r.get(..5))
                .and_then(|r| r.parse::<f64>().ok())
                .unwrap_or(0.0);
            assert!((1.5..3.2).contains(&secs), "{file:?} is {secs} s: {text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_recordings_that_start_together_get_their_own_files() {
        use crate::show::RecordContainer;
        let dir = std::env::temp_dir().join(format!("newq_test_names_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("not made yet").to_string_lossy().into_owned();
        let a = reserve_record_file(&folder, RecordContainer::Mkv).unwrap();
        let b = reserve_record_file(&folder, RecordContainer::Mkv).unwrap();
        let c = reserve_record_file(&folder, RecordContainer::Mkv).unwrap();
        assert!(a != b && b != c && a != c, "{a:?} {b:?} {c:?}");
        for p in [&a, &b, &c] {
            assert!(p.exists() && p.extension().unwrap() == "mkv", "{p:?}");
        }
        // One that never began leaves no empty file behind.
        drop_empty(Some(&a));
        assert!(!a.exists());
        std::fs::write(&b, b"data").unwrap();
        drop_empty(Some(&b));
        assert!(b.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_is_not_tuned_for_a_live_viewer() {
        let live = h264_args("libx264", 8000, 30).join(" ");
        let file = record_h264_args("libx264", 8000, 30).join(" ");
        assert!(live.contains("-tune zerolatency") && live.contains("-bf 0"));
        assert!(
            !file.contains("zerolatency") && !file.contains("-bf"),
            "{file}"
        );
        assert!(
            file.contains("-c:v libx264") && file.contains("-b:v 8000k"),
            "{file}"
        );
        let vt = record_h264_args("h264_videotoolbox", 8000, 30).join(" ");
        assert!(!vt.contains("-realtime"), "{vt}");
        let nv = record_h264_args("h264_nvenc", 8000, 30).join(" ");
        assert!(!nv.contains("-tune"), "{nv}");
    }

    #[test]
    fn a_blackmagic_ntsc_rate_is_the_rate_over_1_001() {
        let k = StreamKind::DeckLink {
            device: "Mini Monitor".into(),
            size: String::new(),
            ntsc: true,
        };
        let a = output_args(&k, None, 60).unwrap().join(" ");
        assert!(a.contains("-r 60000/1001 -f decklink"), "{a}");
        let b = output_args(&k, None, 30).unwrap().join(" ");
        assert!(b.contains("-r 30000/1001"), "{b}");
    }

    /// A stall in the picture (the capture skips frames it can't keep up with)
    /// must not make the file shorter than the time it took.
    #[test]
    fn a_stall_in_the_picture_does_not_shorten_the_recording() {
        use crate::show::{RecordCodec, RecordContainer};
        let Some(ff) = Ffmpeg::locate() else {
            return;
        };
        if encoder(&ff).is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("newq_test_stall_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let kind = StreamKind::Record {
            folder: dir.to_string_lossy().into_owned(),
            codec: RecordCodec::H264,
            container: RecordContainer::Mkv,
            kbps: 2000,
            area: None,
            audio: None,
        };
        let mut sink = Sink::start(&ff, (320, 180), 30, &kind).expect("start");
        let frame = vec![60u8; 320 * 180 * 4];
        let began = std::time::Instant::now();
        for i in 0..15u64 {
            sink.write(&frame).unwrap();
            std::thread::sleep(
                std::time::Duration::from_millis((i + 1) * 33).saturating_sub(began.elapsed()),
            );
        }
        // Nothing for a second, then the picture goes on.
        std::thread::sleep(std::time::Duration::from_millis(1000));
        for _ in 0..15 {
            sink.write(&frame).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
        // The picture is stamped by the clock, so the file lasts as long as the
        // frames took to send: about two seconds, longer on a slow machine (a
        // macOS runner oversleeps and waits on its encoder).
        let sent = began.elapsed().as_secs_f64();
        sink.finish();
        let file = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .expect("a file")
            .unwrap()
            .path();
        let mut probe = ff.command();
        probe
            .args(["-loglevel", "info", "-i"])
            .arg(&file)
            .args(["-f", "null", "-"]);
        let text = String::from_utf8_lossy(&probe.output().unwrap().stderr).into_owned();
        let secs = text
            .split("Duration: 00:00:")
            .nth(1)
            .and_then(|r| r.get(..5))
            .and_then(|r| r.parse::<f64>().ok())
            .unwrap_or(0.0);
        // Without the fix the file is one second (its 30 frames), whatever the time.
        assert!(secs >= 1.7, "{secs} s for {sent:.2} s sent: {text}");
        assert!(
            (secs - sent).abs() < 0.7,
            "{secs} s for {sent:.2} s sent: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_destination_has_the_arguments_it_needs() {
        let udp = StreamKind::Udp {
            host: " 127.0.0.1 ".into(),
            port: 5000,
        };
        let a = output_args(&udp, Some("libx264"), 30).unwrap().join(" ");
        assert!(
            a.contains("libx264") && a.ends_with("udp://127.0.0.1:5000?pkt_size=1316"),
            "{a}"
        );
        let b = output_args(&udp, None, 30).unwrap().join(" ");
        assert!(b.contains("mpeg4") && !b.contains("libx264"), "{b}");
        assert!(
            output_args(
                &StreamKind::Udp {
                    host: " ".into(),
                    port: 1
                },
                Some("libx264"),
                30
            )
            .is_err()
        );

        let custom = StreamKind::Custom {
            args: "-f flv rtmp://x/y".into(),
        };
        assert_eq!(
            output_args(&custom, None, 30).unwrap(),
            ["-f", "flv", "rtmp://x/y"]
        );
        assert!(output_args(&StreamKind::Custom { args: "".into() }, None, 30).is_err());

        let cam = StreamKind::VirtualCamera {
            device: "/dev/video10".into(),
        };
        if cfg!(target_os = "linux") {
            assert_eq!(
                output_args(&cam, None, 30).unwrap().join(" "),
                "-pix_fmt yuyv422 -f v4l2 /dev/video10"
            );
            assert!(
                output_args(&StreamKind::VirtualCamera { device: "".into() }, None, 30).is_err()
            );
        } else {
            assert!(output_args(&cam, None, 30).unwrap_err().contains("OBS"));
        }
    }

    #[test]
    fn a_stopped_server_stream_is_retried_at_growing_then_steady_intervals() {
        let secs: Vec<u64> = (1..8).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 15, 30, 30, 30]);
        assert_eq!(retry_delay(0).as_secs(), 2);
        assert!(SETTLE < STEADY);
    }

    #[test]
    fn a_change_waits_until_it_has_stopped_changing() {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut seen = HashMap::new();
        let mut running: HashMap<u8, String> = HashMap::new();
        let ask = |v: &str| HashMap::from([(1u8, v.to_string())]);

        // A new stream waits for the settings to settle, then starts.
        let mut w = ask("a");
        settle(&mut seen, &mut w, &running, t0);
        assert!(w.is_empty());
        let mut w = ask("a");
        settle(
            &mut seen,
            &mut w,
            &running,
            t0 + SETTLE + Duration::from_millis(1),
        );
        assert_eq!(w, ask("a"));
        running.insert(1, "a".into());

        // Typing: each letter restarts the wait, and the running stream stays as it was.
        let mut at = t0 + SETTLE * 2;
        for text in ["ab", "abc", "abcd"] {
            let mut w = ask(text);
            settle(&mut seen, &mut w, &running, at);
            assert_eq!(w, ask("a"), "{text}");
            at += SETTLE / 2;
        }
        // Left alone, the last text is taken.
        let mut w = ask("abcd");
        settle(&mut seen, &mut w, &running, at + SETTLE);
        assert_eq!(w, ask("abcd"));
        // Turned off: forgotten at once, so turning it on waits again.
        let mut w = HashMap::new();
        settle(&mut seen, &mut w, &running, at + SETTLE);
        assert!(seen.is_empty());
    }

    #[test]
    fn the_reason_a_stream_stopped_is_the_cause_not_the_last_line() {
        let said = "[tcp @ 0x1] Connection to tcp://127.0.0.1:8554 failed: Connection refused\n\
                    [out#0/rtsp @ 0x2] Nothing was written into output file\n";
        assert!(
            reason(said).contains("Connection refused"),
            "{}",
            reason(said)
        );
        assert_eq!(reason("one\ntwo"), "two");
        assert_eq!(reason(""), "FFmpeg stopped");
    }

    #[test]
    fn listings_are_read_by_name() {
        let encoders = " V....D libx264              libx264 H.264\n V....D h264_nvenc   NVIDIA\n";
        assert!(lists(encoders, "libx264") && lists(encoders, "h264_nvenc"));
        assert!(!lists(encoders, "h264_qsv") && !lists(encoders, "H.264"));
        let muxers = "  E flv             FLV\n DE rtsp   RTSP output\n";
        assert!(lists(muxers, "flv") && lists(muxers, "rtsp") && !lists(muxers, "mpegts"));
        let protocols = "Input:\n  rtmp\n  srt\nOutput:\n  tcp\n";
        assert!(lists(protocols, "srt") && lists(protocols, "tcp") && !lists(protocols, "rtp"));
    }

    #[test]
    fn the_best_encoder_that_is_there_and_works_is_picked() {
        let has = |names: &'static [&'static str]| move |e: &str| names.contains(&e);
        assert_eq!(
            pick_encoder(has(&["libx264", "h264_nvenc"]), |_| true),
            Some("libx264")
        );
        // A Mac's bundle has VideoToolbox only.
        assert_eq!(
            pick_encoder(has(&["h264_videotoolbox"]), |_| true),
            Some("h264_videotoolbox")
        );
        // Listed but no hardware for it: the next one.
        assert_eq!(
            pick_encoder(has(&["h264_nvenc", "h264_qsv"]), |e| e == "h264_qsv"),
            Some("h264_qsv")
        );
        assert_eq!(pick_encoder(has(&["mpeg4"]), |_| true), None);
        assert_eq!(pick_encoder(has(&["h264_nvenc"]), |_| false), None);
    }

    #[test]
    fn each_encoder_gets_its_own_arguments_and_a_two_second_keyframe_interval() {
        for enc in H264_ENCODERS {
            let a = h264_args(enc, 6000, 25).join(" ");
            assert!(a.starts_with(&format!("-c:v {enc} ")), "{a}");
            assert!(
                a.contains("-b:v 6000k -maxrate 6000k -bufsize 12000k -g 50"),
                "{a}"
            );
        }
        assert!(
            h264_args("libx264", 6000, 30)
                .join(" ")
                .contains("-tune zerolatency")
        );
        assert!(
            h264_args("h264_videotoolbox", 6000, 30)
                .join(" ")
                .contains("-realtime 1")
        );
        assert!(
            h264_args("h264_qsv", 6000, 30)
                .join(" ")
                .contains("-pix_fmt nv12")
        );
        // A file being closed is given time; a live stream's FFmpeg is not waited on for long.
        assert!(
            finish_patience(&StreamKind::Custom {
                args: "out.mp4".into()
            }) > finish_patience(&StreamKind::server(ServerProtocol::Rtsp))
        );
        // AMF's low-latency usages send one keyframe and never another.
        let amf = h264_args("h264_amf", 6000, 30).join(" ");
        assert!(amf.contains("-usage webcam"), "{amf}");
        assert!(!amf.contains("latency"), "{amf}");
        // A silly bitrate is held to a range.
        assert!(h264_args("libx264", 1, 30).join(" ").contains("-b:v 200k"));
        assert!(
            h264_args("libx264", 9_999_999, 30)
                .join(" ")
                .contains("-b:v 100000k")
        );
    }

    #[test]
    fn server_addresses_are_built_for_each_protocol() {
        use ServerProtocol::*;
        assert_eq!(
            server_url(Rtsp, " 10.0.0.5 ", 8554, "/newq/").unwrap(),
            "rtsp://10.0.0.5:8554/newq"
        );
        assert_eq!(
            server_url(Rtmp, "media.local", 1935, "live/show").unwrap(),
            "rtmp://media.local:1935/live/show"
        );
        assert_eq!(
            server_url(Srt, "media.local", 8890, "newq").unwrap(),
            "srt://media.local:8890?streamid=publish:newq&pkt_size=1316"
        );
        // Port 0 is the protocol's own; an IPv6 address is bracketed.
        assert_eq!(
            server_url(Rtsp, "::1", 0, "a").unwrap(),
            "rtsp://[::1]:8554/a"
        );
        assert!(server_url(Rtsp, "", 1, "a").is_err());
        assert!(server_url(Rtsp, "h", 1, " / ").is_err());
        assert!(server_url(Rtsp, "h", 1, "a b").is_err());
        assert!(server_url(Srt, "h", 1, "a&streamid=x").is_err());
        // Credentials and ports typed into the wrong box are refused, not mangled.
        assert!(server_url(Rtsp, "host:8554", 1, "a").is_err());
        assert!(server_url(Rtsp, "user:pw@host", 1, "a").is_err());
        assert!(server_url(Srt, "h", 1, "a:user:pw").is_err());
    }

    #[test]
    fn a_server_stream_needs_an_h264_encoder_and_ends_with_its_address() {
        let kind = StreamKind::server(ServerProtocol::Rtsp);
        let a = output_args(&kind, Some("h264_videotoolbox"), 30).unwrap();
        let line = a.join(" ");
        assert!(
            line.starts_with("-c:v h264_videotoolbox ")
                && line.ends_with("-f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/newq"),
            "{line}"
        );
        let rtmp = output_args(
            &StreamKind::server(ServerProtocol::Rtmp),
            Some("libx264"),
            30,
        )
        .unwrap()
        .join(" ");
        assert!(
            rtmp.ends_with("-f flv rtmp://127.0.0.1:1935/newq"),
            "{rtmp}"
        );
        let srt = output_args(
            &StreamKind::server(ServerProtocol::Srt),
            Some("libx264"),
            30,
        )
        .unwrap()
        .join(" ");
        assert!(
            srt.contains("-f mpegts srt://127.0.0.1:8890?streamid=publish:newq"),
            "{srt}"
        );
        assert!(output_args(&kind, None, 30).unwrap_err().contains("H.264"));
    }

    /// Publish a few frames to a receiver that FFmpeg itself runs, the way
    /// MediaMTX would take them, and count what arrives. (Not MediaMTX: that
    /// isn't here; this checks the framing, encoder and arguments.)
    fn publish_to_a_listener(protocol: ServerProtocol, receiver_args: &[&str], port: u16) {
        let Some(ff) = Ffmpeg::locate() else {
            if std::env::var_os("NEWQ_REQUIRE_MEDIA").is_some() {
                panic!("NEWQ_REQUIRE_MEDIA is set but ffmpeg was not found");
            }
            return;
        };
        if encoder(&ff).is_none() || missing_for(&ff, protocol).is_some() {
            eprintln!("this ffmpeg can't publish {}; skipping", protocol.label());
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "newq_test_server_{}_{}",
            protocol.label(),
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // MPEG-TS: every FFmpeg build has that muxer, the Mac bundle's included.
        let out = dir.join("received.ts");
        let _ = std::fs::remove_file(&out);
        /// Kills the receiver however the test ends, so a failure leaves no
        /// listener holding the port.
        struct Receiver(std::process::Child);
        impl Drop for Receiver {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut rx = ff.command();
        rx.args(receiver_args)
            .args(["-t", "20", "-c", "copy", "-y"])
            .arg(&out)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.join("receiver.log")).unwrap());
        let mut rx = Receiver(rx.spawn().expect("start the receiver"));
        let kind = StreamKind::Server {
            protocol,
            host: "127.0.0.1".into(),
            port,
            path: "newq".into(),
            kbps: 1000,
            adapter: String::new(),
            player_port: crate::show::default_player_port(),
            host_here: false,
            encoder: String::new(),
        };
        let size = (320u32, 180u32);
        let frame = [40u8, 120, 200, 255].repeat((size.0 * size.1) as usize);
        // The receiver takes a while to listen on a cold machine, and a write into
        // FFmpeg's pipe succeeds before it has tried to connect: so the whole
        // publish is tried again until every frame of it has gone out.
        let mut published = false;
        let mut why = String::new();
        for _ in 0..12 {
            let mut sink = Sink::start(&ff, size, 25, &kind).expect("start the sink");
            let mut sent = true;
            for _ in 0..50 {
                if let Err(e) = sink.write(&frame) {
                    why = e;
                    sent = false;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            if sent && let Some(e) = sink.failed() {
                why = e;
                sent = false;
            }
            if sent {
                sink.finish();
                published = true;
                break;
            }
            drop(sink);
            std::thread::sleep(std::time::Duration::from_millis(700));
        }
        assert!(
            published,
            "the receiver never took the stream: {why}\nreceiver said: {}",
            std::fs::read_to_string(dir.join("receiver.log")).unwrap_or_default()
        );
        // Let it finish what it has, but not for ever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while rx.0.try_wait().ok().flatten().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let probe = ff
            .command()
            .args(["-i"])
            .arg(&out)
            .args(["-map", "0:v:0", "-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            probe.status.success() && out.metadata().map(|m| m.len() > 1000).unwrap_or(false),
            "{}: {}",
            protocol.label(),
            String::from_utf8_lossy(&probe.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chosen_encoder_is_used_as_named_and_a_wrong_name_is_an_error() {
        for name in encoder_names() {
            assert!(!encoder_label(name).is_empty(), "{name} has no label");
        }
        let Some(ff) = Ffmpeg::locate() else {
            if std::env::var_os("NEWQ_REQUIRE_MEDIA").is_some() {
                panic!("NEWQ_REQUIRE_MEDIA is set but ffmpeg was not found");
            }
            return;
        };
        let why = encoder_named(&ff, "not_an_encoder").unwrap_err();
        assert!(why.contains("not an H.264 encoder NewQ knows"), "{why}");
        // One this FFmpeg lists is returned as named, with no test encode.
        if let Some(listed) = encoder_names()
            .into_iter()
            .find(|n| lists(&caps(&ff).encoders, n))
        {
            assert_eq!(encoder_named(&ff, &format!(" {listed} ")), Ok(listed));
        }
        // One it doesn't list says so, naming it.
        if let Some(missing) = encoder_names()
            .into_iter()
            .find(|n| !lists(&caps(&ff).encoders, n))
        {
            let why = encoder_named(&ff, missing).unwrap_err();
            assert!(
                why.contains(missing) && why.contains("choose another"),
                "{why}"
            );
        }
        // The choice reaches the arguments FFmpeg is given.
        let mut kind = StreamKind::server(ServerProtocol::Rtsp);
        if let StreamKind::Server { encoder, .. } = &mut kind {
            *encoder = "libopenh264".into();
        }
        let args = output_args(&kind, Some("libopenh264"), 30).unwrap();
        assert_eq!(&args[..2], ["-c:v", "libopenh264"]);
    }

    #[test]
    fn rtsp_frames_reach_a_server() {
        publish_to_a_listener(
            ServerProtocol::Rtsp,
            // An explicit listen timeout: newer FFmpegs (Homebrew's) refuse the
            // endless default of `-rtsp_flags listen` as "-1000 out of range".
            &["-listen_timeout", "30", "-i", "rtsp://127.0.0.1:18554/newq"],
            18554,
        );
    }

    #[test]
    fn rtmp_frames_reach_a_server() {
        publish_to_a_listener(
            ServerProtocol::Rtmp,
            &["-listen", "1", "-i", "rtmp://127.0.0.1:11935/newq"],
            11935,
        );
    }
}
