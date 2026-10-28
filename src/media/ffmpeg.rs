//! Locating the ffmpeg/ffprobe executables and probing media files.
//!
//! NewQ drives FFmpeg as a subprocess: it decodes media to raw frames/samples on
//! stdout, and NewQ does all compositing, fading and mixing itself. This keeps the
//! build free of native FFmpeg dev libraries and works the same on every platform.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

impl Ffmpeg {
    /// Search order: `$NEWQ_FFMPEG_DIR`, next to the NewQ executable (and its
    /// `ffmpeg/` or `ffmpeg/bin/` subfolder), `third_party/ffmpeg/bin` in the
    /// working directory (development), then `PATH`.
    pub fn locate() -> Option<Ffmpeg> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(d) = std::env::var("NEWQ_FFMPEG_DIR") {
            dirs.push(PathBuf::from(d));
        }
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
        {
            dirs.push(exe_dir.join("ffmpeg").join("bin"));
            dirs.push(exe_dir.join("ffmpeg"));
            dirs.push(exe_dir.clone());
            // target/debug -> project root during development.
            if let Some(root) = exe_dir.parent().and_then(Path::parent) {
                dirs.push(root.join("third_party").join("ffmpeg").join("bin"));
            }
        }
        dirs.push(PathBuf::from("third_party").join("ffmpeg").join("bin"));
        if cfg!(target_os = "macos") {
            dirs.push("/opt/homebrew/bin".into());
            dirs.push("/usr/local/bin".into());
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.into_iter().find_map(|d| {
            let ffmpeg = d.join(exe("ffmpeg"));
            let ffprobe = d.join(exe("ffprobe"));
            (ffmpeg.is_file() && ffprobe.is_file()).then_some(Ffmpeg { ffmpeg, ffprobe })
        })
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_console(&mut cmd);
        cmd
    }

