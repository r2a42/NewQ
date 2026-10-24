//! Undo and redo for the show.
//!
//! The show is plain data that serialises to JSON, and the control UI already
//! serialises it every frame to tell whether it has unsaved changes. So a
//! history is just a list of those JSON strings: every frame `observe` is
//! given the current one, and a change starts a "burst" that ends once the
//! show has been left alone for a moment. A drag, a slider or typing is one
//! step however many frames it took. Time is passed in, so tests don't wait.

use std::collections::VecDeque;

/// A burst of edits ends when the show has been still for this long (seconds).
pub const SETTLE: f64 = 0.6;
/// Steps kept, and the most memory they may use between them.
const MAX_STEPS: usize = 200;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub struct History {
    /// The show as it was before each step, oldest first.
    undo: VecDeque<String>,
    redo: Vec<String>,
    /// The show as last seen.
    current: Option<String>,
    /// An edit under way: the show before it began, and when it last changed.
    burst: Option<(String, f64)>,
}

impl History {
    /// Forget everything and start from `json` (a new or opened show).
    pub fn reset(&mut self, json: String) {
        self.undo.clear();
        self.redo.clear();
        self.burst = None;
        self.current = Some(json);
    }

    pub fn started(&self) -> bool {
        self.current.is_some()
    }

    /// Note the show as it is now (call every frame).
    pub fn observe(&mut self, json: &str, now: f64) {
        let Some(current) = &mut self.current else {
            self.current = Some(json.to_string());
            return;
        };
        if current != json {
            match &mut self.burst {
                Some((_, last)) => *last = now,
                None => self.burst = Some((std::mem::take(current), now)),
            }
            *current = json.to_string();
        } else if self
            .burst
            .as_ref()
            .is_some_and(|(_, last)| now - last >= SETTLE)
        {
            self.end_burst();
        }
    }

    /// Close the edit under way, so what comes next is a step of its own.
    pub fn end_burst(&mut self) {
        if let Some((before, _)) = self.burst.take() {
            self.undo.push_back(before);
            self.redo.clear();
            self.trim();
        }
    }

