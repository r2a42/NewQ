//! How a media cue's decoders run: once, or repeating a slice of the file.
//!
//! ffmpeg can't loop a slice by itself (`-stream_loop` repeats the whole
//! file, and an input `-t` ends the whole input after one pass), and the
//! concat demuxer drifts by whole codec frames. So a looping slice is decoded
//! one exact pass at a time, with the next pass's ffmpeg started before the
//! current one ends so the join has no gap. The readers trim or pad every pass
//! to an exact frame/sample count, which keeps audio and video on one timeline.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::process::{Child, ChildStdout, Command};
use std::sync::Arc;

/// Slices shorter than this are not looped (a restart per few frames would
/// spend more time starting ffmpeg than playing).
pub const MIN_SLICE: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Playback {
    /// Play from the in point once, for `duration` seconds if given.
    Once { duration: Option<f64> },
    /// Repeat the `len` seconds after the in point; `passes` None = forever.
    Slice { len: f64, passes: Option<u32> },
    /// A `Slice` started part way through: the first pass begins `skip`
    /// seconds into the slice (and so is `len - skip` long); the passes after
    /// it are whole. This is what a jump inside a loop plays.
    SliceFrom {
        len: f64,
        passes: Option<u32>,
        skip: f64,
    },
    /// The length isn't known: let ffmpeg loop the whole file forever.
    WholeFile,
}

impl Playback {
    /// Work out how a cue plays. `end` is the out point, `total` the file's
    /// duration if known, and `passes` the loop count (None = forever).
    pub fn plan(
        start: f64,
        end: Option<f64>,
        total: Option<f64>,
        looping: bool,
        passes: Option<u32>,
    ) -> Playback {
        let len = end.or(total).map(|e| (e - start).max(0.0));
        if !looping {
            return Playback::Once {
                duration: end.map(|e| (e - start).max(0.0)),
            };
        }
        match len.filter(|l| *l >= MIN_SLICE) {
            Some(len) => Playback::Slice { len, passes },
            None => Playback::WholeFile,
        }
    }

    /// Total running time, None if it never ends or isn't known.
    pub fn total(self, file_len: Option<f64>) -> Option<f64> {
        match self {
            Playback::Once { duration } => duration.or(file_len),
            Playback::Slice { len, passes } => passes.map(|n| len * n as f64),
            Playback::SliceFrom { len, passes, skip } => {
                passes.map(|n| (len * n as f64 - skip).max(0.0))
            }
            Playback::WholeFile => None,
        }
    }

    /// Number of passes the decoders make.
    pub fn passes(self) -> Option<u32> {
        match self {
            Playback::Slice { passes, .. } | Playback::SliceFrom { passes, .. } => passes,
            _ => Some(1),
        }
    }

    /// Exact length of a pass, if passes are trimmed to a fixed length.
    pub fn pass_len(self) -> Option<f64> {
        match self {
            Playback::Slice { len, .. } | Playback::SliceFrom { len, .. } => Some(len),
            _ => None,
        }
    }

    /// The length of the first pass: a whole one unless the playback began
    /// part way through the slice.
    pub fn first_pass_len(self) -> Option<f64> {
        match self {
            Playback::SliceFrom { len, skip, .. } => Some((len - skip).max(0.0)),
            other => other.pass_len(),
        }
    }

    /// This playback as it is started `pos` seconds into its running (a jump
    /// in a loop): in the pass `pos` falls in, `pos` minus the passes before
    /// it into the slice, with only the passes that are left to play. Playbacks
    /// that aren't loops are returned as they are.
    pub fn started_at(self, pos: f64) -> Playback {
        let (len, passes) = match self {
            Playback::Slice { len, passes } => (len, passes),
            Playback::SliceFrom { len, passes, .. } => (len, passes),
            other => return other,
        };
        let pos = pos.max(0.0);
        let done = (pos / len).floor();
        let done_passes = done as u32;
        let left = passes.map(|n| n.saturating_sub(done_passes).max(1));
        Playback::SliceFrom {
            len,
            passes: left,
            skip: pos - done * len,
        }
    }