    /// Write a short clip for lining sound up with picture: once a second the
    /// screen flashes white and a 1 kHz beep sounds, for a tenth of a second.
    pub fn make_sync_clip(&self, path: &Path) -> Result<()> {
        let mut cmd = self.command();
        cmd.args(["-y", "-f", "lavfi", "-i", "color=c=black:s=1280x720:r=30"]);
        cmd.args(["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000"]);
        cmd.args(["-t", "4"]);
        cmd.args([
            "-vf",
            "drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='lt(mod(t,1),0.1)'",
            "-af",
            "volume='if(lt(mod(t,1),0.1),0.5,0)':eval=frame",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ]);
        cmd.arg(path);
        let out = cmd.output().context("running ffmpeg")?;
        anyhow::ensure!(
            out.status.success(),
            "ffmpeg: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }

    pub fn probe(&self, path: &Path) -> Result<MediaInfo> {
        let mut cmd = Command::new(&self.ffprobe);
        cmd.args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,format_name:stream=index,codec_type,codec_name,width,height,\
             avg_frame_rate,r_frame_rate,color_space,color_range,sample_aspect_ratio,channels,duration:\
             stream_disposition=attached_pic:stream_side_data=rotation",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null());
        no_console(&mut cmd);
        let out = cmd.output().context("running ffprobe")?;
        if !out.status.success() {
            return Err(anyhow!(
                "ffprobe failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        parse_probe(&out.stdout)
    }
}

#[cfg(windows)]
pub fn no_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn no_console(_cmd: &mut Command) {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

#[derive(Clone, Debug)]
pub struct VideoInfo {
    /// Display size (sample aspect ratio and rotation applied).
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub matrix: ColorMatrix,
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub duration: Option<f64>,
    pub video: Option<VideoInfo>,
    pub has_audio: bool,
    /// How many channels the first audio stream has (0 if none).
    pub audio_channels: u16,
    /// A single still image (png, jpg, ...).
    pub is_still: bool,
}

#[derive(Deserialize)]
struct ProbeOut {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    format_name: Option<String>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    color_space: Option<String>,
    sample_aspect_ratio: Option<String>,
    duration: Option<String>,
    channels: Option<u16>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    #[serde(default)]
    attached_pic: i32,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<f64>,
}

fn parse_ratio(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').or_else(|| s.split_once(':'))?;
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok()?;
    (n > 0.0 && d > 0.0).then_some(n / d)
}

fn parse_probe(json: &[u8]) -> Result<MediaInfo> {
    let p: ProbeOut = serde_json::from_slice(json).context("parsing ffprobe output")?;
    let format_name = p
        .format
        .as_ref()
        .and_then(|f| f.format_name.clone())
        .unwrap_or_default();
    let is_still = format_name.ends_with("_pipe") || format_name == "image2";
    let mut duration = p
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok());
    let mut video = None;
    let mut has_audio = false;
    let mut audio_channels = 0;
    for s in &p.streams {
        match s.codec_type.as_deref() {
            Some("audio") => {
                if !has_audio {
                    audio_channels = s.channels.unwrap_or(2);
                }
                has_audio = true;
            }
            Some("video") if video.is_none() => {
                if s.disposition.as_ref().is_some_and(|d| d.attached_pic != 0) {
                    continue;
                }
                let (Some(w), Some(h)) = (s.width, s.height) else {
                    continue;
                };
                let sar = s
                    .sample_aspect_ratio
                    .as_deref()
                    .and_then(parse_ratio)
                    .unwrap_or(1.0);
                let mut dw = ((w as f64) * sar).round().max(2.0) as u32;
                let mut dh = h.max(2);
                let rotation = s
                    .side_data_list
                    .iter()
                    .find_map(|sd| sd.rotation)
                    .unwrap_or(0.0);
                if (rotation.abs() - 90.0).abs() < 1.0 || (rotation.abs() - 270.0).abs() < 1.0 {
                    std::mem::swap(&mut dw, &mut dh);
                }
                let fps = s
                    .avg_frame_rate
                    .as_deref()
                    .and_then(parse_ratio)
                    .filter(|f| *f > 1.0 && *f < 300.0)
                    .or_else(|| {
                        s.r_frame_rate
                            .as_deref()
                            .and_then(parse_ratio)
                            .filter(|f| *f > 1.0 && *f < 300.0)
                    })
                    .unwrap_or(30.0);
                let matrix = match s.color_space.as_deref() {
                    Some("bt709") => ColorMatrix::Bt709,
                    Some("bt2020nc") | Some("bt2020c") => ColorMatrix::Bt2020,
                    Some("bt470bg") | Some("smpte170m") | Some("fcc") => ColorMatrix::Bt601,
                    _ if h >= 720 => ColorMatrix::Bt709,
                    _ => ColorMatrix::Bt601,
                };
                if duration.is_none() {
                    duration = s.duration.as_deref().and_then(|d| d.parse().ok());
                }
                video = Some(VideoInfo {
                    width: dw,
                    height: dh,
                    fps,
                    matrix,
                });
            }
            _ => {}
        }
    }
    if video.is_none() && !has_audio {
        return Err(anyhow!("no playable audio or video stream"));
    }
    Ok(MediaInfo {
        duration: if is_still { None } else { duration },
        video,
        has_audio,
        audio_channels,
        is_still,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sync_clip_has_picture_and_sound() {
        let Some((ff, _)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        let path = std::env::temp_dir().join("newq_test_sync_clip.mp4");
        ff.make_sync_clip(&path).unwrap();
        let info = ff.probe(&path).unwrap();
        assert!(info.video.is_some() && info.has_audio);
        assert!((info.duration.unwrap_or(0.0) - 4.0).abs() < 0.3);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parses_video_with_audio() {
        let json = br#"{"streams":[
            {"index":0,"codec_type":"video","width":1440,"height":1080,"avg_frame_rate":"30000/1001",
             "r_frame_rate":"30000/1001","color_space":"bt709","sample_aspect_ratio":"4:3",
             "disposition":{"attached_pic":0}},
            {"index":1,"codec_type":"audio","disposition":{"attached_pic":0}}],
            "format":{"duration":"12.5","format_name":"mov,mp4,m4a,3gp,3g2,mj2"}}"#;
        let info = parse_probe(json).unwrap();
        let v = info.video.unwrap();
        assert_eq!((v.width, v.height), (1920, 1080));
        assert!((v.fps - 29.97).abs() < 0.01);
        assert_eq!(v.matrix, ColorMatrix::Bt709);
        assert!(info.has_audio);
        assert_eq!(info.duration, Some(12.5));
        assert!(!info.is_still);
    }

    #[test]
    fn parses_rotated_phone_video() {
        let json = br#"{"streams":[{"codec_type":"video","width":1920,"height":1080,
            "avg_frame_rate":"30/1","side_data_list":[{"rotation":-90}]}],
            "format":{"duration":"3.0","format_name":"mov,mp4"}}"#;
        let v = parse_probe(json).unwrap().video.unwrap();
        assert_eq!((v.width, v.height), (1080, 1920));
    }

    #[test]
    fn mp3_cover_art_is_not_video() {
        let json = br#"{"streams":[{"codec_type":"audio"},
            {"codec_type":"video","width":500,"height":500,"disposition":{"attached_pic":1}}],
            "format":{"duration":"200.0","format_name":"mp3"}}"#;
        let info = parse_probe(json).unwrap();
        assert!(info.video.is_none());
        assert!(info.has_audio);
    }

    #[test]
    fn png_is_still() {
        let json = br#"{"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"0/0"}],
            "format":{"format_name":"png_pipe"}}"#;
        let info = parse_probe(json).unwrap();
        assert!(info.is_still);
        assert_eq!(info.duration, None);
    }
}
