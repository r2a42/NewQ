//! Vidvox HAP: video whose frames are GPU texture data (DXT/BC blocks),
//! so a frame is a decompress and an upload, with no colour conversion.
//!
//! FFmpeg only demuxes (`-c copy`): the frames arrive on a pipe one after
//! another, each a section with its own length, which is read here, expanded
//! (HAP's second stage is Snappy, optionally in chunks) and given to the
//! graphics card as the compressed texture it is.

use anyhow::{Result, anyhow, bail, ensure};
use std::io::Read;

/// What the texture in a frame is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapFormat {
    /// HAP: BC1 (DXT1), colour only.
    Dxt1,
    /// HAP Alpha: BC3 (DXT5), colour and alpha.
    Dxt5,
    /// HAP Q: BC3 holding scaled YCoCg, turned into colour by the shader.
    Dxt5YCoCg,
}

impl HapFormat {
    /// Bytes of one 4x4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            HapFormat::Dxt1 => 8,
            HapFormat::Dxt5 | HapFormat::Dxt5YCoCg => 16,
        }
    }

    /// Bytes of a texture `w` by `h` (both a multiple of 4).
    pub fn texture_len(self, w: u32, h: u32) -> usize {
        (w as usize / 4) * (h as usize / 4) * self.block_bytes()
    }

    fn from_nibble(n: u8) -> Option<HapFormat> {
        match n {
            0x0B => Some(HapFormat::Dxt1),
            0x0E => Some(HapFormat::Dxt5),
            0x0F => Some(HapFormat::Dxt5YCoCg),
            _ => None,
        }
    }
}

/// The largest frame section taken as sane (a 16K by 16K Hap Q frame is
/// 256 MB of texture; a damaged length must not ask for more than this).
const MAX_SECTION: usize = 512 << 20;

/// A section header: how long its header is, how long its data, and its type
/// byte. Three bytes of length, little-endian, and the type; a length of 0
/// means four more bytes of length follow.
fn section_header(bytes: &[u8]) -> Option<(usize, usize, u8)> {
    let b = bytes.get(..4)?;
    let small = usize::from(b[0]) | usize::from(b[1]) << 8 | usize::from(b[2]) << 16;
    if small != 0 {
        return Some((4, small, b[3]));
    }
    let e = bytes.get(4..8)?;
    let big = u32::from_le_bytes([e[0], e[1], e[2], e[3]]) as usize;
    Some((8, big, b[3]))
}

/// Read one frame (its whole top-level section, header included) from a
/// stream of them. Ok(None) at a clean end of the stream.
pub fn read_frame(r: &mut impl Read, buf: &mut Vec<u8>) -> Result<Option<()>> {
    buf.clear();
    buf.resize(4, 0);
    let mut got = 0;
    while got < 4 {
        match r.read(&mut buf[got..4])? {
            0 if got == 0 => return Ok(None),
            0 => bail!("the stream ended inside a frame"),
            n => got += n,
        }
    }
    let extended = buf[..3] == [0, 0, 0];
    if extended {
        buf.resize(8, 0);
        r.read_exact(&mut buf[4..8])?;
    }
    let (head, len, _) = section_header(buf).ok_or_else(|| anyhow!("no section header"))?;
    ensure!(len <= MAX_SECTION, "a frame of {len} bytes is not sane");
    buf.resize(head + len, 0);
    r.read_exact(&mut buf[head..])?;
    Ok(Some(()))
}

/// The texture format a frame holds, without decoding it.
pub fn frame_format(frame: &[u8]) -> Result<HapFormat> {
    let (_, _, ty) = section_header(frame).ok_or_else(|| anyhow!("no section header"))?;
    HapFormat::from_nibble(ty & 0x0F).ok_or_else(|| anyhow!("unsupported Hap format {ty:#04x}"))
}

