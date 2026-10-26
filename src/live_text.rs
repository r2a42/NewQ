//! Text that changes while it is on screen: the time of day and the speaker
//! timer. The text cue draws one picture of its words; for these the engine
//! draws a new one each time the shown time changes.

use crate::show::{TextCue, TextSource};

/// Colour of a timer that is running out, and one that has run out.
pub const WARNING_COLOR: [u8; 3] = [255, 190, 40];
pub const OVER_COLOR: [u8; 3] = [255, 70, 60];
/// What stands for the time in a live cue's text.
pub const PLACEHOLDER: &str = "{time}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Warning,
    Over,
}

/// "9:05" or, from an hour up, "1:02:03". Whole seconds, never negative.
pub fn format_span(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The time of day as "18:30", "6:30 PM" or with seconds.
pub fn format_clock(hms: (u32, u32, u32), hour24: bool, seconds: bool) -> String {
    let (h, m, s) = hms;
    let (shown_h, suffix) = if hour24 {
        (h, "")
    } else {
        (
            if h % 12 == 0 { 12 } else { h % 12 },
            if h < 12 { " AM" } else { " PM" },
        )
    };
    if seconds {
        format!("{shown_h}:{m:02}:{s:02}{suffix}")
    } else {
        format!("{shown_h}:{m:02}{suffix}")
    }
}

/// What a timer shows `elapsed` seconds in. A countdown rounds up, so it
/// reads its full length at the start and 0:00 for the last second, then
/// counts the overrun with a minus sign. A count up has a limit only when
/// `total` is above zero.
pub fn timer(total: f64, up: bool, warn: f64, elapsed: f64) -> (String, Tone) {
    let elapsed = elapsed.max(0.0);
    if up {
        let secs = elapsed.floor() as u64;
        let tone = if total > 0.0 && elapsed >= total {
            Tone::Over
        } else {
            Tone::Normal
        };
        return (format_span(secs), tone);
    }
    let left = (total - elapsed).ceil() as i64;
    let text = if left < 0 {
        format!("-{}", format_span(left.unsigned_abs()))
    } else {
        format_span(left as u64)
    };
    let tone = if left <= 0 {
        Tone::Over
    } else if warn > 0.0 && (left as f64) <= warn {
        Tone::Warning
    } else {
        Tone::Normal
    };
    (text, tone)
}

/// The cue as it should be drawn now, or None for plain text. `elapsed` is
/// seconds since the cue started; `now` the local time of day.
pub fn resolve(cue: &TextCue, elapsed: f64, now: (u32, u32, u32)) -> Option<TextCue> {
    let (shown, tone) = match cue.source {
        TextSource::Static => return None,
        TextSource::Clock { hour24, seconds } => (format_clock(now, hour24, seconds), Tone::Normal),
        TextSource::Timer { total, up, warn } => timer(total, up, warn, elapsed),
    };
    let mut out = cue.clone();
    out.text = if cue.text.contains(PLACEHOLDER) {
        cue.text.replace(PLACEHOLDER, &shown)
    } else {
        shown
    };
    match tone {
        Tone::Normal => {}
        Tone::Warning => out.color[..3].copy_from_slice(&WARNING_COLOR),
        Tone::Over => out.color[..3].copy_from_slice(&OVER_COLOR),
    }
    Some(out)
}

/// The local time of day now.
pub fn now_hms() -> (u32, u32, u32) {
    let t = jiff::Zoned::now().time();
    (t.hour() as u32, t.minute() as u32, t.second() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(source: TextSource, text: &str) -> TextCue {
        let mut c = TextCue::new(None);
        c.text = text.into();
        c.source = source;
        c
    }

    #[test]
    fn spans_switch_to_hours() {
        assert_eq!(format_span(0), "0:00");
        assert_eq!(format_span(65), "1:05");
        assert_eq!(format_span(3599), "59:59");
        assert_eq!(format_span(3723), "1:02:03");
    }

    #[test]
    fn clock_formats() {
        assert_eq!(format_clock((18, 5, 9), true, false), "18:05");
        assert_eq!(format_clock((18, 5, 9), true, true), "18:05:09");
        assert_eq!(format_clock((18, 5, 9), false, false), "6:05 PM");
        assert_eq!(format_clock((0, 5, 9), false, true), "12:05:09 AM");
        assert_eq!(format_clock((12, 0, 0), false, false), "12:00 PM");
        assert_eq!(format_clock((9, 0, 0), true, false), "9:00");
    }

    #[test]
    fn countdown_rounds_up_and_runs_over() {
        assert_eq!(
            timer(600.0, false, 60.0, 0.0),
            ("10:00".into(), Tone::Normal)
        );
        assert_eq!(
            timer(600.0, false, 60.0, 0.4),
            ("10:00".into(), Tone::Normal)
        );
        assert_eq!(
            timer(600.0, false, 60.0, 1.0),
            ("9:59".into(), Tone::Normal)
        );
        assert_eq!(
            timer(600.0, false, 60.0, 540.0),
            ("1:00".into(), Tone::Warning)
        );
        assert_eq!(
            timer(600.0, false, 60.0, 599.5),
            ("0:01".into(), Tone::Warning)
        );
        assert_eq!(
            timer(600.0, false, 60.0, 600.0),
            ("0:00".into(), Tone::Over)
        );
        assert_eq!(
            timer(600.0, false, 60.0, 601.5),
            ("-0:01".into(), Tone::Over)
        );
        assert_eq!(
            timer(600.0, false, 0.0, 570.0),
            ("0:30".into(), Tone::Normal)
        );
    }

    #[test]
    fn count_up_has_a_limit_only_when_set() {
        assert_eq!(timer(0.0, true, 0.0, 125.9), ("2:05".into(), Tone::Normal));
        assert_eq!(timer(0.0, true, 0.0, 99999.0).1, Tone::Normal);
        assert_eq!(timer(60.0, true, 0.0, 59.0).1, Tone::Normal);
        assert_eq!(timer(60.0, true, 0.0, 60.0).1, Tone::Over);
    }

    #[test]
    fn plain_text_is_not_live() {
        assert!(resolve(&live(TextSource::Static, "Hello"), 3.0, (1, 2, 3)).is_none());
    }

    #[test]
    fn placeholder_is_replaced_or_text_is_the_time() {
        let src = TextSource::Timer {
            total: 60.0,
            up: false,
            warn: 10.0,
        };
        let r = resolve(&live(src, "Left: {time}"), 5.0, (0, 0, 0)).unwrap();
        assert_eq!(r.text, "Left: 0:55");
        let r = resolve(&live(src, ""), 5.0, (0, 0, 0)).unwrap();
        assert_eq!(r.text, "0:55");
    }

    #[test]
    fn timer_colour_follows_its_state() {
        let src = TextSource::Timer {
            total: 60.0,
            up: false,
            warn: 10.0,
        };
        let base = live(src, "{time}").color;
        assert_eq!(
            resolve(&live(src, "{time}"), 5.0, (0, 0, 0)).unwrap().color,
            base
        );
        let w = resolve(&live(src, "{time}"), 55.0, (0, 0, 0)).unwrap();
        assert_eq!(w.color[..3], WARNING_COLOR);
        assert_eq!(w.color[3], base[3]);
        let o = resolve(&live(src, "{time}"), 61.0, (0, 0, 0)).unwrap();
        assert_eq!(o.color[..3], OVER_COLOR);
    }

    #[test]
    fn clock_follows_the_time_of_day() {
        let src = TextSource::Clock {
            hour24: true,
            seconds: true,
        };
        let r = resolve(&live(src, "{time}"), 0.0, (7, 8, 9)).unwrap();
        assert_eq!(r.text, "7:08:09");
    }
}
