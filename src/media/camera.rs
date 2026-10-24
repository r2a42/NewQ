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

/// The ffmpeg input options (everything up to and including `-i <input>`)
/// that open `device` in the given mode. Empty if no device is chosen.
pub fn input_args(platform: Platform, device: &str, w: u32, h: u32, fps: u32) -> Vec<String> {
    let s = |v: &str| v.to_string();
    if device.is_empty() {
        return Vec::new();
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
        }
    }
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
