//! Several NewQs on one computer, for the network test end to end (tests only;
//! `scripts/net_demo.sh` does the same by hand for developers). Each is NewQ's
//! own program started as a child process with a folder of its own
//! (`NEWQ_NODE_DIR` for its identity, `XDG_STATE_HOME` for its log, test media
//! and media store), its identity written beforehand: one primary and display
//! computers already in its network, so nothing has to be clicked. The primary
//! is told by `NEWQ_NET_TEST_AUTOSTART` to start the network test once that
//! many computers are in step (with `NEWQ_E2E_PARTS`, say `cue,network`, as
//! `NEWQ_TEST_PARTS`: more than the network, one after another); the test then
//! makes its zip as for anyone.
//! Needs a display (Xvfb): each NewQ can have its own (`NEWQ_E2E_DISPLAYS`).

use crate::net::node::{self, Identity, NetworkId, NodeId, Role};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One NewQ started for the demo.
pub struct Started {
    pub name: String,
    pub dir: PathBuf,
    child: Child,
}

/// The NewQs; ended (and waited for) when dropped.
pub struct Demo {
    pub computers: Vec<Started>,
}

impl Drop for Demo {
    fn drop(&mut self) {
        for c in &mut self.computers {
            let _ = c.child.kill();
            let _ = c.child.wait();
        }
    }
}

/// NewQ's program: `NEWQ_E2E_PROGRAM`, else beside this test program (`target/debug/newq`).
pub fn newq_program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NEWQ_E2E_PROGRAM") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let p = dir.join(if cfg!(windows) { "newq.exe" } else { "newq" });
    p.is_file().then_some(p)
}

/// Write the identities: `names[0]` the primary, the rest display computers
/// of its network.
pub fn identities(base: &Path, names: &[&str]) -> std::io::Result<Vec<Identity>> {
    let network = NetworkId(node::random_u64() | 1);
    let primary = NodeId(node::random_u64() | 1);
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let id = if i == 0 {
            Identity {
                id: primary,
                name: Some((*name).into()),
                role: Role::Primary,
                on: true,
                network: Some(network),
                primary: Some(primary),
                term: 1,
                ..Identity::fresh()
            }
        } else {
            Identity {
                name: Some((*name).into()),
                role: Role::Display,
                on: true,
                network: Some(network),
                primary: Some(primary),
                primary_name: Some(names[0].into()),
                term: 1,
                ..Identity::fresh()
            }
        };
        node::save(&base.join(name).join("node"), &id)?;
        out.push(id);
    }
    Ok(out)
}

