//! Converting a video to Hap, the format a graphics card plays with the
//! least work (see `hap`). FFmpeg does it, to a QuickTime file beside the
//! original, with the picture's size made a whole number of blocks.

use super::ffmpeg::Ffmpeg;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// The flavours of Hap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapKind {
    /// Hap: the smallest and fastest, no transparency.
    Hap,
    /// Hap Alpha: with transparency.
    Alpha,
    /// Hap Q: more colour detail, twice the data.
    Q,
}

impl HapKind {
    pub const ALL: [HapKind; 3] = [HapKind::Hap, HapKind::Alpha, HapKind::Q];

    pub fn label(self) -> &'static str {
        match self {
            HapKind::Hap => "Hap (smallest, fastest)",
            HapKind::Alpha => "Hap Alpha (with transparency)",
            HapKind::Q => "Hap Q (best colour)",
        }
    }

    fn format_name(self) -> &'static str {
        match self {
            HapKind::Hap => "hap",
            HapKind::Alpha => "hap_alpha",
            HapKind::Q => "hap_q",
        }
    }
}

/// Where the Hap copy of `input` goes: beside it, `name_hap.mov`, or with a
/// number if that is taken.
pub fn output_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "video".to_string(), |s| s.to_string_lossy().into_owned());
    let dir = input.parent().unwrap_or(Path::new("."));
    let mut path = dir.join(format!("{stem}_hap.mov"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}_hap{n}.mov"));
        n += 1;
    }
    path
}

/// The FFmpeg options (after `-i <input>`) that make Hap of `kind`: sizes in
/// whole blocks and square pixels, the sound as it is in a file this format
/// holds (uncompressed).
pub fn args(kind: HapKind) -> Vec<String> {
    let s = |v: &str| v.to_string();
    vec![
        s("-map"),
        s("0:v:0"),
        s("-map"),
        s("0:a:0?"),
        s("-vf"),
        // The shown shape (sample aspect applied) to a multiple of 4 each way.
        s("scale='trunc(iw*sar/4)*4':'trunc(ih/4)*4',setsar=1"),
        s("-c:v"),
        s("hap"),
        s("-format"),
        s(kind.format_name()),
        s("-c:a"),
        s("pcm_s16le"),
    ]
}

/// Convert `input` to Hap in a new file, which is its path. Blocks until done.
pub fn to_hap(ff: &Ffmpeg, input: &Path, kind: HapKind) -> Result<PathBuf> {
    let output = output_path(input);
    let result = ff
        .command()
        .arg("-y")
        .arg("-i")
        .arg(input)
        .args(args(kind))
        .arg(&output)
        .output()?;
    if !result.status.success() {
        let _ = std::fs::remove_file(&output);
        bail!(
            "FFmpeg could not convert it: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_goes_beside_the_original_and_never_over_a_file() {
        let dir = std::env::temp_dir().join("newq_test_convert_path");
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("scene one.mp4");
        assert_eq!(output_path(&input), dir.join("scene one_hap.mov"));
        std::fs::write(dir.join("scene one_hap.mov"), b"x").unwrap();
        assert_eq!(output_path(&input), dir.join("scene one_hap2.mov"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_kind_asks_for_its_format() {
        for (k, name) in [
            (HapKind::Hap, "hap"),
            (HapKind::Alpha, "hap_alpha"),
            (HapKind::Q, "hap_q"),
        ] {
            let a = args(k);
            let at = a.iter().position(|x| x == "-format").unwrap();
            assert_eq!(a[at + 1], name);
        }
    }

    #[test]
    fn a_converted_file_is_hap_in_whole_blocks_and_plays_as_hap() {
        let Some((ff, src)) = crate::media::test_media_with_hap("smpte_8s.mp4") else {
            return;
        };
        let dir = std::env::temp_dir().join("newq_test_convert");
        std::fs::create_dir_all(&dir).unwrap();
        // A copy to convert, with an odd size and a sound.
        let input = dir.join("odd.mp4");
        let made = ff
            .command()
            .args(["-y", "-i"])
            .arg(&src)
            .args(["-t", "1", "-vf", "scale=322:182", "-c:v", "libx264"])
            .arg(&input)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !made {
            return; // no H.264 encoder in this FFmpeg
        }
        let out = to_hap(&ff, &input, HapKind::Hap).unwrap();
        let info = ff.probe(&out).unwrap();
        let v = info.video.unwrap();
        assert_eq!(v.codec, "hap");
        assert_eq!(v.coded, (320, 180), "a whole number of blocks");
        assert_eq!((v.width, v.height), v.coded, "and square pixels");
        crate::media::video::set_bc_supported(true);
        assert!(crate::media::video::plays_as_hap(&v));
        crate::media::video::set_bc_supported(false);
        // A file that isn't one is an error, and leaves nothing behind.
        let bad = dir.join("notvideo.mp4");
        std::fs::write(&bad, b"nothing").unwrap();
        assert!(to_hap(&ff, &bad, HapKind::Hap).is_err());
        assert!(!output_path(&bad).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
