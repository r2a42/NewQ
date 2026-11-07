//! Cameras and capture cards: finding them, and the ffmpeg input that opens one.
//!
//! Each platform has its own capture API, and ffmpeg reaches all of them as an
//! input device: Video4Linux2 on Linux, DirectShow on Windows, AVFoundation on
//! macOS. The pure parts (the command line, reading ffmpeg's device lists) are
//! here and tested; the devices themselves can only be tried on real hardware,
//! so the built-in test picture stands in for a camera in the tests.

use super::ffmpeg::Ffmpeg;
use crate::show::{CaptureFormat, TEST_PATTERN_DEVICE};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Windows,
    MacOs,
}

impl Platform {
    pub const fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// A camera cue's device that is a picture of the screen, so that what
/// another program shows (a Spout or Syphon sender's window, a media
/// server's output) can be taken in: `screen:` and what to capture.
pub const SCREEN_PREFIX: &str = "screen:";

/// The whole desktop (or the main screen).
pub const SCREEN_DESKTOP: &str = "screen:desktop";

/// What a screen device captures: `desktop`, a screen's number (macOS), or
/// `title=<a window's title>` (Windows).
pub fn screen_target(device: &str) -> Option<&str> {
    device.strip_prefix(SCREEN_PREFIX)
}

/// The ffmpeg input options that capture the screen device `target`.
/// `display` is the X display (Linux).
pub fn screen_args(platform: Platform, target: &str, fps: u32, display: &str) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let rate = [s("-framerate"), fps.to_string()];
    match platform {
        Platform::Linux => [s("-f"), s("x11grab")]
            .into_iter()
            .chain(rate)
            .chain([s("-draw_mouse"), s("0"), s("-i"), display.to_string()])
            .collect(),
        Platform::Windows => [s("-f"), s("gdigrab")]
            .into_iter()
            .chain(rate)
            .chain([s("-draw_mouse"), s("0"), s("-i")])
            .chain([if target == "desktop" {
                s("desktop")
            } else {
                target.to_string()
            }])
            .collect(),
        Platform::MacOs => [s("-f"), s("avfoundation")]
            .into_iter()
            .chain(rate)
            .chain([s("-capture_cursor"), s("0"), s("-i")])
            .chain([format!("{}:none", macos_screen_index(target))])
            .collect(),
    }
}

/// An AVFoundation screen: `desktop` is the first, otherwise it is the
/// device number from the list (`Capture screen 0` is usually device 1).
fn macos_screen_index(target: &str) -> String {
    match target {
        "desktop" | "" => "Capture screen 0".to_string(),
        n if n.chars().all(|c| c.is_ascii_digit()) => format!("Capture screen {n}"),
        other => other.to_string(),
    }
}

/// The ffmpeg input options (everything up to and including `-i <input>`)
/// that open `device` in the given mode. Empty if no device is chosen.
pub fn input_args(
    platform: Platform,
    device: &str,
    w: u32,
    h: u32,
    fps: u32,
    format: CaptureFormat,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    if device.is_empty() {
        return Vec::new();
    }
    if let Some(target) = screen_target(device) {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        return screen_args(platform, target, fps, &display);
    }
    if device == TEST_PATTERN_DEVICE {
        // `-re` paces the generator at real time, as a camera would be.
        return vec![
            s("-re"),
            s("-f"),
            s("lavfi"),
            s("-i"),
            format!("testsrc2=size={w}x{h}:rate={fps}"),
        ];
    }
    let mode = [
        s("-framerate"),
        fps.to_string(),
        s("-video_size"),
        format!("{w}x{h}"),
    ]
    .into_iter()
    .chain(format_args(platform, format))
    .collect::<Vec<_>>();
    match platform {
        Platform::Linux => [s("-f"), s("v4l2")]
            .into_iter()
            .chain(mode)
            .chain([s("-i"), device.to_string()])
            .collect(),
        Platform::Windows => [s("-f"), s("dshow"), s("-rtbufsize"), s("64M")]
            .into_iter()
            .chain(mode)
            .chain([s("-i"), format!("video={device}")])
            .collect(),
        Platform::MacOs => [s("-f"), s("avfoundation")]
            .into_iter()
            .chain(mode)
            .chain([s("-i"), format!("{device}:none")])
            .collect(),
    }
}