/// How many bytes of texture a frame of `w` by `h` has, from the format its
/// header names.
pub fn texture_len_of(frame: &[u8], w: u32, h: u32) -> usize {
    frame_format(frame).map_or(0, |f| f.texture_len(w, h))
}

/// Expand a frame into the texture data of `expected` bytes.
pub fn decode_frame(frame: &[u8], expected: usize, out: &mut Vec<u8>) -> Result<HapFormat> {
    let (head, len, ty) = section_header(frame).ok_or_else(|| anyhow!("no section header"))?;
    let data = frame
        .get(head..head + len)
        .ok_or_else(|| anyhow!("a frame shorter than its length"))?;
    let format = HapFormat::from_nibble(ty & 0x0F)
        .ok_or_else(|| anyhow!("unsupported Hap format {ty:#04x}"))?;
    out.clear();
    match ty & 0xF0 {
        // Not compressed.
        0xA0 => out.extend_from_slice(data),
        // Snappy.
        0xB0 => snappy(data, out)?,
        // In chunks, each with its own second stage.
        0xC0 => chunks(data, expected, out)?,
        other => bail!("unsupported Hap second stage {other:#04x}"),
    }
    ensure!(
        out.len() == expected,
        "a Hap frame of {} bytes, not the {expected} the picture needs",
        out.len()
    );
    Ok(format)
}

fn snappy(data: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let n = snap::raw::decompress_len(data).map_err(|e| anyhow!("snappy: {e}"))?;
    ensure!(n <= MAX_SECTION, "a snappy block of {n} bytes is not sane");
    let at = out.len();
    out.resize(at + n, 0);
    snap::raw::Decoder::new()
        .decompress(data, &mut out[at..])
        .map_err(|e| anyhow!("snappy: {e}"))?;
    Ok(())
}

