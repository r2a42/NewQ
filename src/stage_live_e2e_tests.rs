//! A mapped projector's display from end to end, by the real app: a show
//! whose canvas shows a known picture through two mapped projectors on one
//! curved screen in the Proscenium sample (`soak::mapped_stage`), the left
//! projector's display recorded through the stream path
//! (`StreamKind::Record`), and the canvas itself recorded beside it (the
//! codec's own error, unmapped). `scripts/mapped_e2e.sh` writes the show,
//! runs the app headless on it and runs the comparison; both tests here are
//! ignored, and do nothing without `NEWQ_MAPPED_E2E` (the folder).
//!
//! The comparison: a late frame of each recording, decoded by FFmpeg,
//! against what `lensmap::reference` says that projector shows at the
//! recording's size (the canvas's picture at each pixel's place), away from
//! the edges of what it sees.

use crate::show::{CueType, OutputId, RecordCodec, RecordContainer, Show, StreamKind, StreamOut};
use crate::stage3d::lensmap::{feeds_for, reference_every, still_scene};
use std::path::{Path, PathBuf};

/// The canvas's picture.
const PICTURE: (u32, u32) = (1920, 1080);

/// The known picture: smooth, so a place off by a little is a colour off by
/// a little (red across, green down), blue a slow wave.
fn picture_at(u: f32, v: f32) -> [f32; 3] {
    [
        0.15 + 0.7 * u,
        0.15 + 0.7 * v,
        0.5 + 0.3 * (u * std::f32::consts::TAU).sin() * 0.5,
    ]
}

fn dir() -> Option<PathBuf> {
    std::env::var_os("NEWQ_MAPPED_E2E").map(PathBuf::from)
}

fn record(folder: &Path) -> StreamOut {
    StreamOut {
        enabled: true,
        kind: StreamKind::Record {
            folder: folder.to_string_lossy().into_owned(),
            codec: RecordCodec::H264,
            container: RecordContainer::Mkv,
            kbps: 40_000,
            area: None,
            audio: None,
        },
        fps: 30,
        height: 720,
    }
}

/// The show: the picture as an image cue on the canvas (the first
/// display), `soak::mapped_stage`'s screen and projectors, the left
/// projector's display and the canvas recorded into `rec-mapped` and
/// `rec-canvas`.
fn the_show(dir: &Path) -> (Show, OutputId, crate::stage3d::ProjectorId) {
    let png = dir.join("picture.png");
    let mut px = Vec::with_capacity((PICTURE.0 * PICTURE.1 * 3) as usize);
    for y in 0..PICTURE.1 {
        for x in 0..PICTURE.0 {
            let c = picture_at(
                (x as f32 + 0.5) / PICTURE.0 as f32,
                (y as f32 + 0.5) / PICTURE.1 as f32,
            );
            px.extend(c.map(|v| (v * 255.0).round() as u8));
        }
    }
    image::save_buffer(
        &png,
        &px,
        PICTURE.0,
        PICTURE.1,
        image::ExtendedColorType::Rgb8,
    )
    .expect("the picture is written");
    let mut show = Show::default();
    show.name = "Mapped end to end".into();
    let canvas = show.outputs[0].id;
    let mut c = show.make_cue(CueType::Image, Some(png), 0);
    c.name = "The known picture".into();
    if let Some(m) = c.kind.media_mut() {
        m.output = Some(canvas);
    }
    show.cues.push(c);
    let [left, _] = crate::soak::mapped_stage(&mut show, canvas);
    let display = show
        .projector(left)
        .and_then(|p| p.display)
        .expect("a display");
    for (id, folder) in [(display, "rec-mapped"), (canvas, "rec-canvas")] {
        let f = dir.join(folder);
        let _ = std::fs::create_dir_all(&f);
        if let Some(o) = show.outputs.iter_mut().find(|o| o.id == id) {
            o.stream = Some(record(&f));
        }
    }
    (show, canvas, left)
}

/// Writes the show (`mapped.newq`) and its picture into `NEWQ_MAPPED_E2E`.
#[test]
#[ignore]
fn write_a_mapped_recording_show() {
    let Some(dir) = dir() else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let (show, _, _) = the_show(&dir);
    show.save(&dir.join("mapped.newq")).unwrap();
}