    /// The ffmpeg input options for the first pass, which a jump in a loop
    /// starts part way through the slice.
    pub fn first_input_args(self, start: f64) -> Vec<String> {
        match self {
            Playback::SliceFrom { skip, .. } => self.input_args(start + skip),
            other => other.input_args(start),
        }
    }

    /// `limit_args` for the first pass.
    #[cfg(test)]
    pub fn first_limit_args(self) -> Vec<String> {
        self.first_limit_args_at(1.0)
    }

    /// `first_limit_args` for a cue played at `rate` times its speed: the
    /// limit is on what comes out, which takes `1 / rate` of the file's time.
    pub fn first_limit_args_at(self, rate: f64) -> Vec<String> {
        match self.first_pass_len() {
            Some(d) if self.pass_len().is_some() => {
                vec!["-t".to_string(), format!("{:.3}", d / rate)]
            }
            _ => self.limit_args_at(rate),
        }
    }

    /// The ffmpeg input options for one pass.
    pub fn input_args(self, start: f64) -> Vec<String> {
        let mut a = Vec::new();
        if matches!(self, Playback::WholeFile) {
            a.extend(["-stream_loop".to_string(), "-1".into()]);
        }
        if start > 0.0 {
            a.extend(["-ss".to_string(), format!("{start:.3}")]);
        }
        a
    }

    /// The ffmpeg option placed after `-i` to end a pass, if any.
    #[cfg(test)]
    pub fn limit_args(self) -> Vec<String> {
        self.limit_args_at(1.0)
    }

    /// `limit_args` for a cue played at `rate` times its speed.
    pub fn limit_args_at(self, rate: f64) -> Vec<String> {
        let d = match self {
            Playback::Once { duration } => duration,
            Playback::Slice { len, .. } | Playback::SliceFrom { len, .. } => Some(len),
            Playback::WholeFile => None,
        };
        d.map(|d| vec!["-t".to_string(), format!("{:.3}", d / rate)])
            .unwrap_or_default()
    }
}

/// FFmpeg's `atempo` chain for speed `rate` (each stage takes 0.5 to 2), which
/// changes the speed without changing the pitch.
pub fn atempo_filter(rate: f64) -> String {
    let mut stages = Vec::new();
    let mut r = rate.clamp(0.1, 16.0);
    while r > 2.0 {
        stages.push("atempo=2.0".to_string());
        r /= 2.0;
    }
    while r < 0.5 {
        stages.push("atempo=0.5".to_string());
        r *= 2.0;
    }
    stages.push(format!("atempo={r:.6}"));
    stages.join(",")
}

struct State {
    closed: bool,
    next_id: u64,
    live: Vec<(u64, Child)>,
}

/// The ffmpeg processes behind one player. Shared with its reader thread,
/// which starts and retires them as passes come and go; `close` (from the
/// player's `Drop`) kills whatever is left and refuses new ones.
#[derive(Clone)]
pub struct Children(Arc<Mutex<State>>);

impl Children {
    pub fn new() -> Children {
        Children(Arc::new(Mutex::new(State {
            closed: false,
            next_id: 0,
            live: Vec::new(),
        })))
    }

    fn spawn(
        &self,
        cmd: &mut Command,
        error: &Arc<Mutex<Option<String>>>,
    ) -> Result<(u64, ChildStdout)> {
        let mut st = self.0.lock();
        anyhow::ensure!(!st.closed, "player closed");
        let mut child = cmd.spawn().context("starting ffmpeg")?;
        let stdout = child.stdout.take().context("ffmpeg stdout")?;
        super::video::spawn_stderr_drain(child.stderr.take(), error.clone());
        let id = st.next_id;
        st.next_id += 1;
        st.live.push((id, child));
        Ok((id, stdout))
    }

