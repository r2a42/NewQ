//! Running MediaMTX (the streaming server) inside NewQ, so a display can be
//! sent out and watched on phones and other computers with nothing else to
//! install. NewQ ships the `mediamtx` program next to itself, the way it ships
//! FFmpeg, runs it as a child process with a configuration it writes, and
//! stops it when it is no longer wanted or NewQ quits.
//!
//! The server runs on a worker thread of its own (`Runner`): looking for a
//! server left behind and starting a program are slow on some computers, and
//! the main loop must not wait for them.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::show::{ServerProtocol, StreamKind};

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "mediamtx.exe"
    } else {
        "mediamtx"
    }
}

/// Where the program is: `$NEWQ_MEDIAMTX_DIR`, next to NewQ's own executable
/// (and its `mediamtx/` folder), `third_party/mediamtx` while developing, then
/// `PATH`.
pub fn locate() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("NEWQ_MEDIAMTX_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("mediamtx"));
        dirs.push(exe_dir.clone());
        if let Some(root) = exe_dir.parent().and_then(Path::parent) {
            dirs.push(root.join("third_party").join("mediamtx"));
        }
    }
    dirs.push(PathBuf::from("third_party").join("mediamtx"));
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter()
        .map(|d| d.join(exe_name()))
        .find(|p| p.is_file())
}

/// What the hosted server is asked to do: which address to listen on and the
/// port of each way in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hosting {
    /// The adapter's address to listen on; `None` listens on every adapter.
    pub listen: Option<IpAddr>,
    pub rtsp: u16,
    pub rtmp: u16,
    pub srt: u16,
    /// The web player (WebRTC).
    pub player: u16,
    /// The same picture as HLS, for browsers that can't do WebRTC.
    pub hls: u16,
    /// Where WebRTC's media travels (UDP, and TCP as a fallback).
    pub ice: u16,
    /// The UDP port RTSP viewers can receive media on (RTCP uses the next one).
    /// 0 turns UDP off: RTSP then runs over TCP only.
    pub rtp: u16,
}

impl Hosting {
    /// MediaMTX's own usual ports, on `listen`.
    pub fn standard(listen: Option<IpAddr>) -> Hosting {
        Hosting {
            listen,
            rtsp: 8554,
            rtmp: 1935,
            srt: 8890,
            player: crate::show::default_player_port(),
            hls: 8888,
            ice: 8189,
            rtp: 8000,
        }
    }

    /// The port this server takes `protocol` on.
    pub fn port_of(&self, protocol: ServerProtocol) -> u16 {
        match protocol {
            ServerProtocol::Rtsp => self.rtsp,
            ServerProtocol::Rtmp => self.rtmp,
            ServerProtocol::Srt => self.srt,
        }
    }

    /// Two ways in set to the same port can't both listen: say which, rather
    /// than let the program fail on it and be started again and again.
    pub fn conflict(&self) -> Option<String> {
        let mut tcp = vec![
            ("RTSP", self.rtsp),
            ("RTMP", self.rtmp),
            ("the web player", self.player),
            ("HLS", self.hls),
            ("WebRTC", self.ice),
        ];
        let mut udp = vec![("SRT", self.srt), ("WebRTC", self.ice)];
        if self.rtp != 0 {
            udp.push(("RTP", self.rtp));
            udp.push(("RTCP", self.rtp.saturating_add(1)));
        }
        for list in [&mut tcp, &mut udp] {
            for (i, (a, pa)) in list.iter().enumerate() {
                if let Some((b, _)) = list[i + 1..].iter().find(|(_, pb)| pb == pa) {
                    return Some(format!(
                        "{a} and {b} are both set to port {pa}: give them different ports"
                    ));
                }
            }
        }
        None
    }

