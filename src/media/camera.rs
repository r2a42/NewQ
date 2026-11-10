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

/// A camera cue's device that is a Blackmagic (DeckLink) card, such as an
/// UltraStudio: `decklink:` and the card's name. FFmpeg reaches them through
/// its `decklink` input, which only some builds have.
pub const DECKLINK_PREFIX: &str = "decklink:";

/// The card's name, if `device` is a DeckLink one.
pub fn decklink_name(device: &str) -> Option<&str> {
    device.strip_prefix(DECKLINK_PREFIX)
}

/// Whether the FFmpeg NewQ is using can read DeckLink cards, found when the
/// cameras were last looked for. Blackmagic's SDK terms stop NewQ shipping an
/// FFmpeg that can, so it is only there when the person has their own build.
static DECKLINK_SUPPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn decklink_supported() -> bool {
    DECKLINK_SUPPORTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// What to tell someone whose FFmpeg can't read a DeckLink card.
pub const DECKLINK_UNSUPPORTED: &str = "This FFmpeg can't read Blackmagic (DeckLink) cards. NewQ's own FFmpeg can't, because \
     Blackmagic's SDK terms don't let it be shipped. Use an FFmpeg built with --enable-decklink \
     (and Blackmagic's Desktop Video installed) by putting it in the folder named by NEWQ_FFMPEG_DIR.";

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
    code: &str,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    if device.is_empty() {
        return Vec::new();
    }
    if let Some(name) = decklink_name(device) {
        // A Blackmagic card is asked for its mode by name (`-list_formats`
        // lists them); with none given it detects what it is sent.
        let mut a = vec![s("-f"), s("decklink")];
        if !code.trim().is_empty() {
            a.extend([s("-format_code"), code.trim().to_string()]);
        }
        a.extend([s("-i"), name.to_string()]);
        return a;
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
    /// The mode's name, for a device that is asked for one (a DeckLink card's
    /// `Hp60`); empty otherwise.
    pub code: String,
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
        if !self.code.is_empty() {
            text += &format!(" · {}", self.code);
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
            code: String::new(),
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
                    code: String::new(),
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
            code: String::new(),
        });
    }
    tidy(modes)
}

/// The cards FFmpeg's `decklink` input lists for `-list_devices 1`, which
/// prints them quoted, like `[decklink @ 0x1] \t'UltraStudio HD Mini'`, under
/// "Blackmagic DeckLink input devices:".
pub fn parse_decklink_devices(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_input = false;
    for line in text.lines() {
        if line.contains("DeckLink input devices") {
            in_input = true;
            continue;
        }
        if line.contains("DeckLink output devices") {
            in_input = false;
            continue;
        }
        if !in_input {
            continue;
        }
        let Some(start) = line.find('\'') else {
            continue;
        };
        let Some(len) = line[start + 1..].rfind('\'') else {
            continue;
        };
        let name = line[start + 1..start + 1 + len].trim();
        if !name.is_empty() && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
    }
    found
}

/// The modes a DeckLink card offers, from `-list_formats 1`: tab-separated
/// lines such as `\tHp60\t1920 x 1080 at 60/1 fps (progressive)`. The code
/// (`Hp60`) is what `-format_code` takes.
pub fn parse_decklink_formats(text: &str) -> Vec<DeviceMode> {
    let mut modes = Vec::new();
    for line in text.lines() {
        let body = line.rsplit_once("] ").map_or(line, |(_, b)| b);
        let mut cols = body
            .trim_matches(|c: char| c == '\t' || c == ' ')
            .split('\t');
        let (Some(code), Some(description)) = (cols.next(), cols.next()) else {
            continue;
        };
        let code = code.trim();
        if code.is_empty() || code == "format_code" || code.contains(' ') {
            continue;
        }
        // "1920 x 1080 at 60/1 fps (progressive)"
        let mut words = description.split_whitespace();
        let (Some(w), Some("x"), Some(h), Some("at"), Some(rate)) = (
            words.next(),
            words.next(),
            words.next(),
            words.next(),
            words.next(),
        ) else {
            continue;
        };
        let (Ok(width), Ok(height)) = (w.parse::<u32>(), h.parse::<u32>()) else {
            continue;
        };
        let fps =
            rate.split_once('/')
                .map_or(0, |(n, d)| match (n.parse::<f64>(), d.parse::<f64>()) {
                    (Ok(n), Ok(d)) if d > 0.0 => (n / d).round() as u32,
                    _ => 0,
                });
        modes.push(DeviceMode {
            width,
            height,
            fps,
            format_name: String::new(),
            code: code.to_string(),
        });
    }
    tidy(modes)
}