    fn release(&self, id: u64) {
        let child = {
            let mut st = self.0.lock();
            st.live
                .iter()
                .position(|(i, _)| *i == id)
                .map(|p| st.live.remove(p).1)
        };
        if let Some(c) = child {
            reap(c);
        }
    }

    pub fn close(&self) {
        let live = {
            let mut st = self.0.lock();
            st.closed = true;
            std::mem::take(&mut st.live)
        };
        for (_, c) in live {
            reap(c);
        }
    }
}

/// Kill an ffmpeg and see it off on a thread of its own: waiting for the
/// process to be gone can take a while (the pipe to drain, the OS to tidy up),
/// and this is called from the main thread by Stop All and when a cue ends.
fn reap(mut child: Child) {
    let _ = child.kill();
    let _ = std::thread::Builder::new()
        .name("ffmpeg-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// Supplies a reader thread with the decoder output of each pass in turn.
pub struct PassFeed {
    children: Children,
    make: Box<dyn Fn() -> Command + Send>,
    error: Arc<Mutex<Option<String>>>,
    passes: Option<u32>,
    pass: u32,
    current: Option<(u64, ChildStdout)>,
    next: Option<(u64, ChildStdout)>,
}

impl PassFeed {
    /// Start the first pass. `make` builds the ffmpeg command for one pass.
    pub fn start(
        children: &Children,
        passes: Option<u32>,
        error: &Arc<Mutex<Option<String>>>,
        make: impl Fn() -> Command + Send + 'static,
    ) -> Result<PassFeed> {
        let first = children.spawn(&mut make(), error)?;
        Ok(PassFeed {
            children: children.clone(),
            make: Box::new(make),
            error: error.clone(),
            passes,
            pass: 0,
            current: Some(first),
            next: None,
        })
    }

    pub fn stdout(&mut self) -> &mut ChildStdout {
        &mut self.current.as_mut().expect("no current pass").1
    }

    fn has_more(&self) -> bool {
        self.passes.is_none_or(|n| self.pass + 1 < n)
    }

    /// Start the next pass's ffmpeg now, so `advance` has nothing to wait for.
    pub fn prefetch(&mut self) {
        if self.next.is_none() && self.has_more() {
            self.next = self.children.spawn(&mut (self.make)(), &self.error).ok();
        }
    }

    /// Retire the current pass and move to the next. False when there is none.
    pub fn advance(&mut self) -> bool {
        if let Some((id, _)) = self.current.take() {
            self.children.release(id);
        }
        if !self.has_more() {
            return false;
        }
        self.pass += 1;
        self.current = self
            .next
            .take()
            .or_else(|| self.children.spawn(&mut (self.make)(), &self.error).ok());
        self.current.is_some()
    }
}

impl Drop for PassFeed {
    fn drop(&mut self) {
        for (id, _) in self.current.take().into_iter().chain(self.next.take()) {
            self.children.release(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_playback() {
        assert_eq!(
            Playback::plan(2.0, Some(5.0), Some(10.0), false, None),
            Playback::Once {
                duration: Some(3.0)
            }
        );
        assert_eq!(
            Playback::plan(0.0, None, Some(10.0), false, None),
            Playback::Once { duration: None }
        );
        assert_eq!(
            Playback::plan(2.0, Some(5.0), Some(10.0), true, None),
            Playback::Slice {
                len: 3.0,
                passes: None
            }
        );
        // No out point: loop from the in point to the end of the file.
        assert_eq!(
            Playback::plan(2.0, None, Some(10.0), true, Some(3)),
            Playback::Slice {
                len: 8.0,
                passes: Some(3)
            }
        );
        // Unknown length, or a sliver: fall back to looping the file.
        assert_eq!(
            Playback::plan(0.0, None, None, true, None),
            Playback::WholeFile
        );
        assert_eq!(
            Playback::plan(1.0, Some(1.05), Some(10.0), true, None),
            Playback::WholeFile
        );
    }

    #[test]
    fn totals_and_passes() {
        let s = Playback::Slice {
            len: 3.0,
            passes: Some(4),
        };
        assert_eq!(s.total(Some(99.0)), Some(12.0));
        assert_eq!(s.passes(), Some(4));
        assert_eq!(s.pass_len(), Some(3.0));
        let forever = Playback::Slice {
            len: 3.0,
            passes: None,
        };
        assert_eq!(forever.total(None), None);
        assert_eq!(forever.passes(), None);
        let once = Playback::Once { duration: None };
        assert_eq!(once.total(Some(10.0)), Some(10.0));
        assert_eq!(once.passes(), Some(1));
        assert_eq!(once.pass_len(), None);
        assert_eq!(Playback::WholeFile.total(Some(10.0)), None);
    }

    #[test]
    fn builds_ffmpeg_args() {
        let s = Playback::Slice {
            len: 3.0,
            passes: None,
        };
        assert_eq!(s.input_args(2.0), ["-ss", "2.000"]);
        assert_eq!(s.limit_args(), ["-t", "3.000"]);
        assert_eq!(Playback::WholeFile.input_args(0.0), ["-stream_loop", "-1"]);
        assert!(Playback::WholeFile.limit_args().is_empty());
        assert!(Playback::Once { duration: None }.limit_args().is_empty());
    }

    #[test]
    fn a_jump_in_a_loop_starts_part_way_through_a_pass() {
        let s = Playback::Slice {
            len: 4.0,
            passes: None,
        };
        // 1.5 s into the slice: the first pass is the last 2.5 s of it, then whole ones.
        let j = s.started_at(1.5);
        assert_eq!(
            j,
            Playback::SliceFrom {
                len: 4.0,
                passes: None,
                skip: 1.5
            }
        );
        assert_eq!(j.first_pass_len(), Some(2.5));
        assert_eq!(j.pass_len(), Some(4.0));
        assert_eq!(j.first_input_args(2.0), ["-ss", "3.500"]);
        assert_eq!(j.first_limit_args(), ["-t", "2.500"]);
        assert_eq!(j.input_args(2.0), ["-ss", "2.000"]);
        assert_eq!(j.limit_args(), ["-t", "4.000"]);
        // A set number of plays: 9 s in of three 4 s plays is 1 s into the
        // third, which is the only one left, and 3 s of it remain.
        let three = Playback::Slice {
            len: 4.0,
            passes: Some(3),
        };
        let j = three.started_at(9.0);
        assert_eq!(
            j,
            Playback::SliceFrom {
                len: 4.0,
                passes: Some(1),
                skip: 1.0
            }
        );
        assert_eq!(j.total(None), Some(3.0));
        // Not a loop: unchanged. A plain pass: its own first length.
        let once = Playback::Once { duration: None };
        assert_eq!(once.started_at(3.0), once);
        assert_eq!(s.first_pass_len(), Some(4.0));
        assert_eq!(s.first_input_args(2.0), s.input_args(2.0));
    }

    #[test]
    fn speed_changes_the_limits_and_chains_atempo() {
        let s = Playback::Slice {
            len: 4.0,
            passes: None,
        };
        // 4 s of file at double speed is 2 s of output.
        assert_eq!(s.limit_args_at(2.0), ["-t", "2.000"]);
        assert_eq!(s.limit_args_at(1.0), s.limit_args());
        let j = s.started_at(1.0);
        assert_eq!(j.first_limit_args_at(0.5), ["-t", "6.000"]);
        assert_eq!(
            Playback::Once {
                duration: Some(3.0)
            }
            .limit_args_at(2.0),
            ["-t", "1.500"]
        );
        // Each stage takes 0.5 to 2: more is chained.
        assert_eq!(atempo_filter(1.5), "atempo=1.500000");
        assert_eq!(atempo_filter(4.0), "atempo=2.0,atempo=2.000000");
        assert_eq!(atempo_filter(0.25), "atempo=0.5,atempo=0.500000");
        assert_eq!(atempo_filter(0.7), "atempo=0.700000");
    }
}