    /// The configuration file's text. Only what NewQ uses is on: no control
    /// API, metrics or recording. Anyone on the network may watch, but only
    /// this computer (NewQ publishes from here) may send a picture, and a
    /// second sender can't take over the stream from the first.
    pub fn config(&self) -> String {
        let at = |port: u16| match self.listen {
            Some(IpAddr::V6(ip)) => format!("\"[{ip}]:{port}\""),
            Some(ip) => format!("\"{ip}:{port}\""),
            None => format!("\":{port}\""),
        };
        // WebRTC offers its own addresses to the browser: with an adapter
        // chosen, offer only that one.
        let webrtc_hosts = match self.listen {
            Some(ip) => format!("webrtcIPsFromInterfaces: no\nwebrtcAdditionalHosts: [\"{ip}\"]\n"),
            None => String::new(),
        };
        let rtsp_transports = if self.rtp == 0 {
            "rtspTransports: [tcp]\n".to_string()
        } else {
            format!(
                "rtspTransports: [udp, tcp]\nrtpAddress: {}\nrtcpAddress: {}\n",
                at(self.rtp),
                at(self.rtp.saturating_add(1))
            )
        };
        // Connecting to the computer's own adapter address comes from that
        // same address, so it is allowed to send too.
        let mut senders = vec!["\"127.0.0.1\"".to_string(), "\"::1\"".to_string()];
        if let Some(ip) = self.listen {
            senders.push(format!("\"{ip}\""));
        }
        let senders = senders.join(", ");
        format!(
            "# Written by NewQ each time it starts the server: changes here are lost.\n\
             logLevel: info\n\
             api: no\n\
             metrics: no\n\
             pprof: no\n\
             playback: no\n\
             moq: no\n\
             authInternalUsers:\n\
             \x20 - user: any\n\
             \x20   pass:\n\
             \x20   ips: [{senders}]\n\
             \x20   permissions:\n\
             \x20     - action: publish\n\
             \x20 - user: any\n\
             \x20   pass:\n\
             \x20   ips: []\n\
             \x20   permissions:\n\
             \x20     - action: read\n\
             {rtsp_transports}\
             rtspAddress: {rtsp}\n\
             rtspsAddress: \"\"\n\
             rtmpAddress: {rtmp}\n\
             rtmpsAddress: \"\"\n\
             srtAddress: {srt}\n\
             hlsAddress: {hls}\n\
             webrtcAddress: {player}\n\
             webrtcLocalUDPAddress: {udp}\n\
             webrtcLocalTCPAddress: {udp}\n\
             {webrtc_hosts}\
             pathDefaults:\n\
             \x20 overridePublisher: no\n\
             paths:\n  all_others:\n",
            rtsp = at(self.rtsp),
            rtmp = at(self.rtmp),
            srt = at(self.srt),
            hls = at(self.hls),
            player = at(self.player),
            udp = at(self.ice),
        )
    }
}

/// What the streams that run their own server ask of it: the settings of the
/// first one for each thing (one server serves them all), and the adapter's
/// name as it was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub adapter: String,
    pub hosting: Hosting,
}

/// The server wanted by `kinds` (every stream that is on), if any asks to run
/// here. `None` for an adapter that isn't there; the caller says so.
pub fn plan<'a>(
    kinds: impl Iterator<Item = &'a StreamKind>,
    adapters: &[crate::streamlink::Adapter],
) -> Option<Plan> {
    let mut plan: Option<Plan> = None;
    let mut seen: Vec<ServerProtocol> = Vec::new();
    for kind in kinds {
        let StreamKind::Server {
            protocol,
            port,
            adapter,
            player_port,
            host_here: true,
            ..
        } = kind
        else {
            continue;
        };
        let p = plan.get_or_insert_with(|| Plan {
            adapter: adapter.trim().to_string(),
            hosting: Hosting::standard(crate::streamlink::adapter_ip(adapter, adapters)),
        });
        if seen.is_empty() && *player_port != 0 {
            p.hosting.player = *player_port;
        }
        if !seen.contains(protocol) {
            seen.push(*protocol);
            let port = if *port == 0 {
                protocol.default_port()
            } else {
                *port
            };
            match protocol {
                ServerProtocol::Rtsp => p.hosting.rtsp = port,
                ServerProtocol::Rtmp => p.hosting.rtmp = port,
                ServerProtocol::Srt => p.hosting.srt = port,
            }
        }
    }
    plan
}

/// Make a stream that runs its server here use the server as it is: one
/// server, one adapter and one set of ports, whatever each display says.
pub fn align(kind: &mut StreamKind, plan: &Plan) {
    if let StreamKind::Server {
        protocol,
        port,
        adapter,
        player_port,
        host_here: true,
        ..
    } = kind
    {
        *adapter = plan.adapter.clone();
        *port = plan.hosting.port_of(*protocol);
        *player_port = plan.hosting.player;
    }
}

