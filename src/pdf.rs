//! Drawing the pages of a PDF as pictures, with no program but NewQ itself.
//!
//! A presentation saved as a PDF (every presentation program can) is the most
//! reliable way to bring slides in: the pages are already drawn, with their
//! fonts. `hayro` (a PDF rasteriser in Rust, no unsafe code) draws them on the
//! processor, so this works on a computer with no office program and no
//! poppler. Encrypted PDFs aren't supported by it; a page that can't be drawn
//! is an error that names the page.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Pixels along the longer side of each picture: a slide is 1920 x 1080.
pub const LONG_EDGE: f32 = 1920.0;
/// The most pages taken from one file (a book isn't a presentation).
pub const MAX_PAGES: usize = 1000;
/// The biggest PDF read, in bytes.
const MAX_FILE: u64 = 1 << 30;

/// Draw every page of the PDF at `pdf` into `out_dir` as `Slide0001.png`,
/// `Slide0002.png`..., and say which files. Stops with "cancelled" when `cancel` is set.
pub fn render_pages(
    pdf: &Path,
    out_dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<PathBuf>, String> {
    let size = std::fs::metadata(pdf)
        .map_err(|e| format!("can't read the PDF: {e}"))?
        .len();
    if size > MAX_FILE {
        return Err("the PDF is bigger than 1 GB".into());
    }
    let data = std::fs::read(pdf).map_err(|e| format!("can't read the PDF: {e}"))?;
    let doc = Pdf::new(data).map_err(|e| format!("this isn't a PDF NewQ can read ({e:?})"))?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("can't make the folder {}: {e}", out_dir.display()))?;
    let pages = doc.pages();
    let count = pages.len();
    if count == 0 {
        return Err("the PDF has no pages".into());
    }
    if count > MAX_PAGES {
        return Err(format!(
            "the PDF has {count} pages: NewQ takes up to {MAX_PAGES} as slides"
        ));
    }
    let settings = InterpreterSettings::default();
    let cache = RenderCache::new();
    let mut files = Vec::with_capacity(count);
    for (n, page) in pages.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let (w, h) = page.render_dimensions();
        if !(w > 0.0 && h > 0.0) {
            return Err(format!("page {} has no size", n + 1));
        }
        // The picture is `LONG_EDGE` along its longer side, whatever the page's
        // size (a lower limit on the scale let a page declaring millions of
        // points ask for a picture of 100000 pixels, a bitmap of gigabytes: that
        // is an abort, which nothing here can catch). A page too thin to have a
        // pixel on its short side is no slide.
        let scale = (LONG_EDGE / w.max(h)).min(16.0);
        if w * scale < 1.0 || h * scale < 1.0 {
            return Err(format!("page {} has a shape that can't be drawn", n + 1));
        }
        // A bad page must not take the show down with it.
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let pixmap = render(
                page,
                &cache,
                &settings,
                &RenderSettings::default(),
                &PixmapSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: WHITE,
                },
            );
            pixmap.into_png()
        }));
        let png = match drawn {
            Ok(Ok(png)) => png,
            _ => return Err(format!("page {} couldn't be drawn", n + 1)),
        };
        let file = out_dir.join(format!("Slide{:04}.png", n + 1));
        std::fs::write(&file, png).map_err(|e| format!("can't write {}: {e}", file.display()))?;
        files.push(file);
    }
    Ok(files)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PDF by hand: `pages` landscape pages, each a red square on white.
    pub(crate) fn tiny_pdf(pages: usize) -> Vec<u8> {
        sized_pdf(pages, 400, 225)
    }

    /// The same, with pages `width` x `height` points.
    fn sized_pdf(pages: usize, width: u64, height: u64) -> Vec<u8> {
        let mut objects: Vec<String> = Vec::new();
        objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
        objects.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages
        ));
        for i in 0..pages {
            let content = 4 + 2 * i;
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents {content} 0 R /Resources << >> >>"
            ));
            let stream = "1 0 0 rg 100 50 200 125 re f";
            objects.push(format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }

    #[test]
    fn each_page_becomes_a_picture_of_the_slide_size_in_order() {
        let dir = std::env::temp_dir().join(format!("newq_pdf_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("deck.pdf");
        std::fs::write(&pdf, tiny_pdf(3)).unwrap();
        let out = dir.join("pics");
        let files = render_pages(&pdf, &out, &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files[0].ends_with("Slide0001.png") && files[2].ends_with("Slide0003.png"));
        let img = image::open(&files[1]).unwrap().to_rgba8();
        // A 400 x 225 page, 1920 pixels across: 1920 x 1080.
        assert_eq!((img.width(), img.height()), (1920, 1080));
        // Red in the middle, white at the corner.
        let mid = img.get_pixel(960, 540).0;
        assert!(mid[0] > 240 && mid[1] < 20 && mid[2] < 20, "{mid:?}");
        assert_eq!(img.get_pixel(5, 5).0, [255, 255, 255, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_of_any_declared_size_is_drawn_no_bigger_than_a_slide() {
        let dir = std::env::temp_dir().join(format!("newq_pdf_big_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let go = Arc::new(AtomicBool::new(false));
        // A page declaring two million points a side would have been a bitmap of
        // gigabytes (an abort, not an error): it is drawn at slide size.
        let huge = dir.join("huge.pdf");
        std::fs::write(&huge, sized_pdf(1, 2_000_000, 2_000_000)).unwrap();
        let files = render_pages(&huge, &dir.join("a"), &go).unwrap();
        let img = image::open(&files[0]).unwrap();
        assert!(
            img.width() <= 1920 && img.height() <= 1920,
            "{}x{}",
            img.width(),
            img.height()
        );
        // A page too thin for a pixel on its short side is an error, not a crash.
        let thin = dir.join("thin.pdf");
        std::fs::write(&thin, sized_pdf(1, 2_000_000, 10)).unwrap();
        let err = render_pages(&thin, &dir.join("b"), &go).unwrap_err();
        assert!(err.contains("shape"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_not_a_pdf_and_a_cancel_say_so() {
        let dir = std::env::temp_dir().join(format!("newq_pdf_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.pdf");
        std::fs::write(&junk, "not a pdf at all").unwrap();
        let go = Arc::new(AtomicBool::new(false));
        assert!(render_pages(&junk, &dir.join("o"), &go).is_err());
        assert!(render_pages(&dir.join("none.pdf"), &dir.join("o"), &go).is_err());
        let pdf = dir.join("ok.pdf");
        std::fs::write(&pdf, tiny_pdf(2)).unwrap();
        let stop = Arc::new(AtomicBool::new(true));
        assert_eq!(
            render_pages(&pdf, &dir.join("o2"), &stop).unwrap_err(),
            "cancelled"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
