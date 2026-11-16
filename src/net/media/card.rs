//! The card a display shows in a cue's place while its file isn't on this
//! computer (ruling R13): a plain dark card with what it is waiting for.
//! Drawn on the processor with the text cues' renderer, at the display's size.

use super::words::Card;
use crate::media::ffmpeg::{ColorMatrix, MediaInfo, VideoInfo};
use crate::media::video::RgbaImage;
use crate::net::store::manifest::Facts;
use crate::show::{TextAlign, TextCue, TextVAlign};

/// The card's background.
pub const BACKGROUND: [u8; 4] = [22, 24, 30, 255];

/// Draw `card` at `size`; `black` draws plain black instead (the show's
/// "no transfer cards" setting: an audience sees nothing).
pub fn draw(card: &Card, size: (u32, u32), black: bool) -> RgbaImage {
    let (w, h) = (size.0.max(2), size.1.max(2));
    let bg = if black { [0, 0, 0, 255] } else { BACKGROUND };
    let mut img = RgbaImage {
        width: w,
        height: h,
        data: bg.repeat((w * h) as usize),
    };
    if black {
        return img;
    }
    let line = |text: &str, size: f32, valign: TextVAlign, color: [u8; 4], bold: bool| {
        let mut t = TextCue::new(None);
        t.text = text.to_string();
        t.size = size;
        t.color = color;
        t.align = TextAlign::Center;
        t.valign = valign;
        t.width = 0.9;
        t.shadow = false;
        t.bold = bold;
        crate::text::render(&t, (w, h))
    };
    // Three lines, each drawn in the middle and moved up or down.
    let hf = h as f32;
    let heading = line(
        &card.heading,
        0.06,
        TextVAlign::Middle,
        [235, 190, 80, 255],
        true,
    );
    let name = line(
        &card.name,
        0.05,
        TextVAlign::Middle,
        [240, 240, 240, 255],
        false,
    );
    let detail = line(
        &card.detail,
        0.035,
        TextVAlign::Middle,
        [175, 180, 190, 255],
        false,
    );
    over(&mut img, &heading, (-0.1 * hf) as i32);
    over(&mut img, &name, 0);
    over(&mut img, &detail, (0.09 * hf) as i32);
    img
}

/// `top` (straight alpha, the same size) over `img`, which is opaque, moved
/// down `dy` rows (up if negative).
fn over(img: &mut RgbaImage, top: &RgbaImage, dy: i32) {
    if top.width != img.width || top.height != img.height {
        return;
    }
    let (w, h) = (img.width as i64, img.height as i64);
    for y in 0..h {
        let ty = y - i64::from(dy);
        if ty < 0 || ty >= h {
            continue;
        }
        let row = (y * w * 4) as usize..((y + 1) * w * 4) as usize;
        let trow = (ty * w * 4) as usize..((ty + 1) * w * 4) as usize;
        for (d, s) in img.data[row]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(top.data[trow].as_chunks::<4>().0)
        {
            let a = u32::from(s[3]);
            if a == 0 {
                continue;
            }
            for i in 0..3 {
                d[i] = ((u32::from(s[i]) * a + u32::from(d[i]) * (255 - a)) / 255) as u8;
            }
        }
    }
}

/// What a computer that hasn't a file yet takes it to be: what the primary
/// read from it, or (not known) a picture with sound of no set length.
pub fn info_from(facts: Option<&Facts>) -> MediaInfo {
    match facts {
        Some(f) => MediaInfo {
            duration: f.duration(),
            video: f.video.then(|| VideoInfo {
                width: f.width.max(2),
                height: f.height.max(2),
                fps: f64::from(f.fps_milli) / 1000.0,
                matrix: ColorMatrix::Bt709,
                has_alpha: f.alpha,
                codec: f.codec.clone(),
                coded: (f.width.max(2), f.height.max(2)),
            }),
            has_audio: f.audio,
            audio_channels: f.channels,
            is_still: f.still,
        },
        None => MediaInfo {
            duration: None,
            video: Some(VideoInfo {
                width: 1920,
                height: 1080,
                fps: 25.0,
                matrix: ColorMatrix::Bt709,
                has_alpha: false,
                codec: String::new(),
                coded: (1920, 1080),
            }),
            has_audio: true,
            audio_channels: 2,
            is_still: false,
        },
    }
}