/// How the server NewQ runs is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// No stream asks for it.
    #[default]
    Off,
    Running(Hosting),
    /// The program isn't with this copy of NewQ.
    Missing,
    /// This computer hasn't been allowed to run a server for a show.
    NotAllowed,
    Failed(String),
}

#[cfg(windows)]
pub(crate) struct JobGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for JobGuard {}

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        // Closing the job ends everything in it.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Put `child` in a job that is ended when NewQ's handle on it closes, which
/// happens when NewQ exits however it does, so a crash leaves no server.
#[cfg(windows)]
pub(crate) fn tie_to_us(child: &Child) -> Option<JobGuard> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let guard = JobGuard(job);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
            return None;
        }
        Some(guard)
    }
}

/// The server NewQ started. Dropping it stops the program.
pub struct Hosted {
    child: Child,
    /// The last lines it wrote, for saying why it stopped.
    tail: Arc<Mutex<VecDeque<String>>>,
    pidfile: PathBuf,
    #[cfg(windows)]
    _job: Option<JobGuard>,
}

/// How the started server is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    /// It stopped by itself: the end of what it said (a port in use shows here).
    Stopped(String),
}

fn process_is_running(sys: &sysinfo::System, pid: u32) -> bool {
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}

/// Stop servers that earlier NewQs left running when they were killed or
/// crashed: they would hold the ports. Each run keeps a folder named for its
/// own process under `root` with a `server.pid` (`<NewQ's pid> <server's pid>`);
/// a server is stopped when its NewQ is gone (or is this one, which has one
/// server at a time) and it is still the MediaMTX program. A server of a NewQ
/// that is still running, or anyone's own MediaMTX, is left alone.
fn stop_leftovers(root: &Path, exe: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    );
    let me = std::process::id();
    let exe = exe.canonicalize().ok();
    let mut stopped = false;
    for e in entries.flatten() {
        let dir = e.path();
        let Ok(text) = std::fs::read_to_string(dir.join("server.pid")) else {
            continue;
        };
        let mut nums = text
            .split_whitespace()
            .filter_map(|n| n.parse::<u32>().ok());
        let (Some(owner), Some(server)) = (nums.next(), nums.next()) else {
            continue;
        };
        if owner != me && process_is_running(&sys, owner) {
            continue;
        }
        if let Some(p) = sys.process(sysinfo::Pid::from_u32(server)) {
            let same = p.exe().and_then(|x| x.canonicalize().ok()) == exe && exe.is_some();
            if same && p.kill() {
                log::info!("stopped a MediaMTX left running by an earlier NewQ (pid {server})");
                stopped = true;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    if stopped {
        // Let the system free its ports.
        std::thread::sleep(Duration::from_millis(300));
    }
}

impl Hosted {
    /// Start the program, with its configuration in a folder of this run under
    /// `root` and what it says added to `root/mediamtx.log`.
    pub fn start(exe: &Path, root: &Path, hosting: Hosting) -> Result<Hosted, String> {
        if let Some(why) = hosting.conflict() {
            return Err(why);
        }
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        stop_leftovers(root, exe);
        let dir = root.join(std::process::id().to_string());
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let conf = dir.join("mediamtx.yml");
        std::fs::write(&conf, hosting.config()).map_err(|e| format!("{}: {e}", conf.display()))?;
        // What the server says is kept in a file, for finding out why a
        // viewer sees nothing: added to, and started afresh when it is big.
        let log_path = root.join("mediamtx.log");
        let big = std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 1_000_000);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(!big)
            .write(true)
            .truncate(big)
            .open(&log_path)
            .ok();
        if let Some(f) = &log {
            let _ = writeln!(&*f, "--- NewQ started the server");
        }
        let mut cmd = Command::new(exe);
        cmd.arg(&conf)
            // It leaves a certificate it makes in its working folder.
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::media::ffmpeg::no_console(&mut cmd);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // Ends with the thread that started it, which lives as long as NewQ
            // does: a crash leaves no server behind.
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    Ok(())
                });
            }
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("can't start MediaMTX: {e}"))?;
        #[cfg(windows)]
        let job = tie_to_us(&child);
        let pidfile = dir.join("server.pid");
        let _ = std::fs::write(&pidfile, format!("{} {}", std::process::id(), child.id()));
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let pipes: [Option<Box<dyn std::io::Read + Send>>; 2] = [
            child.stdout.take().map(|p| Box::new(p) as _),
            child.stderr.take().map(|p| Box::new(p) as _),
        ];
        for pipe in pipes.into_iter().flatten() {
            let tail = tail.clone();
            let log = log.as_ref().and_then(|f| f.try_clone().ok());
            std::thread::Builder::new()
                .name("mediamtx-log".into())
                .spawn(move || {
                    // Read as bytes: a line that isn't text must not end the
                    // reading, or the program would block on a full pipe.
                    let mut reader = BufReader::new(pipe);
                    let mut buf = Vec::new();
                    loop {
                        buf.clear();
                        match reader.read_until(b'\n', &mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                        if let Some(f) = &log {
                            let _ = writeln!(&*f, "{line}");
                        }
                        let mut t = tail.lock();
                        if t.len() >= 12 {
                            t.pop_front();
                        }
                        t.push_back(line);
                    }
                })
                .ok();
        }
        Ok(Hosted {
            child,
            tail,
            pidfile,
            #[cfg(windows)]
            _job: job,
        })
    }

    pub fn state(&mut self) -> State {
        match self.child.try_wait() {
            Ok(None) => State::Running,
            Ok(Some(status)) => State::Stopped(self.why(Some(status.to_string()))),
            Err(e) => State::Stopped(e.to_string()),
        }
    }

    /// The error line it ended with, if it said one, else `fallback`.
    fn why(&self, fallback: Option<String>) -> String {
        let t = self.tail.lock();
        t.iter()
            .rev()
            .find(|l| l.contains("ERR") || l.contains("error") || l.contains("address already"))
            .or(t.back())
            .map(|l| l.trim().to_string())
            .or(fallback)
            .unwrap_or_else(|| "it stopped".into())
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.pidfile);
    }
}