/// A frame in chunks: a section of decode instructions (which second stage
/// each chunk has, and how long each is), then the chunks.
fn chunks(data: &[u8], expected: usize, out: &mut Vec<u8>) -> Result<()> {
    let (head, len, ty) = section_header(data).ok_or_else(|| anyhow!("no instructions"))?;
    ensure!(
        ty == 0x01,
        "a chunked frame without its decode instructions"
    );
    let instructions = data
        .get(head..head + len)
        .ok_or_else(|| anyhow!("instructions longer than the frame"))?;
    let mut compressors: &[u8] = &[];
    let mut sizes: &[u8] = &[];
    let mut at = 0;
    while at < instructions.len() {
        let (h, l, t) =
            section_header(&instructions[at..]).ok_or_else(|| anyhow!("a damaged instruction"))?;
        let body = instructions
            .get(at + h..at + h + l)
            .ok_or_else(|| anyhow!("an instruction longer than its section"))?;
        match t {
            0x02 => compressors = body,
            0x03 => sizes = body,
            // 0x04 says where each chunk's output goes; they go in order.
            _ => {}
        }
        at += h + l;
    }
    ensure!(
        !compressors.is_empty() && sizes.len() == compressors.len() * 4,
        "chunk instructions that don't agree"
    );
    let mut cursor = head + len;
    for (i, c) in compressors.iter().enumerate() {
        let s = &sizes[i * 4..i * 4 + 4];
        let size = u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize;
        let chunk = data
            .get(cursor..cursor + size)
            .ok_or_else(|| anyhow!("a chunk longer than the frame"))?;
        match c {
            0x0A => out.extend_from_slice(chunk),
            0x0B => snappy(chunk, out)?,
            other => bail!("unsupported chunk compressor {other:#04x}"),
        }
        ensure!(out.len() <= expected, "chunks longer than the picture");
        cursor += size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_headers_have_three_or_seven_bytes_of_length() {
        assert_eq!(section_header(&[0x10, 0, 0, 0xBB]), Some((4, 16, 0xBB)));
        assert_eq!(
            section_header(&[0x00, 0x01, 0x02, 0xAE]),
            Some((4, 0x020100, 0xAE))
        );
        // A zero three-byte length means the real one follows.
        assert_eq!(
            section_header(&[0, 0, 0, 0xBB, 0xCC, 0x47, 0, 0]),
            Some((8, 0x47CC, 0xBB))
        );
        assert_eq!(section_header(&[1, 2]), None);
        assert_eq!(section_header(&[0, 0, 0, 0xBB, 1]), None);
    }

    #[test]
    fn frames_are_read_one_after_another_and_a_cut_stream_is_an_error() {
        // Two uncompressed frames (type 0xAB), the second with a long header.
        let mut stream = vec![3, 0, 0, 0xAB, 1, 2, 3];
        stream.extend([0, 0, 0, 0xAB, 2, 0, 0, 0, 9, 9]);
        let mut r = &stream[..];
        let mut buf = Vec::new();
        assert!(read_frame(&mut r, &mut buf).unwrap().is_some());
        assert_eq!(buf, [3, 0, 0, 0xAB, 1, 2, 3]);
        assert!(read_frame(&mut r, &mut buf).unwrap().is_some());
        assert_eq!(&buf[8..], [9, 9]);
        assert!(
            read_frame(&mut r, &mut buf).unwrap().is_none(),
            "a clean end"
        );
        let mut cut = &stream[..5];
        assert!(read_frame(&mut cut, &mut buf).is_err());
        // An absurd length is refused, not allocated.
        let mut huge = &[0, 0, 0, 0xAB, 0xFF, 0xFF, 0xFF, 0xFF][..];
        assert!(read_frame(&mut huge, &mut buf).is_err());
    }

    #[test]
    fn frames_say_their_format_and_decode_or_say_why_not() {
        let mut out = Vec::new();
        let raw = [4, 0, 0, 0xAB, 1, 2, 3, 4];
        assert_eq!(frame_format(&raw).unwrap(), HapFormat::Dxt1);
        assert_eq!(decode_frame(&raw, 4, &mut out).unwrap(), HapFormat::Dxt1);
        assert_eq!(out, [1, 2, 3, 4]);
        // The wrong length for the picture.
        assert!(decode_frame(&raw, 8, &mut out).is_err());
        // Snappy: a literal of four bytes.
        let snappy_frame = [6, 0, 0, 0xBE, 4, 0x0C, 5, 6, 7, 8];
        assert_eq!(
            decode_frame(&snappy_frame, 4, &mut out).unwrap(),
            HapFormat::Dxt5
        );
        assert_eq!(out, [5, 6, 7, 8]);
        // Formats and second stages that aren't known are errors.
        assert!(frame_format(&[1, 0, 0, 0xAC, 0]).is_err());
        assert!(decode_frame(&[1, 0, 0, 0xDB, 0], 1, &mut out).is_err());
        // Damaged: shorter than its length, or a bad snappy block.
        assert!(decode_frame(&[9, 0, 0, 0xAB, 1], 9, &mut out).is_err());
        assert!(decode_frame(&[3, 0, 0, 0xBB, 0xFF, 0xFF, 0xFF], 4, &mut out).is_err());
    }

    #[test]
    fn a_chunked_frame_is_put_together_from_its_chunks() {
        // Two chunks: the first not compressed, the second snappy.
        let chunk1 = [1u8, 2];
        let chunk2 = [4u8, 0x0C, 3, 4, 5, 6]; // snappy for [3,4,5,6]
        let compressors = [0x0A, 0x0B];
        let mut sizes = Vec::new();
        sizes.extend(2u32.to_le_bytes());
        sizes.extend(6u32.to_le_bytes());
        let mut instr = vec![2, 0, 0, 0x02];
        instr.extend(compressors);
        instr.extend([8, 0, 0, 0x03]);
        instr.extend(&sizes);
        let mut data = vec![instr.len() as u8, 0, 0, 0x01];
        data.extend(&instr);
        data.extend(chunk1);
        data.extend(chunk2);
        let mut frame = vec![data.len() as u8, 0, 0, 0xCB];
        frame.extend(&data);
        let mut out = Vec::new();
        decode_frame(&frame, 6, &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4, 5, 6]);
        // Instructions that don't agree are an error.
        let mut bad = frame.clone();
        let i = bad.iter().position(|&b| b == 0x0A).unwrap();
        bad[i] = 0x77;
        assert!(decode_frame(&bad, 6, &mut out).is_err());
    }

    /// A BC1 colour block's 16 pixels, as FFmpeg's decoder makes them.
    fn bc1(block: &[u8]) -> [[u8; 3]; 16] {
        let c = |i: usize| u16::from_le_bytes([block[i], block[i + 1]]);
        let (c0, c1) = (c(0), c(2));
        let rgb = |c: u16| {
            let (r, g, b) = (
                u32::from(c >> 11),
                u32::from((c >> 5) & 63),
                u32::from(c & 31),
            );
            [
                ((r * 255 + 15) / 31) as u8,
                ((g * 255 + 31) / 63) as u8,
                ((b * 255 + 15) / 31) as u8,
            ]
        };
        let (p0, p1) = (rgb(c0), rgb(c1));
        let mix = |a: u8, b: u8, wa: u32, wb: u32, d: u32| {
            ((u32::from(a) * wa + u32::from(b) * wb) / d) as u8
        };
        let palette: [[u8; 3]; 4] = if c0 > c1 {
            [
                p0,
                p1,
                [
                    mix(p0[0], p1[0], 2, 1, 3),
                    mix(p0[1], p1[1], 2, 1, 3),
                    mix(p0[2], p1[2], 2, 1, 3),
                ],
                [
                    mix(p0[0], p1[0], 1, 2, 3),
                    mix(p0[1], p1[1], 1, 2, 3),
                    mix(p0[2], p1[2], 1, 2, 3),
                ],
            ]
        } else {
            [
                p0,
                p1,
                [
                    mix(p0[0], p1[0], 1, 1, 2),
                    mix(p0[1], p1[1], 1, 1, 2),
                    mix(p0[2], p1[2], 1, 1, 2),
                ],
                [0, 0, 0],
            ]
        };
        let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        std::array::from_fn(|i| palette[((bits >> (2 * i)) & 3) as usize])
    }

    /// A BC3 alpha block's 16 values.
    fn bc3_alpha(block: &[u8]) -> [u8; 16] {
        let (a0, a1) = (u32::from(block[0]), u32::from(block[1]));
        let pal: [u32; 8] = if a0 > a1 {
            std::array::from_fn(|i| match i {
                0 => a0,
                1 => a1,
                n => ((8 - n as u32) * a0 + (n as u32 - 1) * a1) / 7,
            })
        } else {
            std::array::from_fn(|i| match i {
                0 => a0,
                1 => a1,
                6 => 0,
                7 => 255,
                n => ((6 - n as u32) * a0 + (n as u32 - 1) * a1) / 5,
            })
        };
        let mut bits = 0u64;
        for (i, b) in block[2..8].iter().enumerate() {
            bits |= u64::from(*b) << (8 * i);
        }
        std::array::from_fn(|i| pal[((bits >> (3 * i)) & 7) as usize] as u8)
    }

    /// The whole picture as RGBA, from texture data.
    fn to_rgba(format: HapFormat, tex: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        for by in 0..h / 4 {
            for bx in 0..w / 4 {
                let at = (by * (w / 4) + bx) * format.block_bytes();
                let block = &tex[at..at + format.block_bytes()];
                let (alpha, colour) = match format {
                    HapFormat::Dxt1 => (None, bc1(block)),
                    _ => (Some(bc3_alpha(&block[..8])), bc1(&block[8..])),
                };
                for k in 0..16 {
                    let (x, y) = (bx * 4 + k % 4, by * 4 + k / 4);
                    let mut px = colour[k];
                    if format == HapFormat::Dxt5YCoCg {
                        // Scaled YCoCg: Y in alpha, Co and Cg in red and green, the scale in blue.
                        let s = 1.0 / (f32::from(px[2]) / 255.0 * (255.0 / 8.0) + 1.0);
                        let co = (f32::from(px[0]) / 255.0 - 128.0 / 255.0) * s;
                        let cg = (f32::from(px[1]) / 255.0 - 128.0 / 255.0) * s;
                        let yy = f32::from(alpha.unwrap()[k]) / 255.0;
                        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                        px = [c(yy + co - cg), c(yy + cg), c(yy - co - cg)];
                    }
                    let o = (y * w + x) * 4;
                    out[o..o + 3].copy_from_slice(&px);
                    out[o + 3] = match (format, alpha) {
                        (HapFormat::Dxt5, Some(a)) => a[k],
                        _ => 255,
                    };
                }
            }
        }
        out
    }

    #[test]
    fn frames_from_ffmpeg_decode_to_the_picture_ffmpeg_makes() {
        let Some((ff, src)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let dir = std::env::temp_dir().join("newq_test_hap");
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (640usize, 360usize);
        // Each format, and chunked and not compressed (the other second stages).
        for (name, format, extra) in [
            ("hap", HapFormat::Dxt1, ["-chunks", "1"]),
            ("hap_alpha", HapFormat::Dxt5, ["-chunks", "1"]),
            ("hap_q", HapFormat::Dxt5YCoCg, ["-chunks", "1"]),
            ("hap", HapFormat::Dxt1, ["-chunks", "4"]),
            ("hap_q", HapFormat::Dxt5YCoCg, ["-compressor", "none"]),
        ] {
            let clip = dir.join(format!("{name}_{}.mov", extra.join("")));
            let made = ff
                .command()
                .args(["-y", "-i"])
                .arg(&src)
                .args([
                    "-t",
                    "1",
                    "-vf",
                    "scale=640:360",
                    "-c:v",
                    "hap",
                    "-format",
                    name,
                    "-an",
                ])
                .arg(&clip)
                .status()
                .unwrap()
                .success();
            assert!(made, "ffmpeg could not make {name}");
            // The frames as they are in the file.
            let raw = ff
                .command()
                .args(["-i"])
                .arg(&clip)
                .args(["-c", "copy", "-f", "rawvideo", "-"])
                .output()
                .unwrap()
                .stdout;
            let mut r = &raw[..];
            let (mut buf, mut tex) = (Vec::new(), Vec::new());
            let mut frames = 0;
            let mut first = Vec::new();
            while read_frame(&mut r, &mut buf).unwrap().is_some() {
                assert_eq!(frame_format(&buf).unwrap(), format, "{name}");
                decode_frame(&buf, format.texture_len(w as u32, h as u32), &mut tex).unwrap();
                if frames == 0 {
                    first = tex.clone();
                }
                frames += 1;
            }
            assert!((29..=31).contains(&frames), "{name}: {frames} frames");
            // FFmpeg's own decode of the first frame.
            let theirs = ff
                .command()
                .args(["-i"])
                .arg(&clip)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
                .output()
                .unwrap()
                .stdout;
            let mine = to_rgba(format, &first, w, h);
            assert_eq!(theirs.len(), mine.len());
            let (mut worst, mut sum) = (0i32, 0u64);
            for (a, b) in theirs.iter().zip(&mine) {
                let d = (i32::from(*a) - i32::from(*b)).abs();
                worst = worst.max(d);
                sum += d as u64;
            }
            let mean = sum as f64 / theirs.len() as f64;
            // The same texture, decoded two ways: they agree but for rounding.
            assert!(
                mean < 1.0 && worst <= 24,
                "{name}: mean {mean}, worst {worst}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_sizes_of_textures() {
        assert_eq!(HapFormat::Dxt1.texture_len(8, 8), 32);
        assert_eq!(HapFormat::Dxt5.texture_len(640, 360), 160 * 90 * 16);
    }
}