/// The newest recording in `folder`.
fn newest(folder: &Path) -> PathBuf {
    std::fs::read_dir(folder)
        .unwrap_or_else(|e| panic!("{}: {e}", folder.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "mkv"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
        .unwrap_or_else(|| panic!("nothing recorded in {}", folder.display()))
}

/// The last frame of `file` (a recording NewQ was stopped in the middle
/// of: it has no index to seek by, so it is decoded to its end), as RGB
/// rows, and its size.
fn late_frame(file: &Path) -> (Vec<u8>, (u32, u32)) {
    let png = file.with_extension("last.png");
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(file)
        .args(["-update", "1"])
        .arg(&png)
        .output()
        .expect("ffmpeg runs");
    let img = image::open(&png)
        .unwrap_or_else(|e| {
            panic!(
                "a frame of {}: {e} {}",
                file.display(),
                String::from_utf8_lossy(&out.stderr)
            )
        })
        .to_rgb8();
    let size = img.dimensions();
    (img.into_raw(), size)
}

/// Errors (the largest of a pixel's three, 0..1), summed up.
#[derive(Debug)]
struct Errors {
    compared: usize,
    mean: f32,
    p95: f32,
    p99: f32,
    worst: f32,
}

fn sum_up(mut e: Vec<f32>) -> Errors {
    e.sort_by(f32::total_cmp);
    let n = e.len().max(1);
    Errors {
        compared: e.len(),
        mean: e.iter().sum::<f32>() / n as f32,
        p95: e.get(e.len() * 95 / 100).copied().unwrap_or(f32::NAN),
        p99: e.get(e.len() * 99 / 100).copied().unwrap_or(f32::NAN),
        worst: e.last().copied().unwrap_or(f32::NAN),
    }
}

fn rgb(img: &[u8], w: u32, x: u32, y: u32) -> [f32; 3] {
    let i = ((y * w + x) * 3) as usize;
    [img[i], img[i + 1], img[i + 2]].map(|v| f32::from(v) / 255.0)
}

fn off(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

/// The recordings against the reference. Prints the numbers.
#[test]
#[ignore]
fn a_recorded_mapped_display_shows_what_the_reference_says() {
    let Some(dir) = dir() else { return };
    let (show, _, left) = the_show(&dir);
    // The codec's own error: the canvas, unmapped, against its picture.
    let (canvas, csize) = late_frame(&newest(&dir.join("rec-canvas")));
    let mut floor = Vec::new();
    for y in 2..csize.1 - 2 {
        for x in 2..csize.0 - 2 {
            let want = picture_at(
                (x as f32 + 0.5) / csize.0 as f32,
                (y as f32 + 0.5) / csize.1 as f32,
            );
            floor.push(off(rgb(&canvas, csize.0, x, y), want));
        }
    }
    let floor = sum_up(floor);
    // The mapped display, against the reference at its size.
    let (mapped, size) = late_frame(&newest(&dir.join("rec-mapped")));
    let venue = show.stage3d.venue.clone().expect("the sample");
    let loaded =
        crate::stage3d::load::load_venue(Path::new(""), &venue).expect("the sample is made");
    let scene = show.stage3d.scene(&show, Some(&loaded));
    let still = still_scene(&show, &scene);
    let p = show.projector(left).expect("there").clone();
    let feeds = feeds_for(&show, left);
    // Every fourth pixel each way (the processor's sum is slow over a venue).
    const STEP: u32 = 4;
    let map = reference_every(&still, &p, size, p.aspect(size), &feeds, STEP);
    let at = |x: u32, y: u32| {
        let i = (y * size.0 + x) as usize;
        (
            crate::mapped::unpack_how(map.how[i]).feed,
            crate::mapped::unpack_place(map.place[i]),
        )
    };
    let (mut on, mut black, mut lit_black) = (Vec::new(), 0usize, 0usize);
    let near = 3 * STEP;
    for y in (near..size.1 - near).step_by(STEP as usize) {
        for x in (near..size.0 - near).step_by(STEP as usize) {
            let (feed, place) = at(x, y);
            // Away from edges: every worked-out pixel three steps round sees
            // the same.
            let edge = (y - near..=y + near).step_by(STEP as usize).any(|yy| {
                (x - near..=x + near)
                    .step_by(STEP as usize)
                    .any(|xx| at(xx, yy).0 != feed)
            });
            if edge {
                continue;
            }
            let got = rgb(&mapped, size.0, x, y);
            match feed {
                Some(_) => on.push(off(got, picture_at(place[0], place[1]))),
                None => {
                    black += 1;
                    if got.iter().any(|v| *v > 0.1) {
                        lit_black += 1;
                    }
                }
            }
        }
    }
    let mapped_errors = sum_up(on);
    let report = format!(
        "recording {}x{}; canvas unmapped (codec floor): {floor:?}\nmapped display against lensmap::reference: {mapped_errors:?}\nblack where the reference sees nothing: {} of {black} pixels lit\n",
        size.0, size.1, lit_black
    );
    eprintln!("{report}");
    let _ = std::fs::write(dir.join("comparison.txt"), &report);
    assert!(mapped_errors.compared > 5_000, "{report}");
    assert!(
        mapped_errors.mean <= floor.mean + 0.02,
        "the mapped picture is the reference's within the codec's error and 2%: {report}"
    );
    assert!(mapped_errors.p95 <= floor.p95 + 0.05, "{report}");
    assert!(lit_black * 100 <= black.max(1), "{report}");
}
