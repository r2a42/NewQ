//! What the media manager says, in words for the people running the show:
//! the card on a display, a computer's progress, its policy, the notice.
//! Pure, tested.

use super::{Local, Transfer};
use crate::net::store::local::Policy;
use crate::net::store::pull::Progress;
use crate::net::store::space;

/// "about 2 min", "less than a minute", "about 1 h 20 min".
pub fn time_left(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "working out the time left".into();
    }
    if secs < 1.0 {
        return "nearly there".into();
    }
    if secs < 60.0 {
        return "less than a minute".into();
    }
    let min = (secs / 60.0).round() as u64;
    if min < 60 {
        return format!("about {min} min");
    }
    let (h, m) = (min / 60, min % 60);
    if m == 0 {
        format!("about {h} h")
    } else {
        format!("about {h} h {m} min")
    }
}

/// The file's own name, from a path as the primary names it (either kind of
/// separator).
pub fn file_name(show_path: &str) -> String {
    show_path
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(show_path)
        .to_string()
}

/// What a card in a cue's place says: its heading, the file's name and a line
/// under it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Card {
    pub heading: String,
    pub name: String,
    pub detail: String,
}

/// The card for a file that isn't here (None when it is).
pub fn card(show_path: &str, local: &Local) -> Option<Card> {
    let name = file_name(show_path);
    match local {
        Local::Here(_) => None,
        Local::Transferring(t) => Some(Card {
            heading: "Media transferring".into(),
            name,
            detail: transfer_line(t),
        }),
        Local::NotFound => Some(Card {
            heading: "Media not found".into(),
            name,
            detail: format!("Not on any computer of the show: {show_path}"),
        }),
    }
}

/// "43 % · about 2 min", or "waiting its turn".
pub fn transfer_line(t: &Transfer) -> String {
    if !t.known {
        return "Waiting for the primary's list of media".into();
    }
    if t.done == 0 {
        return format!("Waiting its turn · {}", space::words(t.total));
    }
    let pct = t.percent().floor();
    match t.eta_secs {
        Some(s) => format!("{pct:.0} % · {}", time_left(s)),
        None => format!("{pct:.0} %"),
    }
}

/// "Keeps 5 versions", "Replaces", "Keeps 5 versions · replaces files over 4 GB".
pub fn policy(p: &Policy) -> String {
    if p.replace_all {
        return "Replaces (keeps no older versions)".into();
    }
    let keep = match p.keep.max(1) {
        1 => "Keeps the current version only".to_string(),
        n => format!("Keeps {n} versions"),
    };
    match p.replace_over {
        Some(over) => format!("{keep} · replaces files over {}", space::words(over)),
        None => keep,
    }
}

/// A rate in bytes a second: "45 MB/s".
pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", space::words(bytes_per_sec.max(0.0) as u64))
}

/// One computer's (or all of them together's) progress on one line:
/// "62 % · 3.1 GB left · 45 MB/s · about 14 min".
pub fn progress(p: &Progress) -> String {
    let all = p.all();
    if all.files_wanted == 0 && all.files_not_found == 0 {
        return "Nothing to copy yet".into();
    }
    if all.complete() {
        let mut s = format!(
            "All here · {} file{}",
            all.files_here,
            if all.files_here == 1 { "" } else { "s" }
        );
        if all.files_not_found > 0 {
            s.push_str(&format!(" · {} not found", all.files_not_found));
        }
        return s;
    }
    let mut parts = vec![
        format!("{:.0} %", p.percent().floor()),
        format!("{} left", space::words(all.bytes_left())),
    ];
    if let Some(r) = p.rate.filter(|r| *r > 0.0) {
        parts.push(rate(r));
    }
    parts.push(match p.eta_secs {
        Some(s) => time_left(s),
        None => "working out the time left".into(),
    });
    if all.files_not_found > 0 {
        parts.push(format!("{} not found", all.files_not_found));
    }
    parts.join(" · ")
}

/// Whether anything is still to come (the used files or the library).
pub fn busy(p: &Progress) -> bool {
    !p.all().complete()
}

/// The primary's status-bar notice while transfers run (R16):
/// "Media transferring to 3 computers · 62 % · about 14 min · playback may be
/// less smooth meanwhile". None when nothing is on its way.
pub fn notice(sum: &Progress, busy_computers: usize) -> Option<String> {
    if busy_computers == 0 || !busy(sum) {
        return None;
    }
    let to = if busy_computers == 1 {
        "1 computer".to_string()
    } else {
        format!("{busy_computers} computers")
    };
    let eta = match sum.eta_secs {
        Some(s) => time_left(s),
        None => "working out the time left".into(),
    };
    Some(format!(
        "Media transferring to {to} · {:.0} % · {eta} · playback may be less smooth meanwhile",
        sum.percent().floor()
    ))
}

/// A size typed or chosen for "replace files over": in GB.
pub fn gb(bytes: u64) -> f64 {
    bytes as f64 / 1e9
}
