//! What a PowerPoint file says about how its slides run: which slides are
//! hidden, how long each stays up on its own, and how long its transition is.
//!
//! The pictures of the slides are not made here (nothing but PowerPoint, or an
//! office program, draws a slide properly: see `slides_export`); this reads the
//! file's own words, which are XML inside a zip.

use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// One slide, in the order of the show.
#[derive(Clone, Debug, PartialEq)]
pub struct Slide {
    /// Left out of the slide show by whoever made the file.
    pub hidden: bool,
    /// Seconds before it goes on by itself, if it is set to.
    pub advance: Option<f64>,
    /// Seconds its transition takes, if it has one.
    pub fade: Option<f64>,
}

const PRESENTATION_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const RELATIONSHIP_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const P14_NS: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";

/// The longest time taken from a deck, in seconds (the inspector's own limit).
const MAX_SECONDS: f64 = 36_000.0;

/// A time in milliseconds as written in a deck, in seconds; nothing for text
/// that isn't a finite number ("inf", "1e999", "NaN": a saved show can't hold
/// those), and kept within 0 to `MAX_SECONDS`.
fn seconds_from_ms(text: &str) -> Option<f64> {
    let ms: f64 = text.trim().parse().ok()?;
    ms.is_finite()
        .then(|| (ms / 1000.0).clamp(0.0, MAX_SECONDS))
}

/// The most read from one part of a deck (a slide's XML is some kilobytes).
const MAX_PART: u64 = 32 << 20;

/// The slides of the `.pptx` at `path`, in order.
pub fn read(path: &Path) -> Result<Vec<Slide>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("can't open it: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("it isn't a PowerPoint file: {e}"))?;
    let mut text = |name: &str| -> Option<String> {
        let entry = zip.by_name(name).ok()?;
        // The sizes in a file are only claims: a crafted one could ask for
        // gigabytes, and running out of memory ends the whole program.
        if entry.size() > MAX_PART {
            return None;
        }
        let mut s = String::new();
        entry.take(MAX_PART).read_to_string(&mut s).ok()?;
        Some(s)
    };
    let presentation = text("ppt/presentation.xml").ok_or("no presentation in it")?;
    let rels = text("ppt/_rels/presentation.xml.rels").ok_or("no slide list in it")?;
    parse(&presentation, &rels, |name| text(&format!("ppt/{name}")))
}

/// The slides described by `presentation.xml` and its relationships; `slide`
/// gives the XML of a slide by its path under `ppt/`.
pub fn parse(
    presentation: &str,
    rels: &str,
    mut slide: impl FnMut(&str) -> Option<String>,
) -> Result<Vec<Slide>, String> {
    let rels_doc = Document::parse(rels).map_err(|e| format!("damaged slide list: {e}"))?;
    let targets: HashMap<&str, &str> = rels_doc
        .descendants()
        .filter(|n| n.has_tag_name("Relationship"))
        .filter_map(|n| Some((n.attribute("Id")?, n.attribute("Target")?)))
        .collect();
    let doc = Document::parse(presentation).map_err(|e| format!("damaged presentation: {e}"))?;
    let mut slides = Vec::new();
    for id in doc.descendants().filter(|n| n.tag_name().name() == "sldId") {
        let Some(rid) = id.attribute((RELATIONSHIP_NS, "id")) else {
            continue;
        };
        let Some(target) = targets.get(rid) else {
            continue;
        };
        // Targets are relative to ppt/ ("slides/slide1.xml"), or absolute.
        let name = target.trim_start_matches('/').trim_start_matches("ppt/");
        let xml = slide(name).ok_or_else(|| format!("slide {name} is missing"))?;
        slides.push(read_slide(&xml)?);
    }
    if slides.is_empty() {
        return Err("it has no slides".into());
    }
    Ok(slides)
}