/// Whether this FFmpeg lists `decklink` among its input devices (`-devices`
/// lines like ` D  decklink  Blackmagic DeckLink input`).
pub fn parse_has_decklink(devices_text: &str) -> bool {
    devices_text.lines().any(|l| {
        let mut w = l.split_whitespace();
        let flags = w.next().unwrap_or("");
        flags.contains('D') && w.next() == Some("decklink")
    })
}

/// What to tell someone whose FFmpeg can't send to a Blackmagic device.
pub const DECKLINK_OUTPUT_UNSUPPORTED: &str = "This FFmpeg can't send to Blackmagic (DeckLink) devices. NewQ's own FFmpeg can't, because \
     Blackmagic's SDK terms don't let it be shipped. Use an FFmpeg built with --enable-decklink \
     (and Blackmagic's Desktop Video installed) by putting it in the folder named by NEWQ_FFMPEG_DIR.";

/// Whether a listing from `ffmpeg -devices` has the `decklink` output (a line
/// like ` E decklink  Blackmagic DeckLink output`).
pub fn parse_has_decklink_output(devices_text: &str) -> bool {
    devices_text.lines().any(|l| {
        let mut w = l.split_whitespace();
        let flags = w.next().unwrap_or("");
        flags.contains('E') && w.next() == Some("decklink")
    })
}