/// Start the NewQs: the primary with `show`, on display `displays[i]` each
/// (or all on `DISPLAY`), the ports from `port`. `extra` is added to the
/// primary's environment.
pub fn start(
    program: &Path,
    base: &Path,
    names: &[&str],
    show: &Path,
    displays: &[String],
    port: u16,
    extra: &[(&str, String)],
) -> std::io::Result<Demo> {
    let mut computers = Vec::new();
    // The display computers first: they wait for their primary.
    for (i, name) in names.iter().enumerate().rev() {
        let dir = base.join(name);
        std::fs::create_dir_all(dir.join("state"))?;
        let mut cmd = Command::new(program);
        if i == 0 {
            cmd.arg(show);
            for (k, v) in extra {
                cmd.env(k, v);
            }
        }
        if let Some(d) = displays.get(i) {
            cmd.env("DISPLAY", d);
        }
        cmd.env("NEWQ_NODE_DIR", dir.join("node"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("NEWQ_NET_PORT", port.to_string())
            .env("NEWQ_NO_SPLASH", "1")
            .env("NEWQ_MUTE", "1")
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(dir.join("out.txt"))?)
            .stderr(std::fs::File::create(dir.join("err.txt"))?);
        let child = cmd.spawn()?;
        crate::reaper::watch(&child);
        computers.push(Started {
            name: (*name).into(),
            dir,
            child,
        });
    }
    computers.reverse();
    Ok(Demo { computers })
}

impl Demo {
    /// The primary's log folder.
    pub fn log_dir(&self, i: usize) -> PathBuf {
        self.computers[i].dir.join("state").join("newq")
    }

    /// Wait for the primary's test zip (None: not within `limit`, or a NewQ
    /// ended).
    pub fn wait_for_zip(&mut self, limit: Duration) -> Option<PathBuf> {
        let runs = self.log_dir(0).join("test-mode").join("runs");
        let end = Instant::now() + limit;
        while Instant::now() < end {
            for c in &mut self.computers {
                if let Ok(Some(status)) = c.child.try_wait() {
                    eprintln!("{} ended: {status}", c.name);
                    return None;
                }
            }
            let zip = std::fs::read_dir(&runs)
                .into_iter()
                .flatten()
                .flatten()
                .find(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with("NewQ-test-") && n.ends_with(".zip")
                });
            if let Some(z) = zip {
                // Written whole once the folder beside it has gone.
                let folder = z
                    .path()
                    .to_string_lossy()
                    .trim_end_matches(".zip")
                    .replace("NewQ-test-", "");
                if !Path::new(&folder).exists() {
                    return Some(z.path());
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        None
    }
}

/// A file of the zip, as text.
pub fn read_from_zip(zip: &Path, name: &str) -> Option<String> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip).ok()?).ok()?;
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

/// A show for the primary: one display and nothing else (the test adds a
/// display for each display computer's monitor).
pub fn write_show(path: &Path) -> std::io::Result<()> {
    let show = crate::show::Show::default();
    std::fs::write(
        path,
        serde_json::to_string_pretty(&show).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End to end: a primary and two display computers, each NewQ's own
    /// program, run the network test by themselves. Needs `cargo build` first
    /// and a display (`xvfb-run -a cargo test ... -- --ignored --nocapture`, or
    /// `NEWQ_E2E_DISPLAYS=:91,:92,:93` with an Xvfb on each). The numbers are
    /// loopback numbers: they say the machinery works, not how a real network does.
    #[test]
    #[ignore]
    fn three_newqs_run_the_network_test_by_themselves() {
        let program = newq_program().expect("build NewQ first: cargo build");
        let displays: Vec<String> = std::env::var("NEWQ_E2E_DISPLAYS")
            .ok()
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_default();
        assert!(
            !displays.is_empty() || std::env::var_os("DISPLAY").is_some(),
            "needs a display: run under xvfb-run, or set NEWQ_E2E_DISPLAYS"
        );
        let base = std::env::var_os("NEWQ_E2E_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("newq_e2e_{}", std::process::id()))
            });
        let _ = std::fs::remove_dir_all(&base);
        let names = ["Booth", "Left", "Right"];
        identities(&base, &names).unwrap();
        // The test media, made once and kept between runs.
        let cache = std::env::temp_dir().join("newq_e2e_media");
        std::fs::create_dir_all(&cache).unwrap();
        let tm = base
            .join("Booth")
            .join("state")
            .join("newq")
            .join("test-mode");
        std::fs::create_dir_all(&tm).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&cache, tm.join("media")).unwrap();
        let show = base.join("show.newq");
        write_show(&show).unwrap();
        let port = 48100 + (std::process::id() % 100) as u16 * 8;
        let mut demo = start(
            &program,
            &base,
            &names,
            &show,
            &displays,
            port,
            &[
                ("NEWQ_NET_TEST_AUTOSTART", "2".into()),
                (
                    "NEWQ_TEST_PARTS",
                    std::env::var("NEWQ_E2E_PARTS").unwrap_or_else(|_| "network".into()),
                ),
            ],
        )
        .unwrap();
        let zip = demo
            .wait_for_zip(Duration::from_secs(20 * 60))
            .unwrap_or_else(|| panic!("no zip; see the logs in {}", base.display()));
        let report = read_from_zip(&zip, "report.txt").expect("a report in the zip");
        println!("{report}");
        let csv = read_from_zip(&zip, "network.csv").expect("network.csv in the zip");
        assert!(
            report.contains("Left — ") && report.contains("Right — "),
            "{report}"
        );
        assert!(
            csv.lines().filter(|l| l.starts_with("Left,")).count() > 50,
            "{csv}"
        );
        assert!(csv.lines().filter(|l| l.starts_with("Right,")).count() > 50);
        // (Run with other parts, a name two parts wrote has the part's in front.)
        assert!(
            read_from_zip(&zip, "computer-Left-newq.log")
                .or_else(|| read_from_zip(&zip, "network-computer-Left-newq.log"))
                .is_some()
        );
        assert!(read_from_zip(&zip, "events.log").is_some_and(|e| e.contains("All here")));
        println!("zip: {}", zip.display());
    }
}