fn read_slide(xml: &str) -> Result<Slide, String> {
    let doc = Document::parse(xml).map_err(|e| format!("damaged slide: {e}"))?;
    let root = doc.root_element();
    let hidden = root.attribute("show") == Some("0");
    // The first transition (newer files put it inside an alternate-content
    // block that also holds an older form: either says the same).
    let transition: Option<Node> = doc
        .descendants()
        .find(|n| n.has_tag_name((PRESENTATION_NS, "transition")));
    let (advance, fade) = match transition {
        None => (None, None),
        Some(t) => {
            let advance = t.attribute("advTm").and_then(seconds_from_ms);
            let fade = t.attribute((P14_NS, "dur")).and_then(seconds_from_ms).or(
                match t.attribute("spd") {
                    Some("slow") => Some(1.0),
                    Some("fast") => Some(0.5),
                    // Medium, and none said: PowerPoint's own default.
                    _ => Some(0.75),
                },
            );
            (advance, fade)
        }
    };
    Ok(Slide {
        hidden,
        advance,
        fade,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PRESENTATION: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
 <p:sldIdLst><p:sldId id="257" r:id="rId3"/><p:sldId id="256" r:id="rId2"/><p:sldId id="258" r:id="rId4"/></p:sldIdLst>
</p:presentation>"#;

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Id="rId2" Type="x" Target="slides/slide1.xml"/>
 <Relationship Id="rId3" Type="x" Target="slides/slide2.xml"/>
 <Relationship Id="rId4" Type="x" Target="slides/slide3.xml"/>
 <Relationship Id="rId9" Type="x" Target="theme/theme1.xml"/>
</Relationships>"#;

    fn slide_xml(inner: &str, attrs: &str) -> String {
        format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"
 xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" {attrs}>
 <p:cSld/>{inner}</p:sld>"#
        )
    }

    fn deck(slide: impl Fn(&str) -> Option<String>) -> Result<Vec<Slide>, String> {
        parse(PRESENTATION, RELS, slide)
    }

    #[test]
    fn slides_come_in_the_order_of_the_slide_list_with_their_timings() {
        let slides = deck(|name| match name {
            // Listed second, though it is called slide2: the list decides.
            "slides/slide2.xml" => Some(slide_xml("", "")),
            "slides/slide1.xml" => Some(slide_xml(
                r#"<p:transition spd="slow" advTm="4000"/>"#,
                "",
            )),
            "slides/slide3.xml" => Some(slide_xml(
                r#"<mc:AlternateContent><mc:Choice Requires="p14"><p:transition spd="med" p14:dur="1500" advTm="2500"/></mc:Choice><mc:Fallback><p:transition spd="med" advTm="2500"/></mc:Fallback></mc:AlternateContent>"#,
                r#"show="0""#,
            )),
            _ => None,
        })
        .unwrap();
        // The first in the list is slide2: nothing set.
        assert_eq!(
            slides[0],
            Slide {
                hidden: false,
                advance: None,
                fade: None
            }
        );
        assert_eq!(
            slides[1],
            Slide {
                hidden: false,
                advance: Some(4.0),
                fade: Some(1.0)
            }
        );
        // The newer form's own duration wins; this slide is hidden.
        assert_eq!(
            slides[2],
            Slide {
                hidden: true,
                advance: Some(2.5),
                fade: Some(1.5)
            }
        );
    }

    #[test]
    fn times_that_are_not_finite_numbers_are_dropped_and_huge_ones_capped() {
        // A saved show can't hold infinity (it is written as null and then
        // won't load), so a deck's "inf" or "1e999" must never get that far.
        for bad in ["inf", "-inf", "NaN", "1e999", "", "soon"] {
            assert_eq!(seconds_from_ms(bad), None, "{bad:?}");
        }
        assert_eq!(seconds_from_ms("2500"), Some(2.5));
        assert_eq!(seconds_from_ms(" 4000 "), Some(4.0));
        assert_eq!(seconds_from_ms("-300"), Some(0.0));
        assert_eq!(seconds_from_ms("1e15"), Some(MAX_SECONDS));
        let slides = deck(|name| match name {
            "slides/slide1.xml" => Some(slide_xml(
                r#"<p:transition advTm="1e999" p14:dur="inf"/>"#,
                "",
            )),
            _ => Some(slide_xml("", "")),
        })
        .unwrap();
        // (The list puts slide2 first: slide1, with the bad numbers, is second.)
        assert_eq!(slides[1].advance, None);
        assert!(slides[1].fade.is_some_and(f64::is_finite));
    }

    #[test]
    fn a_missing_slide_or_an_empty_list_is_an_error_not_a_guess() {
        assert!(deck(|_| None).unwrap_err().contains("missing"));
        let empty = r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst/></p:presentation>"#;
        assert!(
            parse(empty, RELS, |_| None)
                .unwrap_err()
                .contains("no slides")
        );
        assert!(parse("not xml", RELS, |_| None).is_err());
    }

    #[test]
    fn a_pptx_is_read_from_its_zip() {
        let dir = std::env::temp_dir().join(format!("newq_pptx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deck.pptx");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("ppt/presentation.xml", PRESENTATION.to_string()),
                ("ppt/_rels/presentation.xml.rels", RELS.to_string()),
                (
                    "ppt/slides/slide1.xml",
                    slide_xml(r#"<p:transition advTm="3000"/>"#, ""),
                ),
                ("ppt/slides/slide2.xml", slide_xml("", "")),
                ("ppt/slides/slide3.xml", slide_xml("", "")),
            ] {
                z.start_file(name, opts).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        let slides = read(&path).unwrap();
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[1].advance, Some(3.0));
        // Not a zip at all.
        std::fs::write(dir.join("not.pptx"), "hello").unwrap();
        assert!(read(&dir.join("not.pptx")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