/// Keeps the server as asked, on a thread of its own: starts it, starts it
/// again with new settings, tries again a while after it fails, and stops it
/// (and waits for it to stop) when dropped.
pub struct Runner {
    tx: Option<Sender<Option<Hosting>>>,
    status: Arc<Mutex<Status>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// How long after a server stopped or couldn't start before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(10);

impl Runner {
    /// `root` is where the server keeps its files (see `Hosted::start`).
    pub fn start(root: PathBuf) -> Runner {
        let (tx, rx) = std::sync::mpsc::channel();
        let status = Arc::new(Mutex::new(Status::Off));
        let thread = {
            let status = status.clone();
            std::thread::Builder::new()
                .name("mediamtx".into())
                .spawn(move || work(rx, status, root))
                .ok()
        };
        Runner {
            tx: Some(tx),
            status,
            thread,
        }
    }

    /// Ask for the server to run as `hosting`, or to stop for `None`.
    pub fn want(&self, hosting: Option<Hosting>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(hosting);
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }

    /// Whether its thread is still there to do as asked.
    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn work(rx: Receiver<Option<Hosting>>, status: Arc<Mutex<Status>>, root: PathBuf) {
    let mut want: Option<Hosting> = None;
    let mut hosted: Option<Hosted> = None;
    let mut failed: Option<(Hosting, Instant)> = None;
    let set = |s: Status| *status.lock() = s;
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(w) => {
                if w != want {
                    want = w;
                    hosted = None;
                    failed = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let Some(h) = want else {
            hosted = None;
            set(Status::Off);
            continue;
        };
        if let Some(server) = &mut hosted
            && let State::Stopped(why) = server.state()
        {
            log::warn!("Streaming server stopped: {why}");
            hosted = None;
            failed = Some((h, Instant::now()));
            set(Status::Failed(why));
        }
        if hosted.is_none() {
            let waiting = failed.is_some_and(|(fh, at)| fh == h && at.elapsed() < RETRY_AFTER);
            if !waiting {
                match locate() {
                    None => {
                        failed = Some((h, Instant::now()));
                        set(Status::Missing);
                    }
                    Some(exe) => match Hosted::start(&exe, &root, h) {
                        Ok(server) => {
                            log::info!("Streaming server started");
                            hosted = Some(server);
                            failed = None;
                        }
                        Err(e) => {
                            log::warn!("Streaming server: {e}");
                            failed = Some((h, Instant::now()));
                            set(Status::Failed(e));
                        }
                    },
                }
            }
        }
        if hosted.is_some() {
            set(Status::Running(h));
        }
    }
}

#[cfg(test)]
mod stress;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{ServerProtocol, StreamKind};
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::Path;

    /// A port free for TCP and UDP (WebRTC uses both), never given out twice:
    /// tests run in parallel, and a port freed again is free for the next caller.
    fn free_port() -> u16 {
        static GIVEN: parking_lot::Mutex<Vec<u16>> = parking_lot::Mutex::new(Vec::new());
        let mut given = GIVEN.lock();
        loop {
            let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = tcp.local_addr().unwrap().port();
            if !given.contains(&port) && std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
                given.push(port);
                return port;
            }
        }
    }

