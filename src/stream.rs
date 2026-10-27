//! Sending a display's picture out of NewQ as a video stream: to a virtual
//! camera (Linux, through v4l2loopback), over the network, or wherever an
//! FFmpeg output can go. Frames are handed to an FFmpeg process on its
//! standard input and it does the rest.
//!
//! Windows and macOS have no virtual camera NewQ can make without a signed
//! driver or system extension; the network stream goes to OBS, whose own
//! virtual camera Zoom and Teams can use.

use crate::media::ffmpeg::Ffmpeg;
use crate::show::StreamKind;
use std::io::Write;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::Arc;
use std::sync::OnceLock;

/// The size to send a picture of `canvas` at, `height` tall, with the same
/// shape and even sides (video wants that).
pub fn size_for(canvas: (u32, u32), height: u32) -> (u32, u32) {
    let height = height.clamp(16, 4320) & !1;
    let width = (canvas.0.max(1) as f64 * height as f64 / canvas.1.max(1) as f64).round() as u32;
    (width.clamp(16, 16_384) & !1, height)
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

/// FFmpeg's output arguments for `kind`. `x264` says whether this FFmpeg can
/// encode H.264, which is what the network stream prefers.
pub fn output_args(kind: &StreamKind, x264: bool) -> Result<Vec<String>, String> {
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
            let mut a = if x264 {
                list(&[
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-tune",
                    "zerolatency",
                    "-pix_fmt",
                    "yuv420p",
                    "-g",
                    "30",
                ])
            } else {
                list(&["-c:v", "mpeg4", "-q:v", "3", "-pix_fmt", "yuv420p"])
            };
            a.extend(list(&["-f", "mpegts"]));
            a.push(format!("udp://{host}:{port}?pkt_size=1316"));
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

/// Whether `ff` can encode H.264 (asked once).
pub fn has_x264(ff: &Ffmpeg) -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let mut cmd = ff.command();
        cmd.args(["-encoders"]);
        cmd.output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"))
            .unwrap_or(false)
    })
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
            let output = output_args(kind, has_x264(ff))?;
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
                    let mut text = String::new();
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = std::io::Read::read(&mut err, &mut buf) {
                        if n == 0 {
                            break;
                        }
                        text.push_str(&String::from_utf8_lossy(&buf[..n]));
                        // The end is what matters.
                        if text.len() > 4000 {
                            text = text.split_off(text.len() - 2000);
                        }
                        *said.lock() = text.clone();
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
        // Nonsense is kept in range.
        let (w, h) = size_for((0, 0), 0);
        assert!(w >= 16 && h >= 16 && w % 2 == 0 && h % 2 == 0);
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
        let a = output_args(&udp, true).unwrap().join(" ");
        assert!(
            a.contains("libx264") && a.ends_with("udp://127.0.0.1:5000?pkt_size=1316"),
            "{a}"
        );
        let b = output_args(&udp, false).unwrap().join(" ");
        assert!(b.contains("mpeg4") && !b.contains("libx264"), "{b}");
        assert!(
            output_args(
                &StreamKind::Udp {
                    host: " ".into(),
                    port: 1
                },
                true
            )
            .is_err()
        );

        let custom = StreamKind::Custom {
            args: "-f flv rtmp://x/y".into(),
        };
        assert_eq!(
            output_args(&custom, true).unwrap(),
            ["-f", "flv", "rtmp://x/y"]
        );
        assert!(output_args(&StreamKind::Custom { args: "".into() }, true).is_err());

        let cam = StreamKind::VirtualCamera {
            device: "/dev/video10".into(),
        };
        if cfg!(target_os = "linux") {
            assert_eq!(
                output_args(&cam, true).unwrap().join(" "),
                "-pix_fmt yuyv422 -f v4l2 /dev/video10"
            );
            assert!(output_args(&StreamKind::VirtualCamera { device: "".into() }, true).is_err());
        } else {
            assert!(output_args(&cam, true).unwrap_err().contains("OBS"));
        }
    }
}
