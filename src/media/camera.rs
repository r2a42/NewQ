//! Cameras and capture cards: finding them, and the ffmpeg input that opens one.
//!
//! Each platform has its own capture API, and ffmpeg reaches all of them as an
//! input device: Video4Linux2 on Linux, DirectShow on Windows, AVFoundation on
//! macOS. The pure parts (the command line, reading ffmpeg's device lists) are
//! here and tested; the devices themselves can only be tried on real hardware,
//! so the built-in test picture stands in for a camera in the tests.

use super::ffmpeg::Ffmpeg;
use crate::show::TEST_PATTERN_DEVICE;
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
pub fn input_args(platform: Platform, device: &str, w: u32, h: u32, fps: u32) -> Vec<String> {
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
    ];
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
    fn builds_the_input_for_each_platform() {
        let a = |p, d: &str| input_args(p, d, 1280, 720, 30).join(" ");
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
        assert!(input_args(Platform::Linux, "", 1280, 720, 30).is_empty());
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
        let via = input_args(Platform::Windows, "screen:title=OBS", 1280, 720, 25).join(" ");
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
