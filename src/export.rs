//! The cue sheet: the cue list as a CSV file for a spreadsheet, or a
//! print-ready web page for the stage manager's desk.
//!
//! Each cue becomes a `Row` of plain text, built the way the cue list shows
//! it; the two formats are then just ways of writing those rows out.

use crate::engine::Engine;
use crate::show::{Cue, CueKind, Show};
use crate::ui;

pub const COLUMNS: [&str; 14] = [
    "Number",
    "Name",
    "Type",
    "Target",
    "Fades",
    "Pre-wait",
    "Duration",
    "Continue",
    "Armed",
    "Hotkey",
    "MIDI note",
    "Timecode",
    "Clock",
    "Notes",
];

/// One cue as text, in `COLUMNS` order, with whether it is a group's child
/// and whether it is armed (for the page's styling).
#[derive(Debug, PartialEq)]
pub struct Row {
    pub cells: [String; 14],
    pub child: bool,
    pub armed: bool,
    pub group: bool,
}

fn seconds(secs: f64) -> String {
    format!("{secs:.1} s")
}

/// A row for every cue, in list order.
pub fn rows(show: &Show, engine: &mut Engine) -> Vec<Row> {
    show.cues
        .iter()
        .map(|c| {
            let duration = match engine.cue_duration(show, c) {
                Some(Some(d)) if d > 0.0 => seconds(d),
                Some(None) => "∞".to_string(),
                _ => String::new(),
            };
            let live = show.cue_index(c.id).is_some_and(|i| show.is_armed(i));
            Row {
                cells: [
                    c.number.clone(),
                    c.name.clone(),
                    c.kind.cue_type().label().to_string(),
                    ui::cue_target_text(show, c),
                    ui::fades_text(c),
                    if c.pre_wait > 0.0 {
                        seconds(c.pre_wait)
                    } else {
                        String::new()
                    },
                    duration,
                    continue_text(c),
                    if c.armed { "" } else { "off" }.to_string(),
                    c.hotkey.clone().unwrap_or_default(),
                    c.midi_note.map(|n| n.to_string()).unwrap_or_default(),
                    c.timecode.map(|t| t.to_string()).unwrap_or_default(),
                    c.clock.map(|t| t.to_string()).unwrap_or_default(),
                    c.notes.trim().to_string(),
                ],
                child: c.parent.is_some(),
                armed: live,
                group: matches!(c.kind, CueKind::Group(_)),
            }
        })
        .collect()
}

fn continue_text(c: &Cue) -> String {
    use crate::show::ContinueMode::*;
    match c.continue_mode {
        _ if c.parent.is_some() => String::new(),
        DoNotContinue => String::new(),
        AutoContinue => "Auto-continue".into(),
        AutoFollow => "Auto-follow".into(),
    }
}

/// One CSV field: quoted if it holds a comma, a quote or a line break.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The sheet as CSV. It starts with a byte order mark, which is what makes
/// Excel read the accents and symbols as UTF-8.
pub fn to_csv(rows: &[Row]) -> String {
    let mut out = String::from('\u{feff}');
    let line = |cells: Vec<String>| cells.join(",") + "\r\n";
    out.push_str(&line(COLUMNS.iter().map(|c| c.to_string()).collect()));
    for r in rows {
        out.push_str(&line(r.cells.iter().map(|c| csv_field(c)).collect()));
    }
    out
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Year, month, day, hour and minute (UTC) of a time in seconds since 1970.
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32)
}

