//! No program NewQ started outlives it, however NewQ ends.
//!
//! A decoder writing to NewQ notices the pipe close and stops, but one that
//! is waiting (a Mac capture that never got its permission, a stream's
//! encoder, the streaming server) does not: after a crash or a forced quit
//! they stayed, holding a camera or a processor. Linux can ask the kernel to
//! end a child with its parent; a Mac can't. So NewQ starts a copy of itself
//! as a watcher (`newq --reap`), tells it each program it starts, and holds
//! the writing end of a pipe to it. The watcher notes a program only if it
//! finds it running as a child of that NewQ and of a kind NewQ starts, with
//! the moment it began; it ends only programs it noted, and only while the
//! number still belongs to the one that began then. An FFmpeg anyone else
//! started is never touched. That end closes when NewQ is gone, in
//! any way at all, and the watcher then ends what is left.
//!
//! Windows does this with job objects where it matters (`mediamtx::tie_to_us`).

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

/// The option that makes this program the watcher.
const FLAG: &str = "--reap";

/// The names of the programs NewQ starts: nothing else is ever ended, so a
/// number the system has since given to another program is left alone.
const OURS: [&str; 4] = ["ffmpeg", "ffprobe", "mediamtx", "newq"];

static WATCHER: OnceLock<Option<parking_lot::Mutex<std::process::ChildStdin>>> = OnceLock::new();

/// Start the watcher (once; the app does it as it starts). Without one,
/// `watch` does nothing and NewQ runs as it did before.
pub fn start() {
    WATCHER.get_or_init(|| {
        if !cfg!(unix) {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let mut child = Command::new(exe)
            .args([FLAG, &std::process::id().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| log::warn!("no watcher for the programs NewQ starts: {e}"))
            .ok()?;
        child.stdin.take().map(parking_lot::Mutex::new)
    });
}

/// Tell the watcher about a program just started.
pub fn watch(child: &Child) {
    if let Some(Some(pipe)) = WATCHER.get() {
        // A watcher that has gone is not worth a word: the show goes on.
        let _ = writeln!(pipe.lock(), "{}", child.id());
    }
}

/// A program NewQ started, as the watcher saw it when it was told: its
/// number and the moment it began. The two together are one program and no
/// other, so a number the system later gives to something else is not it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
    pid: u32,
    started: u64,
}

fn processes(pids: &[u32]) -> sysinfo::System {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pids: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        true,
        ProcessRefreshKind::nothing(),
    );
    system
}

/// Take note of `pid`, if it is what NewQ says it is: a program running now,
/// started by the NewQ this watcher serves (`parent`), and one of the kinds
/// NewQ starts. Anything else is never noted, so never ended.
fn note(pid: u32, parent: Option<u32>) -> Option<Seen> {
    let system = processes(&[pid]);
    let p = system.process(sysinfo::Pid::from_u32(pid))?;
    let by_newq = parent.is_some() && p.parent().map(sysinfo::Pid::as_u32) == parent;
    let name = p.name().to_string_lossy().to_lowercase();
    (by_newq && is_ours(&name)).then(|| Seen {
        pid,
        started: p.start_time(),
    })
}

/// `newq --reap <pid of NewQ>`: note each program NewQ says it started until
/// the pipe closes (NewQ has ended), then end those still running.
pub fn run_if_asked(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some(FLAG) {
        return None;
    }
    let parent: Option<u32> = args.get(1).and_then(|p| p.parse().ok());
    let mut seen = Vec::new();
    for line in std::io::stdin().lines() {
        let Ok(line) = line else { break };
        if let Ok(pid) = line.trim().parse::<u32>()
            && let Some(s) = note(pid, parent)
        {
            seen.push(s);
        }
    }
    reap(&seen);
    Some(0)
}

/// End each of `seen` that is still running: the very program that was
/// noted (same number, same start), still one of NewQ's kinds. Returns how
/// many were ended.
fn reap(seen: &[Seen]) -> usize {
    let pids: Vec<u32> = seen.iter().map(|s| s.pid).collect();
    let system = processes(&pids);
    let mut ended = 0;
    for s in seen {
        let Some(p) = system.process(sysinfo::Pid::from_u32(s.pid)) else {
            continue;
        };
        let name = p.name().to_string_lossy().to_lowercase();
        if p.start_time() == s.started && is_ours(&name) && p.kill() {
            ended += 1;
        }
    }
    ended
}

/// Whether a program of this name is one NewQ starts.
fn is_ours(name: &str) -> bool {
    OURS.iter().any(|o| name.starts_with(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_watcher_option_makes_this_the_watcher() {
        assert_eq!(run_if_asked(&[]), None);
        assert_eq!(run_if_asked(&["show.newq".into()]), None);
        assert_eq!(run_if_asked(&["--self-test".into()]), None);
    }

    #[test]
    fn only_newqs_own_programs_are_ever_ended() {
        for ours in ["ffmpeg", "ffprobe", "mediamtx", "newq", "ffmpeg.exe"] {
            assert!(is_ours(ours), "{ours}");
        }
        for other in ["sleep", "bash", "launchd", "", "systemd", "finder"] {
            assert!(!is_ours(other), "{other}");
        }
        // A program that is not one of NewQ's kinds is never noted, even
        // when NewQ's own number is given as its parent.
        let me = Some(std::process::id());
        let mut other = Command::new("sleep").arg("30").spawn().expect("sleep");
        assert_eq!(note(other.id(), me), None);
        assert!(other.try_wait().unwrap().is_none(), "it was left running");
        // Nor is a number nothing has.
        assert_eq!(note(u32::MAX - 7, me), None);
        let _ = other.kill();
        let _ = other.wait();
    }

    /// An FFmpeg left by a NewQ that has gone is ended. (The watcher itself
    /// is this same code after its pipe closes; a test can't start the app's
    /// own program, so the ending is called directly.)
    #[test]
    fn an_ffmpeg_left_by_a_newq_that_is_gone_is_ended() {
        let Some((ff, _)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        // An FFmpeg that would run for a minute, writing nowhere.
        let mut cmd = ff.command();
        cmd.args(["-re", "-f", "lavfi", "-i", "testsrc2=size=64x64:rate=5"]);
        cmd.args(["-t", "60", "-f", "null", "-"]);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let mut ffmpeg = cmd.spawn().expect("ffmpeg starts");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(ffmpeg.try_wait().unwrap().is_none(), "it is running");
        let me = Some(std::process::id());
        // An FFmpeg that some other program started (not the NewQ this
        // watcher serves) is never noted, so it can't be ended.
        assert_eq!(note(ffmpeg.id(), Some(1_999_999_999)), None);
        assert_eq!(note(ffmpeg.id(), None), None);
        // The one NewQ started is noted.
        let seen = note(ffmpeg.id(), me).expect("NewQ's own FFmpeg is noted");
        // If its number had passed to a program begun at another moment,
        // that program is left alone.
        let stale = Seen {
            started: seen.started + 100,
            ..seen
        };
        assert_eq!(reap(&[stale]), 0);
        assert!(ffmpeg.try_wait().unwrap().is_none(), "it is still running");
        // The very one that was noted is ended.
        assert_eq!(reap(&[seen]), 1);
        let began = std::time::Instant::now();
        while ffmpeg.try_wait().unwrap().is_none() {
            assert!(
                began.elapsed() < std::time::Duration::from_secs(5),
                "the FFmpeg was left running"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