/// The options that ask a device for `format`. DirectShow takes a raw pixel
/// format or a codec, Video4Linux one name for either, and AVFoundation only a
/// raw pixel format (it hands over decoded pictures, so a compressed format is
/// not something it can be asked for).
fn format_args(platform: Platform, format: CaptureFormat) -> Vec<String> {
    let s = |v: &str| v.to_string();
    match (platform, format.pixel_format(), format.codec()) {
        (_, None, None) => Vec::new(),
        (Platform::Windows, Some(p), _) => vec![s("-pixel_format"), s(p)],
        (Platform::Windows, None, Some(c)) => vec![s("-vcodec"), s(c)],
        (Platform::Linux, Some(p), _) => vec![s("-input_format"), s(p)],
        (Platform::Linux, None, Some(c)) => vec![s("-input_format"), s(c)],
        (Platform::MacOs, Some(p), _) => vec![s("-pixel_format"), s(p)],
        (Platform::MacOs, None, Some(_)) => Vec::new(),
    }
}

/// One mode a device says it offers: a size, a frame rate (0 when the system
/// doesn't say) and a format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMode {
    pub width: u32,
    pub height: u32,
    /// The most frames a second it offers at this size; 0 if not said.
    pub fps: u32,
    /// FFmpeg's name for the format (`nv12`, `mjpeg`), if the system says.
    pub format_name: String,
}

impl DeviceMode {
    /// The `CaptureFormat` that asks for this mode's format, when there is one.
    pub fn format(&self) -> Option<CaptureFormat> {
        CaptureFormat::ALL.into_iter().find(|f| {
            f.pixel_format() == Some(self.format_name.as_str())
                || f.codec() == Some(self.format_name.as_str())
        })
    }

    pub fn label(&self) -> String {
        let mut text = format!("{} × {}", self.width, self.height);
        if self.fps > 0 {
            text += &format!(" · up to {} fps", self.fps);
        }
        if !self.format_name.is_empty() {
            text += &format!(" · {}", self.format_name);
        }
        text
    }
}

/// `1920x1080` as (1920, 1080).
fn parse_size(token: &str) -> Option<(u32, u32)> {
    let (w, h) = token
        .trim_matches(|c: char| !c.is_ascii_digit() && c != 'x')
        .split_once('x')?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0 && w <= 16384 && h <= 16384).then_some((w, h))
}

/// The modes a device offers, most pixels first, then most frames; the same
/// mode twice is one.
fn tidy(mut modes: Vec<DeviceMode>) -> Vec<DeviceMode> {
    modes.sort_by(|a, b| {
        (b.width * b.height, b.fps, &b.format_name).cmp(&(
            a.width * a.height,
            a.fps,
            &a.format_name,
        ))
    });
    modes.dedup();
    modes
}

/// DirectShow's `-list_options` lines, like
/// `[dshow @ 1]   pixel_format=nv12  min s=1920x1080 fps=30 max s=3840x2160 fps=60`
/// (`vcodec=mjpeg` for a compressed one). The most the line says is taken.
pub fn parse_dshow_modes(text: &str) -> Vec<DeviceMode> {
    let mut modes = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find("pixel_format=").or_else(|| line.find("vcodec=")) else {
            continue;
        };
        let rest = &line[at..];
        let (key, rest) = rest.split_once('=').unwrap_or(("", rest));
        let _ = key;
        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        let Some(max) = line.find("max s=") else {
            continue;
        };
        let mut words = line[max + "max s=".len()..].split_whitespace();
        let Some((width, height)) = words.next().and_then(parse_size) else {
            continue;
        };
        let fps = words
            .next()
            .and_then(|w| w.strip_prefix("fps="))
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(0, |v| v.round() as u32);
        modes.push(DeviceMode {
            width,
            height,
            fps,
            format_name: name,
        });
    }
    tidy(modes)
}

