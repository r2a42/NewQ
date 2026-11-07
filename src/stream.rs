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
        "h264_amf" => add(&["-usage", "lowlatency", "-pix_fmt", "yuv420p"]),
        "libopenh264" => add(&["-allow_skip_frames", "0", "-pix_fmt", "yuv420p"]),
        _ => add(&["-pix_fmt", "yuv420p"]),
    }
    add(&[
        "-b:v", &rate, "-maxrate", &rate, "-bufsize", &buf, "-g", &gop,
    ]);
    a
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
pub fn output_args(kind: &StreamKind, h264: Option<&str>, fps: u32) -> Result<Vec<String>, String> {
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
        } => {
            let url = server_url(*protocol, host, *port, path)?;
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
        .args(h264_args(encoder, 1000, 30))
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

/// Keep only the end of what FFmpeg has said, once it has said a lot. Cut as
/// bytes, not as a string: FFmpeg's messages have characters of more than one
/// byte in them (quotes, file names), and a string cut inside one is a panic.
fn keep_tail(text: &mut Vec<u8>) {
    if text.len() > 4000 {
        text.drain(..text.len() - 2000);
    }
}

/// A running FFmpeg taking frames on its standard input.
pub struct Sink {
    child: Child,
    stdin: Option<ChildStdin>,
    /// What FFmpeg has said on its error output.
    said: Arc<parking_lot::Mutex<String>>,
}

impl Sink {
    pub fn start(
        ff: &Ffmpeg,
        size: (u32, u32),
        fps: u32,
        kind: &StreamKind,
    ) -> Result<Sink, String> {
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
            let output = output_args(kind, encoder(ff), fps)?;
            let mut cmd = ff.command();
            cmd.args(input_args(size, fps)).args(output);
            cmd
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start FFmpeg: {e}"))?;
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
        Ok(Sink { child, stdin, said })
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
                let last = said.lines().last().unwrap_or("").to_string();
                Err(if last.is_empty() {
                    format!("FFmpeg stopped: {e}")
                } else {
                    format!("FFmpeg stopped: {last}")
                })
            }
        }
    }

    /// Whether the process is still going, and if not what it said.
    pub fn failed(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                let said = self.said.lock().trim().to_string();
                Some(said.lines().last().unwrap_or("FFmpeg stopped").to_string())
            }
            _ => None,
        }
    }

    /// Close the input so FFmpeg finishes what it has, and wait for it.
    pub fn finish(mut self) {
        self.stdin = None;
        let _ = self.child.wait();
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    fn frames_go_in_as_raw_rgba_of_the_size_given() {
        let a = input_args((1280, 720), 30);
        let line = a.join(" ");
        assert_eq!(
            line,
            "-f rawvideo -pix_fmt rgba -s 1280x720 -framerate 30 -i pipe:0"
        );
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
            .stderr(Stdio::null());
        let mut rx = Receiver(rx.spawn().expect("start the receiver"));
        let kind = StreamKind::Server {
            protocol,
            host: "127.0.0.1".into(),
            port,
            path: "newq".into(),
            kbps: 1000,
        };
        let size = (320u32, 180u32);
        let frame = [40u8, 120, 200, 255].repeat((size.0 * size.1) as usize);
        // The receiver takes a while to listen on a cold machine: try until it does.
        let mut sink = None;
        for _ in 0..15 {
            let mut s = Sink::start(&ff, size, 25, &kind).expect("start the sink");
            std::thread::sleep(std::time::Duration::from_millis(600));
            if s.failed().is_none() && s.write(&frame).is_ok() {
                sink = Some(s);
                break;
            }
        }
        let mut sink = sink.expect("the receiver never took the stream");
        for _ in 0..50 {
            sink.write(&frame).expect("a frame goes out");
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        sink.finish();
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
    fn rtsp_frames_reach_a_server() {
        publish_to_a_listener(
            ServerProtocol::Rtsp,
            &["-rtsp_flags", "listen", "-i", "rtsp://127.0.0.1:18554/newq"],
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