    #[test]
    fn the_configuration_listens_on_the_chosen_adapter_only() {
        let h = Hosting::standard(Some(Ipv4Addr::new(192, 168, 1, 20).into()));
        let c = h.config();
        assert!(c.contains("rtspAddress: \"192.168.1.20:8554\""), "{c}");
        assert!(c.contains("webrtcAddress: \"192.168.1.20:8889\""), "{c}");
        assert!(
            c.contains("webrtcAdditionalHosts: [\"192.168.1.20\"]"),
            "{c}"
        );
        assert!(c.contains("webrtcIPsFromInterfaces: no"), "{c}");
        assert!(c.contains("paths:\n  all_others:"), "{c}");
    }

    #[test]
    fn without_an_adapter_it_listens_on_all_of_them() {
        let c = Hosting::standard(None).config();
        assert!(
            c.contains("rtspAddress: \":8554\"") && c.contains("srtAddress: \":8890\""),
            "{c}"
        );
        assert!(!c.contains("webrtcAdditionalHosts"), "{c}");
    }

    #[test]
    fn an_ipv6_adapter_is_bracketed() {
        let h = Hosting::standard(Some("fd00::5".parse().unwrap()));
        assert!(h.config().contains("rtspAddress: \"[fd00::5]:8554\""));
    }