/// Whether `ff` can send to a Blackmagic device.
pub fn ffmpeg_has_decklink_output(ff: &Ffmpeg) -> bool {
    let mut cmd = ff.command();
    cmd.args(["-hide_banner", "-devices"]);
    cmd.output()
        .map(|o| parse_has_decklink_output(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

/// The Blackmagic devices that can be sent to, by name, from `ffmpeg -sinks
/// decklink` (lines like `  UltraStudio Express Monitor [UltraStudio Express
/// Monitor]` under "Auto-detected sinks for decklink:") and, for an FFmpeg
/// that lists them there, the "output devices" part of `-list_devices`.
pub fn parse_decklink_outputs(sinks_text: &str, list_text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim().trim_start_matches('*').trim();
        if !name.is_empty() && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
    };
    let mut in_sinks = false;
    for line in sinks_text.lines() {
        if line.contains("Auto-detected sinks") {
            in_sinks = true;
            continue;
        }
        if in_sinks && (line.starts_with(char::is_whitespace) || line.starts_with('*')) {
            let name = line.split_once(" [").map_or(line, |(n, _)| n);
            add(name);
        }
    }
    let mut in_output = false;
    for line in list_text.lines() {
        if line.contains("DeckLink output devices") {
            in_output = true;
            continue;
        }
        if line.contains("DeckLink input devices") {
            in_output = false;
            continue;
        }
        if !in_output {
            continue;
        }
        if let Some(start) = line.find('\'')
            && let Some(len) = line[start + 1..].rfind('\'')
        {
            add(&line[start + 1..start + 1 + len]);
        }
    }
    found
}

/// The Blackmagic devices `ff` can send to (none if its build lacks DeckLink).
pub fn list_decklink_outputs(ff: &Ffmpeg) -> Vec<String> {
    if !ffmpeg_has_decklink_output(ff) {
        return Vec::new();
    }
    let mut sinks = ff.command();
    sinks.args(["-hide_banner", "-sinks", "decklink"]);
    let sinks = sinks
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
        })
        .unwrap_or_default();
    let list = list_output(ff, &["-f", "decklink", "-list_devices", "1", "-i", "dummy"]);
    parse_decklink_outputs(&sinks, &list)
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
    if let Some(name) = decklink_name(device) {
        let text = list_output(ff, &["-f", "decklink", "-list_formats", "1", "-i", name]);
        let modes = parse_decklink_formats(&text);
        return if modes.is_empty() {
            Err("the card didn't list any modes (is a signal connected, and is it in use?)".into())
        } else {
            Ok(modes)
        };
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

/// The format to ask for at `w` x `h` and `fps` from the modes a device lists:
/// one it offers at that size and at least that rate, a raw one before a
/// compressed one. None if it lists no such mode (the request is then left as
/// it is).
pub fn auto_format(modes: &[DeviceMode], w: u32, h: u32, fps: u32) -> Option<CaptureFormat> {
    modes
        .iter()
        .filter(|m| m.width == w && m.height == h && (m.fps == 0 || m.fps >= fps))
        .filter_map(|m| m.format())
        .min_by_key(|f| f.pixel_format().is_none())
}

/// What to ask of `device` when the cue says `Automatic`. Some capture cards
/// (a Magewell USB one, say) refuse to open unless the format is named, so the
/// device is asked what it offers and the matching format is named. Anything
/// else (a format already chosen, a device that isn't a DirectShow or
/// Video4Linux camera, one that won't list its modes) is left as it was. It
/// never waits for the device: until it has answered (see `modes_of`) the
/// cue's own format is used.
pub fn resolve_format(
    ff: &Ffmpeg,
    platform: Platform,
    device: &str,
    w: u32,
    h: u32,
    fps: u32,
    format: CaptureFormat,
) -> CaptureFormat {
    let real = !device.is_empty()
        && device != TEST_PATTERN_DEVICE
        && screen_target(device).is_none()
        && decklink_name(device).is_none();
    if format != CaptureFormat::Auto || !real || platform == Platform::MacOs {
        return format;
    }
    match modes_of(ff, platform, device) {
        Some(modes) => auto_format(&modes, w, h, fps).unwrap_or(format),
        None => format,
    }
}

/// What a device offered when last asked: when, and the modes (None if it
/// wouldn't say, or the asking is still going).
type ModeCache = std::sync::Mutex<
    std::collections::HashMap<String, (std::time::Instant, Option<Vec<DeviceMode>>)>,
>;

/// How long a list of modes is kept, and how long a failure to get one is
/// (a device in use by another program answers later).
const MODES_KEPT: std::time::Duration = std::time::Duration::from_secs(300);
const MODES_FAILED_KEPT: std::time::Duration = std::time::Duration::from_secs(15);

/// The modes `device` offers, as far as is known now. This is called while a
/// cue is prepared, on the main loop, so it never waits: asking a device runs
/// FFmpeg (seconds on some drivers), so the asking is done on a thread of its
/// own and its answer is used from the next time on. The first preload of a
/// camera cue starts it, well before its GO; until then the cue opens as it
/// did before formats were named, and a failed open is tried again (with the
/// answer) a few seconds later.
fn modes_of(ff: &Ffmpeg, platform: Platform, device: &str) -> Option<Vec<DeviceMode>> {
    static CACHE: std::sync::OnceLock<ModeCache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let ff = ff.clone();
    let key = device.to_string();
    modes_known(cache, device, move || list_modes(&ff, platform, &key).ok())
}

/// `modes_of` with the cache and the asking given: what `cache` knows of
/// `device` now, and if that is missing or old, `ask` run on a thread to
/// replace it. An old list is used while it is asked again.
fn modes_known(
    cache: &'static ModeCache,
    device: &str,
    ask: impl FnOnce() -> Option<Vec<DeviceMode>> + Send + 'static,
) -> Option<Vec<DeviceMode>> {
    let known = {
        let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
        let (fresh, known) = match c.get(device) {
            Some((at, modes)) => {
                let kept = if modes.is_some() {
                    MODES_KEPT
                } else {
                    MODES_FAILED_KEPT
                };
                (at.elapsed() < kept, modes.clone())
            }
            None => (false, None),
        };
        if fresh {
            return known;
        }
        // Marked as asked at once, so a slow answer is not asked for again.
        c.insert(
            device.to_string(),
            (std::time::Instant::now(), known.clone()),
        );
        known
    };
    let key = device.to_string();
    let _ = std::thread::Builder::new()
        .name("camera-format".into())
        .spawn(move || {
            let modes = ask();
            let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
            // A device that wouldn't answer this time keeps what it said before.
            let modes = modes.or_else(|| c.get(&key).and_then(|(_, m)| m.clone()));
            c.insert(key, (std::time::Instant::now(), modes));
        });
    known
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
    // Blackmagic cards, if this FFmpeg can read them.
    let mut cmd = ff.command();
    cmd.args(["-hide_banner", "-devices"]);
    let has = cmd
        .output()
        .map(|o| parse_has_decklink(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false);
    DECKLINK_SUPPORTED.store(has, std::sync::atomic::Ordering::Relaxed);
    if has {
        let text = list_output(ff, &["-f", "decklink", "-list_devices", "1", "-i", "dummy"]);
        devices.extend(
            parse_decklink_devices(&text)
                .into_iter()
                .map(|n| format!("{DECKLINK_PREFIX}{n}")),
        );
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

    #[test]
    fn asking_a_device_for_its_modes_never_holds_up_the_caller() {
        use std::time::{Duration, Instant};
        static CACHE: std::sync::OnceLock<ModeCache> = std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let mode = DeviceMode {
            width: 1920,
            height: 1080,
            fps: 30,
            format_name: "yuyv422".into(),
            code: String::new(),
        };
        let answer = vec![mode];
        // A driver that takes two seconds to answer.
        let slow = {
            let answer = answer.clone();
            move || {
                std::thread::sleep(Duration::from_millis(1500));
                Some(answer)
            }
        };
        let t = Instant::now();
        assert_eq!(modes_known(cache, "Slow card", slow), None);
        assert!(
            t.elapsed() < Duration::from_millis(200),
            "{:?}",
            t.elapsed()
        );
        // While it is being asked, it is not asked again (and still nothing waits).
        let t = Instant::now();
        assert_eq!(
            modes_known(cache, "Slow card", || panic!("asked twice")),
            None
        );
        assert!(t.elapsed() < Duration::from_millis(200));
        // Once it has answered, the answer is what the next cue gets.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let got = modes_known(cache, "Slow card", || panic!("asked again"));
            if let Some(got) = got {
                assert_eq!(got, answer);
                break;
            }
            assert!(Instant::now() < deadline, "the answer never arrived");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    use super::*;

    #[test]
    fn a_format_is_asked_for_the_way_each_platform_takes_it() {
        let a = |p, d: &str, f| input_args(p, d, 3840, 2160, 60, f, "").join(" ");
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
                CaptureFormat::Nv12,
                ""
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
                CaptureFormat::Nv12,
                ""
            )
            .join(" ")
            .contains("nv12")
        );
    }

    #[test]
    fn automatic_names_the_format_a_card_offers_at_the_size() {
        // A Magewell USB Capture SDI offers YUY2 only, up to 60 fps.
        let text = "[dshow @ 1]   pixel_format=yuyv422  min s=1920x1080 fps=15 max s=1920x1080 fps=59.9999\n\
                    [dshow @ 1]   pixel_format=yuyv422  min s=1280x720 fps=15 max s=1280x720 fps=59.9999\n";
        let modes = parse_dshow_modes(text);
        assert_eq!(
            auto_format(&modes, 1280, 720, 30),
            Some(CaptureFormat::Yuy2)
        );
        assert_eq!(
            auto_format(&modes, 1920, 1080, 60),
            Some(CaptureFormat::Yuy2)
        );
        // A size or rate it doesn't offer is left alone.
        assert_eq!(auto_format(&modes, 3840, 2160, 30), None);
        assert_eq!(auto_format(&modes, 1920, 1080, 120), None);
        // Raw is preferred to compressed at the same size.
        let both = parse_dshow_modes(
            "[dshow @ 1]   vcodec=mjpeg  min s=1280x720 fps=5 max s=1280x720 fps=60\n\
             [dshow @ 1]   pixel_format=nv12  min s=1280x720 fps=5 max s=1280x720 fps=30\n",
        );
        assert_eq!(auto_format(&both, 1280, 720, 30), Some(CaptureFormat::Nv12));
        assert_eq!(
            auto_format(&both, 1280, 720, 60),
            Some(CaptureFormat::Mjpeg)
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
    fn a_blackmagic_card_is_asked_for_its_mode_by_name_or_left_to_detect() {
        let a = |code: &str| {
            input_args(
                Platform::Windows,
                "decklink:UltraStudio HD Mini",
                1920,
                1080,
                60,
                CaptureFormat::Nv12,
                code,
            )
            .join(" ")
        };
        // The format and the size don't apply: the card's mode is named.
        assert_eq!(
            a("Hp60"),
            "-f decklink -format_code Hp60 -i UltraStudio HD Mini"
        );
        assert_eq!(
            a(" Hp5994 "),
            "-f decklink -format_code Hp5994 -i UltraStudio HD Mini"
        );
        assert_eq!(a(""), "-f decklink -i UltraStudio HD Mini");
        // The same on every system.
        assert_eq!(
            input_args(
                Platform::MacOs,
                "decklink:X",
                640,
                360,
                30,
                CaptureFormat::Auto,
                "pal"
            )
            .join(" "),
            "-f decklink -format_code pal -i X"
        );
        assert_eq!(
            decklink_name("decklink:UltraStudio 4K"),
            Some("UltraStudio 4K")
        );
        assert_eq!(decklink_name("Integrated Camera"), None);
    }

    #[test]
    fn reads_the_blackmagic_devices_that_can_be_sent_to() {
        let sinks = "Auto-detected sinks for decklink:\n  UltraStudio Express Monitor [UltraStudio Express Monitor]\n* UltraStudio Mini Monitor [UltraStudio Mini Monitor]\n";
        let list = "\
[decklink @ 0x1] Blackmagic DeckLink output devices:
[decklink @ 0x1] \t'UltraStudio Mini Monitor'
[decklink @ 0x1] \t'DeckLink Duo (2)'
[decklink @ 0x1] Blackmagic DeckLink input devices:
[decklink @ 0x1] \t'UltraStudio Express Recorder'
";
        assert_eq!(
            parse_decklink_outputs(sinks, list),
            vec![
                "UltraStudio Express Monitor",
                "UltraStudio Mini Monitor",
                "DeckLink Duo (2)"
            ]
        );
        assert!(parse_decklink_outputs("", "").is_empty());
        assert!(parse_has_decklink_output(
            " E decklink         Blackmagic DeckLink output\n"
        ));
        assert!(!parse_has_decklink_output(
            " D  decklink         Blackmagic DeckLink input\n"
        ));
    }

    #[test]
    fn reads_the_decklink_cards_an_ffmpeg_lists() {
        let text = "\
[decklink @ 0x600003c10000] Blackmagic DeckLink output devices:
[decklink @ 0x600003c10000] \t'UltraStudio HD Mini'
[decklink @ 0x600003c10000] Blackmagic DeckLink input devices:
[decklink @ 0x600003c10000] \t'UltraStudio 4K Mini'
[decklink @ 0x600003c10000] \t'DeckLink Duo (1)'
dummy: Immediate exit requested
";
        // Only the input devices count, with their names as quoted.
        assert_eq!(
            parse_decklink_devices(text),
            vec!["UltraStudio 4K Mini", "DeckLink Duo (1)"]
        );
        assert!(parse_decklink_devices("Unknown input format: 'decklink'\n").is_empty());
    }

    #[test]
    fn reads_the_modes_a_decklink_card_lists() {
        let text = "\
[decklink @ 0x6000] Supported formats for 'UltraStudio 4K Mini':
\tformat_code\tdescription
\tntsc\t720 x 486 at 30000/1001 fps (interlaced, lower field first)
\tHp60\t1920 x 1080 at 60/1 fps (progressive)
\tHp5994\t1920 x 1080 at 60000/1001 fps (progressive)
\t4k30\t3840 x 2160 at 30/1 fps (progressive)
\t4k60\t3840 x 2160 at 60/1 fps (progressive)
dummy: Immediate exit requested
";
        let modes = parse_decklink_formats(text);
        let got: Vec<(u32, u32, u32, &str)> = modes
            .iter()
            .map(|m| (m.width, m.height, m.fps, m.code.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (3840, 2160, 60, "4k60"),
                (3840, 2160, 30, "4k30"),
                (1920, 1080, 60, "Hp60"),
                (1920, 1080, 60, "Hp5994"),
                (720, 486, 30, "ntsc"),
            ]
        );
        assert_eq!(modes[2].label(), "1920 × 1080 · up to 60 fps · Hp60");
        assert!(parse_decklink_formats("Could not enumerate formats\n").is_empty());
    }

    #[test]
    fn an_ffmpeg_that_can_read_decklink_says_so_in_its_device_list() {
        let with = " D  decklink        Blackmagic DeckLink input\n  E decklink        Blackmagic DeckLink output\n D  video4linux2,v4l2 Video4Linux2 device grab\n";
        let without = " D  video4linux2,v4l2 Video4Linux2 device grab\n DE x11grab       X11 screen capture\n";
        assert!(parse_has_decklink(with));
        assert!(!parse_has_decklink(without));
        // An output-only listing doesn't count.
        assert!(!parse_has_decklink(
            "  E decklink        Blackmagic DeckLink output\n"
        ));
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
        let a = |p, d: &str| input_args(p, d, 1280, 720, 30, CaptureFormat::Auto, "").join(" ");
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
        assert!(input_args(Platform::Linux, "", 1280, 720, 30, CaptureFormat::Auto, "").is_empty());
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
            "",
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