/// Video4Linux's `-list_formats all` lines, like
/// `[video4linux2,v4l2 @ 1] Raw       :     yuyv422 :           YUYV 4:2:2 : 640x480 1280x720`
/// and `Compressed:       mjpeg :          Motion-JPEG : 1920x1080`. They give
/// sizes but no frame rates.
pub fn parse_v4l2_modes(text: &str) -> Vec<DeviceMode> {
    let mut modes = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find("] ") else { continue };
        let body = line[at + 2..].trim_start();
        if !(body.starts_with("Raw") || body.starts_with("Compressed")) {
            continue;
        }
        let Some((_, rest)) = body.split_once(':') else {
            continue;
        };
        let mut parts = rest.trim_start().splitn(3, " : ");
        let (Some(name), Some(_description), Some(sizes)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name.contains('?') {
            continue;
        }
        for token in sizes.split_whitespace() {
            if let Some((width, height)) = parse_size(token) {
                modes.push(DeviceMode {
                    width,
                    height,
                    fps: 0,
                    format_name: name.to_string(),
                });
            }
        }
    }
    tidy(modes)
}

/// What AVFoundation prints when asked for a size it doesn't have: lines like
/// `[avfoundation @ 1]   3840x2160@[24.000000 60.000000]fps`. It names no formats.
pub fn parse_avfoundation_modes(text: &str) -> Vec<DeviceMode> {
    let mut modes = Vec::new();
    for line in text.lines() {
        let Some((before, after)) = line.split_once("@[") else {
            continue;
        };
        let Some(size) = before.split_whitespace().last().and_then(parse_size) else {
            continue;
        };
        let Some((range, _)) = after.split_once(']') else {
            continue;
        };
        let fps = range
            .split_whitespace()
            .filter_map(|v| v.parse::<f64>().ok())
            .fold(0.0_f64, f64::max);
        modes.push(DeviceMode {
            width: size.0,
            height: size.1,
            fps: fps.round() as u32,
            format_name: String::new(),
        });
    }
    tidy(modes)
}

/// Ask `ff` what modes `device` offers: each system's own listing, which
/// FFmpeg prints as a side effect (AVFoundation only when asked for a mode it
/// hasn't got, so a size of 1 x 1 is asked for). A device that is in use may
/// not answer.
pub fn list_modes(
    ff: &Ffmpeg,
    platform: Platform,
    device: &str,
) -> Result<Vec<DeviceMode>, String> {
    if device.is_empty() {
        return Err("no camera chosen".into());
    }
    let (text, parse): (String, fn(&str) -> Vec<DeviceMode>) = match platform {
        Platform::Windows => (
            list_output(
                ff,
                &[
                    "-list_options",
                    "true",
                    "-f",
                    "dshow",
                    "-i",
                    &format!("video={device}"),
                ],
            ),
            parse_dshow_modes,
        ),
        Platform::Linux => (
            list_output(ff, &["-f", "v4l2", "-list_formats", "all", "-i", device]),
            parse_v4l2_modes,
        ),
        Platform::MacOs => (
            list_output(
                ff,
                &[
                    "-f",
                    "avfoundation",
                    "-video_size",
                    "1x1",
                    "-framerate",
                    "30",
                    "-i",
                    &format!("{device}:none"),
                ],
            ),
            parse_avfoundation_modes,
        ),
    };
    let modes = parse(&text);
    if modes.is_empty() {
        Err(
            "the device didn't list any modes (it may be in use by another program or cue, \
             or not be one this FFmpeg can ask)"
                .into(),
        )
    } else {
        Ok(modes)
    }
}

/// The devices ffmpeg can open on this machine, by the name `input_args`
/// wants. The test picture is always first.
pub fn list_devices(ff: &Ffmpeg) -> Vec<String> {
    let mut devices = vec![TEST_PATTERN_DEVICE.to_string()];
    // The screen: for what other programs on this computer show. (Linux: only
    // where there is an X display, which Wayland-only sessions lack.)
    if Platform::current() != Platform::Linux || std::env::var_os("DISPLAY").is_some() {
        devices.push(SCREEN_DESKTOP.to_string());
    }
    match Platform::current() {
        Platform::Linux => devices.extend(v4l2_devices()),
        Platform::Windows => {
            let text = list_output(ff, &["-list_devices", "true", "-f", "dshow", "-i", "dummy"]);
            devices.extend(parse_dshow_devices(&text));
        }
        Platform::MacOs => {
            let text = list_output(
                ff,
                &["-f", "avfoundation", "-list_devices", "true", "-i", ""],
            );
            devices.extend(parse_avfoundation_devices(&text));
            // Further screens, by number (the first is the desktop above).
            devices.extend(
                parse_avfoundation_screens(&text)
                    .into_iter()
                    .skip(1)
                    .map(|n| format!("{SCREEN_PREFIX}{n}")),
            );
        }
    }
    // NDI sources on the network, if NDI's runtime is installed (asked of a
    // helper, which takes a couple of seconds). Not in tests: the test program
    // is not NewQ's and can't be a helper.
    #[cfg(not(test))]
    devices.extend(
        crate::ndi::list_sources()
            .iter()
            .map(|n| crate::ndi::device(n)),
    );
    devices
}