    fn trim(&mut self) {
        let mut bytes: usize = self.undo.iter().map(String::len).sum();
        while self.undo.len() > MAX_STEPS || (bytes > MAX_BYTES && self.undo.len() > 1) {
            if let Some(old) = self.undo.pop_front() {
                bytes -= old.len();
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.burst.is_some() || !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.burst.is_none() && !self.redo.is_empty()
    }

    /// Step back: the show to switch to, if there is anything to undo.
    pub fn undo(&mut self) -> Option<String> {
        self.end_burst();
        let before = self.undo.pop_back()?;
        let now = self.current.replace(before.clone())?;
        self.redo.push(now);
        Some(before)
    }

    /// Step forward again after an undo.
    pub fn redo(&mut self) -> Option<String> {
        self.end_burst();
        let after = self.redo.pop()?;
        let now = self.current.replace(after.clone())?;
        self.undo.push_back(now);
        Some(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A history that has seen `"a"` and then each of `edits` after the
    /// previous one has settled.
    fn settled(edits: &[&str]) -> History {
        let mut h = History::default();
        h.observe("a", 0.0);
        let mut t = 1.0;
        for e in edits {
            h.observe(e, t);
            t += SETTLE + 0.1;
            h.observe(e, t);
            t += 1.0;
        }
        h
    }

    #[test]
    fn nothing_to_undo_until_something_changes() {
        let mut h = History::default();
        assert!(!h.started());
        h.observe("a", 0.0);
        h.observe("a", 5.0);
        assert!(h.started() && !h.can_undo() && !h.can_redo());
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), None);
    }

    #[test]
    fn undo_and_redo_walk_through_the_edits() {
        let mut h = settled(&["b", "c", "d"]);
        assert!(h.can_undo() && !h.can_redo());
        assert_eq!(h.undo().as_deref(), Some("c"));
        assert_eq!(h.undo().as_deref(), Some("b"));
        assert_eq!(h.undo().as_deref(), Some("a"));
        assert_eq!(h.undo(), None);
        assert!(h.can_redo());
        assert_eq!(h.redo().as_deref(), Some("b"));
        assert_eq!(h.redo().as_deref(), Some("c"));
        assert_eq!(h.redo().as_deref(), Some("d"));
        assert_eq!(h.redo(), None);
        // Undoing is not itself an edit: seeing the restored show changes nothing.
        assert_eq!(h.undo().as_deref(), Some("c"));
        h.observe("c", 100.0);
        h.observe("c", 200.0);
        assert!(h.can_redo(), "the redo steps survive being observed");
    }

    #[test]
    fn a_burst_of_changes_is_one_step() {
        let mut h = History::default();
        h.observe("a", 0.0);
        // A drag: a new value every frame for a second.
        for i in 1..=60 {
            h.observe(&format!("a{i}"), 1.0 + f64::from(i) / 60.0);
        }
        assert!(h.can_undo(), "the edit under way can already be undone");
        h.observe("a60", 3.0);
        assert_eq!(
            h.undo().as_deref(),
            Some("a"),
            "one undo undoes the whole drag"
        );
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo().as_deref(), Some("a60"));
    }

    #[test]
    fn undoing_in_the_middle_of_a_burst_undoes_it() {
        let mut h = settled(&["b"]);
        h.observe("c", 10.0);
        h.observe("cc", 10.1);
        assert_eq!(
            h.undo().as_deref(),
            Some("b"),
            "the unfinished edit goes first"
        );
        assert_eq!(h.undo().as_deref(), Some("a"));
    }

    #[test]
    fn a_new_edit_clears_the_redo_steps() {
        let mut h = settled(&["b", "c"]);
        h.undo();
        assert!(h.can_redo());
        h.observe("x", 50.0);
        assert!(!h.can_redo(), "redo is unavailable mid-edit");
        h.observe("x", 60.0);
        assert_eq!(h.redo(), None);
        assert_eq!(h.undo().as_deref(), Some("b"));
    }

    #[test]
    fn end_burst_splits_edits_that_would_merge() {
        let mut h = History::default();
        h.observe("a", 0.0);
        h.observe("b", 0.1);
        h.end_burst();
        h.observe("c", 0.2);
        h.end_burst();
        assert_eq!(h.undo().as_deref(), Some("b"));
        assert_eq!(h.undo().as_deref(), Some("a"));
    }

    #[test]
    fn a_change_and_its_reversal_is_still_a_step() {
        // Toggle on then off inside one burst: the show is back as it was, but
        // that is rare enough, and harmless, to leave as a no-op step.
        let mut h = History::default();
        h.observe("a", 0.0);
        h.observe("b", 0.1);
        h.observe("a", 0.2);
        h.observe("a", 5.0);
        assert_eq!(h.undo().as_deref(), Some("a"));
    }

    #[test]
    fn reset_forgets_everything() {
        let mut h = settled(&["b", "c"]);
        h.undo();
        h.reset("z".into());
        assert!(!h.can_undo() && !h.can_redo());
        h.observe("z", 1.0);
        assert!(!h.can_undo());
        h.observe("y", 2.0);
        assert_eq!(h.undo().as_deref(), Some("z"));
    }

    #[test]
    fn the_history_is_bounded() {
        let mut h = History::default();
        h.observe("0", 0.0);
        let mut t = 1.0;
        for i in 1..=(MAX_STEPS + 50) {
            h.observe(&i.to_string(), t);
            h.end_burst();
            t += 1.0;
        }
        let mut steps = 0;
        while h.undo().is_some() {
            steps += 1;
        }
        assert_eq!(steps, MAX_STEPS);
        // And by size: a few huge shows can't use all the memory.
        let mut h = History::default();
        let big = "x".repeat(MAX_BYTES / 4);
        h.observe(&big, 0.0);
        for i in 0..10 {
            h.observe(&format!("{big}{i}"), 1.0 + f64::from(i));
            h.end_burst();
        }
        let mut steps = 0;
        while h.undo().is_some() {
            steps += 1;
        }
        assert!(steps <= 5, "kept {steps} steps of {} bytes", big.len());
    }
}