/// The sheet as a web page that prints well: landscape, a header that
/// repeats on each page, groups shaded, disarmed cues greyed.
pub fn to_html(show_name: &str, rows: &[Row], unix_secs: i64) -> String {
    let (y, mo, d, h, mi) = civil_from_unix(unix_secs);
    let title = html_escape(show_name);
    let mut out = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>{title} — cue sheet</title>\n<style>\n\
         @page {{ size: landscape; margin: 12mm; }}\n\
         body {{ font: 12px/1.35 system-ui, sans-serif; color: #111; margin: 16px; }}\n\
         h1 {{ font-size: 18px; margin: 0 0 2px; }}\n\
         .meta {{ color: #666; margin-bottom: 10px; }}\n\
         table {{ border-collapse: collapse; width: 100%; }}\n\
         thead {{ display: table-header-group; }}\n\
         th {{ text-align: left; background: #eee; border-bottom: 2px solid #999; padding: 4px 6px; }}\n\
         td {{ border-bottom: 1px solid #ccc; padding: 3px 6px; vertical-align: top; }}\n\
         tr {{ break-inside: avoid; }}\n\
         tr.group td {{ background: #f3f3f3; font-weight: 600; }}\n\
         tr.child td:nth-child(2) {{ padding-left: 22px; }}\n\
         tr.off td {{ color: #999; text-decoration: line-through; }}\n\
         td.n {{ font-variant-numeric: tabular-nums; white-space: nowrap; }}\n\
         td.notes {{ min-width: 180px; }}\n\
         </style>\n</head>\n<body>\n<h1>{title}</h1>\n\
         <div class=\"meta\">Cue sheet · {} cues · {y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC</div>\n\
         <table>\n<thead><tr>",
        rows.len()
    );
    for c in COLUMNS {
        out.push_str(&format!("<th>{c}</th>"));
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    for r in rows {
        let class: Vec<&str> = [
            r.group.then_some("group"),
            r.child.then_some("child"),
            (!r.armed).then_some("off"),
        ]
        .into_iter()
        .flatten()
        .collect();
        out.push_str(&format!("<tr class=\"{}\">", class.join(" ")));
        for (i, cell) in r.cells.iter().enumerate() {
            let td = match i {
                0 => "<td class=\"n\">",
                13 => "<td class=\"notes\">",
                _ => "<td>",
            };
            out.push_str(&format!(
                "{td}{}</td>",
                html_escape(cell).replace('\n', "<br>")
            ));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n</body>\n</html>\n");
    out
}

/// Which file format a name asks for: `.csv`, or else a web page.
pub fn is_csv(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
}

/// Write the cue sheet to `path`, as CSV if it ends in `.csv` and as a page
/// otherwise. Returns how many cues it holds.
pub fn write(path: &std::path::Path, show: &Show, engine: &mut Engine) -> std::io::Result<usize> {
    let rows = rows(show, engine);
    let text = if is_csv(path) {
        to_csv(&rows)
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        to_html(&show.name, &rows, now)
    };
    std::fs::write(path, text)?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TimeOfDay;
    use crate::show::{ContinueMode, CueType, GroupMode};

    fn demo() -> (Show, Engine) {
        let mut show = Show {
            name: "Hamlet <Act 1> & more".into(),
            ..Show::default()
        };
        let a = show.make_cue(CueType::Wait, None, 0);
        let a_id = a.id;
        show.cues.push(a);
        let mut g = show.make_cue(CueType::Group, None, 1);
        if let CueKind::Group(gc) = &mut g.kind {
            gc.mode = GroupMode::Timeline;
        }
        let g_id = g.id;
        show.cues.push(g);
        let mut child = show.make_cue(CueType::Wait, None, 2);
        child.parent = Some(g_id);
        child.name = "Child, with \"quotes\"".into();
        child.armed = false;
        show.cues.push(child);
        let cue = show.cue_mut(a_id).unwrap();
        cue.name = "Lights up".into();
        cue.notes = "  Wait for the\nhouse lights, then go  ".into();
        cue.hotkey = Some("F2".into());
        cue.midi_note = Some(36);
        cue.pre_wait = 1.5;
        cue.continue_mode = ContinueMode::AutoFollow;
        cue.timecode = Some(crate::timecode::Timecode::new(1, 0, 5, 12));
        cue.clock = TimeOfDay::new(18, 30, 0);
        (show, Engine::for_tests())
    }

    #[test]
    fn rows_say_what_the_cue_list_says() {
        let (show, mut e) = demo();
        let r = rows(&show, &mut e);
        assert_eq!(r.len(), 3);
        let c = &r[0].cells;
        assert_eq!(c[0], show.cues[0].number);
        assert_eq!(c[1], "Lights up");
        assert_eq!(c[2], "Wait");
        assert_eq!(c[5], "1.5 s");
        assert_eq!(c[6], "5.0 s", "a wait's own length");
        assert_eq!(c[7], "Auto-follow");
        assert_eq!(c[8], "");
        assert_eq!(
            (c[9].as_str(), c[10].as_str(), c[11].as_str()),
            ("F2", "36", "01:00:05:12")
        );
        assert_eq!(c[12], "18:30:00");
        assert_eq!(
            c[13], "Wait for the\nhouse lights, then go",
            "notes trimmed"
        );
        assert!(!r[0].child && r[0].armed && !r[0].group);
        // The group and its disarmed child.
        assert!(r[1].group && !r[1].child);
        assert!(r[2].child && !r[2].armed);
        assert_eq!(r[2].cells[8], "off");
        assert_eq!(r[2].cells[7], "", "a child's continue is the group's");
    }

    #[test]
    fn csv_quotes_what_needs_quoting_and_starts_with_a_bom() {
        let (show, mut e) = demo();
        let csv = to_csv(&rows(&show, &mut e));
        assert!(csv.starts_with('\u{feff}'));
        let body = csv.trim_start_matches('\u{feff}');
        let mut lines = body.split("\r\n");
        assert_eq!(
            lines.next().unwrap(),
            "Number,Name,Type,Target,Fades,Pre-wait,Duration,Continue,Armed,Hotkey,MIDI note,Timecode,Clock,Notes"
        );
        // The first cue's notes have a line break, so that field is quoted over two lines.
        assert!(
            body.contains("\"Wait for the\nhouse lights, then go\""),
            "{body}"
        );
        // Quotes are doubled, and a comma forces quoting.
        assert!(body.contains("\"Child, with \"\"quotes\"\"\""), "{body}");
        assert!(body.ends_with("\r\n"));
        // A plain field is left bare.
        assert!(body.contains(",Lights up,Wait,"));
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field(""), "");
    }

    #[test]
    fn html_escapes_everything_it_prints() {
        let (show, mut e) = demo();
        let page = to_html(&show.name, &rows(&show, &mut e), 1_700_000_000);
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("<title>Hamlet &lt;Act 1&gt; &amp; more — cue sheet</title>"));
        assert!(page.contains("<h1>Hamlet &lt;Act 1&gt; &amp; more</h1>"));
        assert!(!page.contains("<Act 1>"), "the show name is escaped");
        assert!(page.contains("Child, with &quot;quotes&quot;"));
        assert!(page.contains("Wait for the<br>house lights, then go"));
        assert!(
            page.contains("2023-11-14 22:13 UTC"),
            "{}",
            &page[page.find("Cue sheet").unwrap()..][..80]
        );
        // The row classes carry the structure: group, child, disarmed.
        assert!(page.contains("<tr class=\"group\">"));
        assert!(page.contains("<tr class=\"child off\">"));
        assert!(page.contains("<tr class=\"\">"));
        assert_eq!(page.matches("<tr class").count(), 3);
        assert_eq!(page.matches("<th>").count(), COLUMNS.len());
        assert!(page.contains("@page { size: landscape"));
        // Hostile text can't become markup.
        let nasty = Row {
            cells: std::array::from_fn(|_| "<script>alert(1)</script>".to_string()),
            child: false,
            armed: true,
            group: false,
        };
        let page = to_html("<img src=x onerror=alert(1)>", &[nasty], 0);
        assert!(
            !page.contains("<script>") && !page.contains("<img"),
            "{page}"
        );
    }

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil_from_unix(1_700_000_000), (2023, 11, 14, 22, 13));
        assert_eq!(
            civil_from_unix(951_782_400),
            (2000, 2, 29, 0, 0),
            "a leap day"
        );
        assert_eq!(civil_from_unix(1_709_164_799), (2024, 2, 28, 23, 59));
        assert_eq!(civil_from_unix(1_709_164_800), (2024, 2, 29, 0, 0));
        assert_eq!(civil_from_unix(1_709_251_200), (2024, 3, 1, 0, 0));
        assert_eq!(civil_from_unix(-1), (1969, 12, 31, 23, 59));
        assert_eq!(civil_from_unix(4_102_444_800), (2100, 1, 1, 0, 0));
    }

    #[test]
    fn the_extension_picks_the_format_and_files_are_written() {
        let dir = std::env::temp_dir().join("newq_export_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (show, mut e) = demo();
        assert!(is_csv(std::path::Path::new("a.csv")) && is_csv(std::path::Path::new("A.CSV")));
        assert!(!is_csv(std::path::Path::new("a.html")) && !is_csv(std::path::Path::new("a")));

        let csv = dir.join("sheet.csv");
        assert_eq!(write(&csv, &show, &mut e).unwrap(), 3);
        let text = std::fs::read_to_string(&csv).unwrap();
        assert!(text.starts_with('\u{feff}') && text.contains("Lights up"));
        let html = dir.join("sheet.html");
        assert_eq!(write(&html, &show, &mut e).unwrap(), 3);
        let text = std::fs::read_to_string(&html).unwrap();
        assert!(text.starts_with("<!doctype html>") && text.contains("Lights up"));
        // An empty show still makes a sheet.
        let empty = dir.join("empty.csv");
        assert_eq!(write(&empty, &Show::default(), &mut e).unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(&empty).unwrap().lines().count(),
            1,
            "just the header"
        );
        // An unwritable place is an error, not a panic.
        assert!(write(&dir.join("no/such/dir/x.csv"), &show, &mut e).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