/// ffmpeg prints device lists to stderr as a side effect of failing to open
/// the dummy input, so run it and keep the text.
fn list_output(ff: &Ffmpeg, args: &[&str]) -> String {
    let mut cmd: Command = ff.command();
    cmd.args(["-loglevel", "info"]).args(args);
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stderr).into_owned(),
        Err(_) => String::new(),
    }
}

/// Video4Linux capture nodes, e.g. `/dev/video0`. A camera also exposes
/// metadata nodes (index 1, 3, ...) that can't be opened as a picture.
fn v4l2_devices() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };
    let mut nodes: Vec<(u32, String)> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("video")?.parse().ok()?;
            let index = std::fs::read_to_string(e.path().join("index")).unwrap_or_default();
            (index.trim().is_empty() || index.trim() == "0").then(|| (n, format!("/dev/{name}")))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().map(|(_, path)| path).collect()
}

/// The friendly name of a Video4Linux device, for the picker.
pub fn v4l2_label(device: &str) -> Option<String> {
    let node = device.strip_prefix("/dev/")?;
    let name = std::fs::read_to_string(format!("/sys/class/video4linux/{node}/name")).ok()?;
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

/// DirectShow lists lines like `[dshow @ 000001] "Integrated Camera" (video)`;
/// older builds list the names under a "DirectShow video devices" heading.
pub fn parse_dshow_devices(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_video = false;
    for line in text.lines() {
        let tagged = line.contains("(video") || line.contains("(audio") || line.contains("(none)");
        if line.contains("DirectShow video devices") {
            in_video = true;
        } else if line.contains("DirectShow audio devices") {
            in_video = false;
        }
        let Some(start) = line.find('"') else {
            continue;
        };
        let Some(len) = line[start + 1..].find('"') else {
            continue;
        };
        let name = &line[start + 1..start + 1 + len];
        if line.contains("Alternative name") || name.is_empty() {
            continue;
        }
        let video = if tagged {
            line.contains("(video")
        } else {
            in_video
        };
        if video && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
    }
    found
}

/// AVFoundation lists `[AVFoundation indev @ 0x1] [0] FaceTime HD Camera`
/// under a "video devices" heading, then the audio devices.
pub fn parse_avfoundation_devices(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_video = false;
    for line in text.lines() {
        if line.contains("video devices") {
            in_video = true;
            continue;
        }
        if line.contains("audio devices") {
            in_video = false;
            continue;
        }
        if !in_video {
            continue;
        }
        // Skip the "[AVFoundation indev @ ...]" prefix, then read "[n] Name".
        let rest = line.rsplit_once("] [").map(|(_, r)| r);
        let Some((index, name)) = rest.and_then(|r| r.split_once("] ")) else {
            continue;
        };
        if index.chars().all(|c| c.is_ascii_digit())
            && !name.trim().is_empty()
            && !name.starts_with("Capture screen")
        {
            found.push(name.trim().to_string());
        }
    }
    found
}

/// The numbers of the screens AVFoundation can capture: the `n` of each
/// `Capture screen n` in its list.
pub fn parse_avfoundation_screens(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|l| l.split_once("Capture screen ").map(|(_, n)| n))
        .filter_map(|n| n.trim().parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_is_asked_for_the_way_each_platform_takes_it() {
        let a = |p, d: &str, f| input_args(p, d, 3840, 2160, 60, f).join(" ");
        // DirectShow: a raw pixel format, or a codec.
        assert_eq!(
            a(Platform::Windows, "Game Capture 4K X", CaptureFormat::Nv12),
            "-f dshow -rtbufsize 64M -framerate 60 -video_size 3840x2160 -pixel_format nv12 -i video=Game Capture 4K X"
        );
        assert_eq!(
            a(Platform::Windows, "Cam", CaptureFormat::Mjpeg),
            "-f dshow -rtbufsize 64M -framerate 60 -video_size 3840x2160 -vcodec mjpeg -i video=Cam"
        );
        assert!(a(Platform::Windows, "Cam", CaptureFormat::H264).contains("-vcodec h264"));
        assert!(a(Platform::Windows, "Cam", CaptureFormat::Yuy2).contains("-pixel_format yuyv422"));
        assert!(a(Platform::Windows, "Cam", CaptureFormat::Uyvy).contains("-pixel_format uyvy422"));
        // Video4Linux: one option for either.
        assert_eq!(
            a(Platform::Linux, "/dev/video2", CaptureFormat::Mjpeg),
            "-f v4l2 -framerate 60 -video_size 3840x2160 -input_format mjpeg -i /dev/video2"
        );
        assert!(
            a(Platform::Linux, "/dev/video2", CaptureFormat::Yuy2)
                .contains("-input_format yuyv422")
        );
        // AVFoundation: raw formats only; a compressed one can't be asked for.
        assert_eq!(
            a(Platform::MacOs, "Game Capture 4K S", CaptureFormat::Uyvy),
            "-f avfoundation -framerate 60 -video_size 3840x2160 -pixel_format uyvy422 -i Game Capture 4K S:none"
        );
        assert_eq!(
            a(Platform::MacOs, "Cam", CaptureFormat::Mjpeg),
            a(Platform::MacOs, "Cam", CaptureFormat::Auto)
        );
        // Automatic adds nothing, and the test picture and screens take no format.
        assert!(!a(Platform::Windows, "Cam", CaptureFormat::Auto).contains("pixel_format"));
        assert!(
            !input_args(
                Platform::Linux,
                "NewQ test pattern",
                640,
                360,
                30,
                CaptureFormat::Nv12
            )
            .join(" ")
            .contains("nv12")
        );
        assert!(
            !input_args(
                Platform::Windows,
                "screen:desktop",
                640,
                360,
                30,
                CaptureFormat::Nv12
            )
            .join(" ")
            .contains("nv12")
        );
    }

    #[test]
    fn reads_the_modes_dshow_lists_taking_the_most_each_line_says() {
        let text = r#"[dshow @ 000001e5e4c4d0c0] DirectShow video device options (from video devices)
[dshow @ 000001e5e4c4d0c0]  Pin "Capture" (alternative pin name "Capture")
[dshow @ 000001e5e4c4d0c0]   pixel_format=nv12  min s=3840x2160 fps=24 max s=3840x2160 fps=60
[dshow @ 000001e5e4c4d0c0]   pixel_format=yuyv422  min s=1920x1080 fps=5 max s=1920x1080 fps=30
[dshow @ 000001e5e4c4d0c0]   pixel_format=nv12  min s=1920x1080 fps=5 max s=1920x1080 fps=119.88
[dshow @ 000001e5e4c4d0c0]   vcodec=mjpeg  min s=1280x720 fps=5 max s=1280x720 fps=60
[dshow @ 000001e5e4c4d0c0]   vcodec=mjpeg  min s=1280x720 fps=5 max s=1280x720 fps=60
Could not set video options
"#;
        let modes = parse_dshow_modes(text);
        // Most pixels first, then most frames; a repeated line is one mode.
        let got: Vec<(u32, u32, u32, &str)> = modes
            .iter()
            .map(|m| (m.width, m.height, m.fps, m.format_name.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (3840, 2160, 60, "nv12"),
                (1920, 1080, 120, "nv12"),
                (1920, 1080, 30, "yuyv422"),
                (1280, 720, 60, "mjpeg"),
            ]
        );
        assert_eq!(modes[0].format(), Some(CaptureFormat::Nv12));
        assert_eq!(modes[3].format(), Some(CaptureFormat::Mjpeg));
        assert_eq!(modes[0].label(), "3840 × 2160 · up to 60 fps · nv12");
        assert!(parse_dshow_modes("nothing to see\n").is_empty());
    }

    #[test]
    fn reads_the_formats_and_sizes_video4linux_lists() {
        let text = "\
[video4linux2,v4l2 @ 0x55d0c8e3d2c0] Compressed:       mjpeg :          Motion-JPEG : 3840x2160 1920x1080 1280x720
[video4linux2,v4l2 @ 0x55d0c8e3d2c0] Raw       :     yuyv422 :           YUYV 4:2:2 : 1920x1080 1280x720
[video4linux2,v4l2 @ 0x55d0c8e3d2c0] Raw       :        nv12 :         Y/UV 4:2:0 : 3840x2160
[video4linux2,v4l2 @ 0x55d0c8e3d2c0] Raw       :  Unsupported? :        Some other : 640x480
/dev/video2: Immediate exit requested
";
        let modes = parse_v4l2_modes(text);
        // Largest first (4K is offered in two formats), then the rest.
        assert_eq!((modes[0].width, modes[1].width), (3840, 3840));
        assert!(
            modes
                .iter()
                .any(|m| m.width == 3840 && m.format_name == "mjpeg")
        );
        assert!(modes.iter().all(|m| m.fps == 0), "it gives no frame rates");
        assert!(modes.iter().any(|m| m.width == 3840
            && m.height == 2160
            && m.format_name == "nv12"
            && m.format() == Some(CaptureFormat::Nv12)));
        assert!(
            modes
                .iter()
                .any(|m| m.format_name == "yuyv422" && m.width == 1280)
        );
        // An unsupported format is not offered.
        assert!(!modes.iter().any(|m| m.width == 640));
        assert_eq!(modes.len(), 6);
    }

    #[test]
    fn reads_the_modes_avfoundation_lists_when_asked_for_one_it_lacks() {
        let text = "\
[avfoundation @ 0x7f8a4b004a00] Selected video size (1x1) is not supported by the device.
[avfoundation @ 0x7f8a4b004a00] Supported modes:
[avfoundation @ 0x7f8a4b004a00]   3840x2160@[24.000000 60.000000]fps
[avfoundation @ 0x7f8a4b004a00]   1920x1080@[29.970000 59.940000]fps
[avfoundation @ 0x7f8a4b004a00]   1280x720@[1.000000 30.000000]fps
Game Capture 4K S:none: Immediate exit requested
";
        let modes = parse_avfoundation_modes(text);
        let got: Vec<(u32, u32, u32)> = modes.iter().map(|m| (m.width, m.height, m.fps)).collect();
        assert_eq!(
            got,
            vec![(3840, 2160, 60), (1920, 1080, 60), (1280, 720, 30)]
        );
        assert!(
            modes
                .iter()
                .all(|m| m.format_name.is_empty() && m.format().is_none())
        );
    }

    #[test]
    fn a_size_token_is_read_only_when_it_is_a_size() {
        assert_eq!(parse_size("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_size("(1920x1080),"), Some((1920, 1080)));
        for bad in ["", "x", "0x0", "1920x", "axb", "99999x99999", "1920"] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn builds_the_input_for_each_platform() {
        let a = |p, d: &str| input_args(p, d, 1280, 720, 30, CaptureFormat::Auto).join(" ");
        assert_eq!(
            a(Platform::Linux, "/dev/video0"),
            "-f v4l2 -framerate 30 -video_size 1280x720 -i /dev/video0"
        );
        assert_eq!(
            a(Platform::Windows, "Integrated Camera"),
            "-f dshow -rtbufsize 64M -framerate 30 -video_size 1280x720 -i video=Integrated Camera"
        );
        assert_eq!(
            a(Platform::MacOs, "FaceTime HD Camera"),
            "-f avfoundation -framerate 30 -video_size 1280x720 -i FaceTime HD Camera:none"
        );
        // The same test picture everywhere, paced like a live source.
        assert_eq!(
            a(Platform::Windows, TEST_PATTERN_DEVICE),
            "-re -f lavfi -i testsrc2=size=1280x720:rate=30"
        );
        assert!(input_args(Platform::Linux, "", 1280, 720, 30, CaptureFormat::Auto).is_empty());
    }

    #[test]
    fn screens_are_captured_by_each_platforms_own_grabber() {
        let a = |p, t: &str| screen_args(p, t, 30, ":0.0").join(" ");
        assert_eq!(
            a(Platform::Linux, "desktop"),
            "-f x11grab -framerate 30 -draw_mouse 0 -i :0.0"
        );
        assert_eq!(
            a(Platform::Windows, "desktop"),
            "-f gdigrab -framerate 30 -draw_mouse 0 -i desktop"
        );
        // A window, by its title.
        assert_eq!(
            a(Platform::Windows, "title=Resolume Arena"),
            "-f gdigrab -framerate 30 -draw_mouse 0 -i title=Resolume Arena"
        );
        assert_eq!(
            a(Platform::MacOs, "desktop"),
            "-f avfoundation -framerate 30 -capture_cursor 0 -i Capture screen 0:none"
        );
        assert_eq!(
            a(Platform::MacOs, "1"),
            "-f avfoundation -framerate 30 -capture_cursor 0 -i Capture screen 1:none"
        );
        // A screen device goes through `input_args` to those.
        let via = input_args(
            Platform::Windows,
            "screen:title=OBS",
            1280,
            720,
            25,
            CaptureFormat::Auto,
        )
        .join(" ");
        assert_eq!(via, "-f gdigrab -framerate 25 -draw_mouse 0 -i title=OBS");
        assert_eq!(screen_target(SCREEN_DESKTOP), Some("desktop"));
        assert_eq!(screen_target("/dev/video0"), None);
    }

    #[test]
    fn reads_the_screens_avfoundation_can_capture() {
        let text = "\
[AVFoundation indev @ 0x7f8] [0] FaceTime HD Camera
[AVFoundation indev @ 0x7f8] [1] Capture screen 0
[AVFoundation indev @ 0x7f8] [2] Capture screen 1
";
        assert_eq!(parse_avfoundation_screens(text), [0, 1]);
        assert!(parse_avfoundation_screens("").is_empty());
    }

    #[test]
    fn reads_the_dshow_device_list() {
        let new = "\
[dshow @ 0000020] \"Integrated Camera\" (video)
[dshow @ 0000020]   Alternative name \"@device_pnp_\\\\?\\usb#vid_0c45\"
[dshow @ 0000020] \"Microphone (Realtek)\" (audio)
[dshow @ 0000020] \"Blackmagic WDM Capture\" (video)
[dshow @ 0000020] \"OBS Virtual Camera\" (video, none)
";
        assert_eq!(
            parse_dshow_devices(new),
            [
                "Integrated Camera",
                "Blackmagic WDM Capture",
                "OBS Virtual Camera"
            ]
        );
        let old = "\
[dshow @ 0000020] DirectShow video devices (some may be both video and audio devices)
[dshow @ 0000020]  \"USB Capture\"
[dshow @ 0000020]     Alternative name \"@device_pnp_x\"
[dshow @ 0000020] DirectShow audio devices
[dshow @ 0000020]  \"Line In\"
";
        assert_eq!(parse_dshow_devices(old), ["USB Capture"]);
        assert!(parse_dshow_devices("").is_empty());
    }

    #[test]
    fn reads_the_avfoundation_device_list() {
        let text = "\
[AVFoundation indev @ 0x7f8] AVFoundation video devices:
[AVFoundation indev @ 0x7f8] [0] FaceTime HD Camera
[AVFoundation indev @ 0x7f8] [1] Capture screen 0
[AVFoundation indev @ 0x7f8] [2] Blackmagic Design UltraStudio
[AVFoundation indev @ 0x7f8] AVFoundation audio devices:
[AVFoundation indev @ 0x7f8] [0] MacBook Pro Microphone
";
        assert_eq!(
            parse_avfoundation_devices(text),
            ["FaceTime HD Camera", "Blackmagic Design UltraStudio"]
        );
    }

    #[test]
    fn the_test_picture_is_always_listed() {
        if let Some(ff) = Ffmpeg::locate() {
            assert_eq!(list_devices(&ff)[0], TEST_PATTERN_DEVICE);
        }
    }
}