    /// With a real MediaMTX (set NEWQ_MEDIAMTX_DIR, or have it on the PATH):
    /// start it, find its ports, and stop it.
    #[test]
    fn a_real_server_starts_listens_and_stops() {
        let Some(exe) = locate() else {
            eprintln!("no MediaMTX here: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq-mtx-run-{}", std::process::id()));
        let lo: IpAddr = Ipv4Addr::LOCALHOST.into();
        let hosting = Hosting {
            listen: Some(lo),
            rtsp: free_port(),
            rtmp: free_port(),
            srt: free_port(),
            player: free_port(),
            hls: free_port(),
            ice: free_port(),
            rtp: 0,
        };
        let mut server = Hosted::start(&exe, &dir, hosting).expect("start");
        let up = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            crate::streamlink::tcp_probe("127.0.0.1", hosting.rtsp, Duration::from_millis(200))
                .is_ok()
        });
        assert!(up, "the RTSP port never opened: {:?}", server.state());
        // The web player's port opens a little after RTSP's.
        assert!(
            wait_for(hosting.player),
            "the web player port never opened: {:?}",
            server.state()
        );
        assert_eq!(server.state(), State::Running);
        let probes = crate::streamlink::probe_server(
            crate::show::ServerProtocol::Rtsp,
            "127.0.0.1",
            hosting.rtsp,
            hosting.player,
        );
        assert!(probes.iter().all(|p| p.result.is_ok()), "{probes:?}");
        // A second server on the same ports says why it stopped.
        let mut clash = Hosted::start(&exe, &dir.join("clash"), hosting).expect("start");
        let stopped = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            matches!(clash.state(), State::Stopped(_))
        });
        assert!(stopped, "a second server on the same ports kept running");
        drop(server);
        assert!(
            crate::streamlink::tcp_probe("127.0.0.1", hosting.rtsp, Duration::from_millis(300))
                .is_err()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A server left running by a NewQ that was killed doesn't stop the next
    /// one from starting.
    fn wait_for(port: u16) -> bool {
        (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            crate::streamlink::tcp_probe("127.0.0.1", port, Duration::from_millis(200)).is_ok()
        })
    }

    fn lo_hosting() -> Hosting {
        Hosting {
            listen: Some(Ipv4Addr::LOCALHOST.into()),
            rtsp: free_port(),
            rtmp: free_port(),
            srt: free_port(),
            player: free_port(),
            hls: free_port(),
            ice: free_port(),
            rtp: 0,
        }
    }

    /// The process number of a program that has already ended.
    fn dead_pid(exe: &Path) -> u32 {
        let mut c = Command::new(exe)
            .arg("--version")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    fn set_owner(root: &Path, owner: u32, server: u32) {
        let pidfile = root.join(std::process::id().to_string()).join("server.pid");
        std::fs::write(pidfile, format!("{owner} {server}")).unwrap();
    }

    /// A server left running by a NewQ that was killed doesn't stop the next
    /// one from starting, whether that NewQ's number is gone or (as in a test
    /// run) the same as ours.
    #[test]
    fn a_server_left_behind_is_stopped_before_the_next_starts() {
        let Some(exe) = locate() else {
            eprintln!("no MediaMTX here: skipped");
            return;
        };
        for owner_gone in [false, true] {
            let root = std::env::temp_dir()
                .join(format!("newq-mtx-left-{}-{owner_gone}", std::process::id()));
            let hosting = lo_hosting();
            let first = Hosted::start(&exe, &root, hosting).expect("start");
            assert!(wait_for(hosting.rtsp));
            let server = first.child.id();
            // NewQ dying leaves the program running, which forgetting the handle imitates.
            std::mem::forget(first);
            if owner_gone {
                set_owner(&root, dead_pid(&exe), server);
            }
            let mut second = Hosted::start(&exe, &root, hosting).expect("start");
            std::thread::sleep(Duration::from_millis(1500));
            assert_eq!(second.state(), State::Running, "owner gone: {owner_gone}");
            drop(second);
            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// The server of a NewQ that is still running, or any other program that
    /// happens to have the number, is not touched.
    #[test]
    fn a_server_of_a_live_newq_is_left_alone() {
        let Some(exe) = locate() else {
            eprintln!("no MediaMTX here: skipped");
            return;
        };
        let root = std::env::temp_dir().join(format!("newq-mtx-live-{}", std::process::id()));
        let hosting = lo_hosting();
        let mut first = Hosted::start(&exe, &root, hosting).expect("start");
        assert!(wait_for(hosting.rtsp));
        // The owner is a process that is running (this test's parent), not us.
        let parent = sysinfo::System::new_all()
            .process(sysinfo::Pid::from_u32(std::process::id()))
            .and_then(|p| p.parent())
            .map(|p| p.as_u32())
            .expect("a parent process");
        set_owner(&root, parent, first.child.id());
        // Another run (a second start with the same files) must not kill it...
        stop_leftovers(&root, &exe);
        assert_eq!(first.state(), State::Running);
        // ...and a pid that is not a MediaMTX is never killed, even when its owner is gone.
        let mut sleeper = Command::new(if cfg!(windows) { "ping" } else { "sleep" })
            .args(
                if cfg!(windows) {
                    ["-n", "30", "127.0.0.1"]
                } else {
                    ["30", "", ""]
                }
                .iter()
                .filter(|a| !a.is_empty()),
            )
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        set_owner(&root, dead_pid(&exe), sleeper.id());
        stop_leftovers(&root, &exe);
        assert!(
            sleeper.try_wait().unwrap().is_none(),
            "killed an unrelated program"
        );
        sleeper.kill().ok();
        sleeper.wait().ok();
        drop(first);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_this_computer_may_send_a_picture_and_a_second_sender_cannot_take_over() {
        let h = Hosting::standard(Some(Ipv4Addr::new(192, 168, 1, 20).into()));
        let c = h.config();
        assert!(
            c.contains("ips: [\"127.0.0.1\", \"::1\", \"192.168.1.20\"]"),
            "{c}"
        );
        assert!(
            c.contains("- action: publish") && c.contains("- action: read"),
            "{c}"
        );
        // Reading is open to everyone: its entry has no addresses.
        assert!(c.contains("ips: []"), "{c}");
        assert!(c.contains("overridePublisher: no"), "{c}");
        // No other action (api, metrics, pprof, playback) is given to anyone.
        for other in [
            "action: api",
            "action: metrics",
            "action: pprof",
            "action: playback",
        ] {
            assert!(!c.contains(other), "{other}");
        }
        let all = Hosting::standard(None).config();
        assert!(all.contains("ips: [\"127.0.0.1\", \"::1\"]"), "{all}");
    }

    #[test]
    fn two_ways_in_on_one_port_are_reported_before_starting() {
        let ok = Hosting::standard(None);
        assert_eq!(ok.conflict(), None);
        let mut h = ok;
        h.rtmp = h.rtsp;
        let why = h.conflict().expect("a clash");
        assert!(
            why.contains("RTSP") && why.contains("RTMP") && why.contains("8554"),
            "{why}"
        );
        // The same number on TCP and UDP is two different ports.
        let mut h = ok;
        h.srt = h.rtsp;
        assert_eq!(h.conflict(), None);
        // SRT (UDP) against the RTP port, and RTCP is the next one.
        let mut h = ok;
        h.srt = 8001;
        assert!(h.conflict().expect("a clash").contains("RTCP"));
        // With RTP off its ports are free.
        h.rtp = 0;
        assert_eq!(h.conflict(), None);
        let exe = PathBuf::from("/does/not/matter");
        let mut h = ok;
        h.player = h.hls;
        let e = Hosted::start(&exe, &std::env::temp_dir().join("newq-mtx-clash"), h).err();
        assert!(e.is_some_and(|e| e.contains("different ports")));
    }

    fn server(
        protocol: ServerProtocol,
        port: u16,
        adapter: &str,
        player: u16,
        here: bool,
    ) -> StreamKind {
        StreamKind::Server {
            protocol,
            host: "10.0.0.9".into(),
            port,
            path: "newq".into(),
            kbps: 1000,
            adapter: adapter.into(),
            player_port: player,
            host_here: here,
            encoder: String::new(),
        }
    }

    #[test]
    fn one_server_serves_every_display_that_runs_its_own_and_they_agree_with_it() {
        let adapters = vec![
            crate::streamlink::Adapter {
                name: "wlan0".into(),
                ip: Ipv4Addr::new(192, 168, 1, 20).into(),
            },
            crate::streamlink::Adapter {
                name: "eth0".into(),
                ip: Ipv4Addr::new(10, 0, 0, 5).into(),
            },
        ];
        let mut kinds = vec![
            server(ServerProtocol::Rtsp, 9554, "wlan0", 9889, true),
            // Another server, not run here: no part of the plan.
            server(ServerProtocol::Rtmp, 1, "eth0", 1, false),
            // A second display here with other ideas.
            server(ServerProtocol::Rtsp, 7777, "eth0", 7778, true),
            server(ServerProtocol::Srt, 0, "eth0", 0, true),
        ];
        let p = plan(kinds.iter(), &adapters).expect("a plan");
        assert_eq!(p.adapter, "wlan0");
        assert_eq!(
            p.hosting.listen,
            Some(Ipv4Addr::new(192, 168, 1, 20).into())
        );
        // The first display's RTSP port and player port; SRT from the display that asked for it.
        assert_eq!((p.hosting.rtsp, p.hosting.player), (9554, 9889));
        assert_eq!(p.hosting.srt, ServerProtocol::Srt.default_port());
        assert_eq!(p.hosting.rtmp, 1935);
        for k in &mut kinds {
            align(k, &p);
        }
        // The one that runs elsewhere is left as it was.
        assert_eq!(kinds[1], server(ServerProtocol::Rtmp, 1, "eth0", 1, false));
        // The others now publish to, and are linked by, the server that runs.
        assert_eq!(
            kinds[2],
            server(ServerProtocol::Rtsp, 9554, "wlan0", 9889, true)
        );
        assert_eq!(
            kinds[3],
            server(ServerProtocol::Srt, 8890, "wlan0", 9889, true)
        );
        // None asks: no server.
        assert!(
            plan(
                [server(ServerProtocol::Rtsp, 0, "", 0, false)].iter(),
                &adapters
            )
            .is_none()
        );
        // An adapter that isn't there is a plan without an address to listen on,
        // which the app refuses rather than listen everywhere.
        let gone = plan(
            [server(ServerProtocol::Rtsp, 0, "gone0", 0, true)].iter(),
            &adapters,
        )
        .unwrap();
        assert_eq!(
            (gone.adapter.as_str(), gone.hosting.listen),
            ("gone0", None)
        );
    }

    /// A second program sending to the same stream is refused, and the first
    /// keeps its stream.
    #[test]
    fn a_second_sender_cannot_take_over_the_stream() {
        use crate::media::ffmpeg::Ffmpeg;
        use crate::stream::{Sink, encoder};
        let (Some(exe), Some(ff)) = (locate(), Ffmpeg::locate()) else {
            eprintln!("needs MediaMTX and FFmpeg: skipped");
            return;
        };
        if encoder(&ff).is_none() {
            eprintln!("no H.264 encoder: skipped");
            return;
        }
        let root = std::env::temp_dir().join(format!("newq-mtx-two-{}", std::process::id()));
        let hosting = lo_hosting();
        let _server = Hosted::start(&exe, &root, hosting).expect("start");
        assert!(wait_for(hosting.rtsp));
        let mut kind = StreamKind::server(ServerProtocol::Rtsp);
        if let StreamKind::Server {
            port, host_here, ..
        } = &mut kind
        {
            *port = hosting.rtsp;
            *host_here = true;
        }
        let size = (320u32, 180u32);
        let frame = [40u8, 120, 200, 255].repeat((size.0 * size.1) as usize);
        let mut first = Sink::start(&ff, size, 25, &kind).expect("start the first");
        // Let the first get its stream before the second tries.
        for _ in 0..50 {
            let _ = first.write(&frame);
            std::thread::sleep(Duration::from_millis(40));
        }
        assert!(
            first.failed().is_none(),
            "the first sender could not publish"
        );
        let mut second = Sink::start(&ff, size, 25, &kind).expect("start the second");
        let mut second_failed = false;
        for _ in 0..150 {
            let _ = first.write(&frame);
            if second.write(&frame).is_err() || second.failed().is_some() {
                second_failed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        assert!(second_failed, "a second sender was let in");
        assert!(first.failed().is_none(), "the first sender was pushed out");
        first.finish();
        std::fs::remove_dir_all(&root).ok();
    }

    /// The whole chain with the real programs: NewQ runs MediaMTX, publishes a
    /// picture to it through FFmpeg, the web player page answers, and the
    /// picture can be read back over RTSP.
    #[test]
    fn a_stream_published_to_the_hosted_server_can_be_watched() {
        use crate::media::ffmpeg::Ffmpeg;
        use crate::show::{ServerProtocol, StreamKind};
        use crate::stream::{Sink, encoder};
        use std::io::{Read, Write};
        let (Some(exe), Some(ff)) = (locate(), Ffmpeg::locate()) else {
            eprintln!("needs MediaMTX and FFmpeg: skipped");
            return;
        };
        if encoder(&ff).is_none() {
            eprintln!("no H.264 encoder: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("newq-mtx-e2e-{}", std::process::id()));
        let hosting = Hosting {
            listen: Some(Ipv4Addr::LOCALHOST.into()),
            rtsp: free_port(),
            rtmp: free_port(),
            srt: free_port(),
            player: free_port(),
            hls: free_port(),
            ice: free_port(),
            rtp: 0,
        };
        let mut server = Hosted::start(&exe, &dir, hosting).expect("start");
        assert!((0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            crate::streamlink::tcp_probe("127.0.0.1", hosting.rtsp, Duration::from_millis(200))
                .is_ok()
        }));
        let mut kind = StreamKind::server(ServerProtocol::Rtsp);
        if let StreamKind::Server {
            port,
            host_here,
            player_port,
            ..
        } = &mut kind
        {
            *port = hosting.rtsp;
            *host_here = true;
            *player_port = hosting.player;
        }
        let size = (320u32, 180u32);
        let frame = [40u8, 120, 200, 255].repeat((size.0 * size.1) as usize);
        let mut sink = Sink::start(&ff, size, 25, &kind).expect("start the sink");
        // Keep sending while the page and the reader are tried.
        let sender = std::thread::spawn(move || {
            for _ in 0..200 {
                if sink.write(&frame).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            sink.finish();
        });
        // The web player page the QR code opens.
        let mut page = String::new();
        let mut conn = std::net::TcpStream::connect(("127.0.0.1", hosting.player)).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(conn, "GET /newq/ HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
        conn.read_to_string(&mut page).ok();
        assert!(
            page.starts_with("HTTP/1.") && page.contains(" 200 "),
            "{}",
            &page[..page.len().min(200)]
        );
        // Read the picture back the way a player would.
        let mut got = false;
        for _ in 0..10 {
            let out = ff
                .command()
                .args(["-rtsp_transport", "tcp", "-i"])
                .arg(format!("rtsp://127.0.0.1:{}/newq", hosting.rtsp))
                .args(["-frames:v", "5", "-f", "null", "-"])
                .output()
                .unwrap();
            if out.status.success() {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(got, "the stream never played back: {:?}", server.state());
        sender.join().ok();
        drop(server);
        std::fs::remove_dir_all(&dir).ok();
    }
}
