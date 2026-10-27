//! NDI: sending a display's picture out as an NDI source and taking an NDI
//! source in as a camera.
//!
//! NDI's runtime is a library from NDI (not part of NewQ, and not something
//! NewQ may ship): it is loaded when needed from where NDI installs it. All
//! the talking to it is done in a copy of NewQ's own program started for the
//! purpose (`newq --ndi-list`, `--ndi-receive`, `--ndi-send`), with pictures
//! going over its standard input or output, so a fault in a third-party
//! library can't take the show down with it. The structures below are laid out
//! as NDI's published headers (SDK 6) have them.
//!
//! Only the converting and the plumbing are tested here; there is no NDI
//! runtime in the test environment, so the library calls themselves have
//! only been checked against the headers.

use libloading::{Library, Symbol};
use std::ffi::{CStr, CString, c_char, c_void};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// What a camera cue's device starts with when it is an NDI source.
pub const PREFIX: &str = "ndi:";

/// The camera-device name for the NDI source `name`.
pub fn device(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// The NDI source a camera-device name stands for, if it is one.
pub fn source_of(device: &str) -> Option<&str> {
    device.strip_prefix(PREFIX)
}

// ----- The library's structures (Processing.NDI.*.h) ---------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    name: *const c_char,
    url: *const c_char,
}

#[repr(C)]
struct FindCreate {
    show_local_sources: bool,
    groups: *const c_char,
    extra_ips: *const c_char,
}

#[repr(C)]
struct RecvCreate {
    source: Source,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    name: *const c_char,
}

#[repr(C)]
struct SendCreate {
    name: *const c_char,
    groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

#[repr(C)]
struct VideoFrame {
    xres: i32,
    yres: i32,
    fourcc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    picture_aspect_ratio: f32,
    frame_format_type: i32,
    timecode: i64,
    data: *mut u8,
    line_stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

impl VideoFrame {
    fn empty() -> VideoFrame {
        VideoFrame {
            xres: 0,
            yres: 0,
            fourcc: 0,
            frame_rate_n: 0,
            frame_rate_d: 0,
            picture_aspect_ratio: 0.0,
            frame_format_type: 0,
            timecode: 0,
            data: std::ptr::null_mut(),
            line_stride: 0,
            metadata: std::ptr::null(),
            timestamp: 0,
        }
    }
}

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}
const FOURCC_UYVY: u32 = fourcc(b'U', b'Y', b'V', b'Y');
const FOURCC_BGRA: u32 = fourcc(b'B', b'G', b'R', b'A');
const FOURCC_BGRX: u32 = fourcc(b'B', b'G', b'R', b'X');
const FOURCC_RGBA: u32 = fourcc(b'R', b'G', b'B', b'A');

const FRAME_TYPE_VIDEO: i32 = 1;
const COLOR_FORMAT_UYVY_BGRA: i32 = 1;
const BANDWIDTH_HIGHEST: i32 = 100;
const FRAME_FORMAT_PROGRESSIVE: i32 = 1;
const TIMECODE_SYNTHESIZE: i64 = i64::MAX;

type Instance = *mut c_void;

/// The library, loaded, with the functions this uses.
struct Ndi {
    initialize: unsafe extern "C" fn() -> bool,
    destroy: unsafe extern "C" fn(),
    find_create: unsafe extern "C" fn(*const FindCreate) -> Instance,
    find_destroy: unsafe extern "C" fn(Instance),
    find_wait: unsafe extern "C" fn(Instance, u32) -> bool,
    find_sources: unsafe extern "C" fn(Instance, *mut u32) -> *const Source,
    recv_create: unsafe extern "C" fn(*const RecvCreate) -> Instance,
    recv_destroy: unsafe extern "C" fn(Instance),
    recv_capture:
        unsafe extern "C" fn(Instance, *mut VideoFrame, *mut c_void, *mut c_void, u32) -> i32,
    recv_free_video: unsafe extern "C" fn(Instance, *const VideoFrame),
    send_create: unsafe extern "C" fn(*const SendCreate) -> Instance,
    send_destroy: unsafe extern "C" fn(Instance),
    send_video: unsafe extern "C" fn(Instance, *const VideoFrame),
    // Last, so it is dropped after nothing else needs it.
    _lib: Library,
}

/// The file NDI's runtime is called on this system.
fn library_name() -> &'static str {
    if cfg!(windows) {
        if cfg!(target_arch = "aarch64") {
            "Processing.NDI.Lib.arm64.dll"
        } else {
            "Processing.NDI.Lib.x64.dll"
        }
    } else if cfg!(target_os = "macos") {
        "libndi.dylib"
    } else {
        "libndi.so.6"
    }
}

/// Where the runtime might be, best first.
fn library_candidates() -> Vec<PathBuf> {
    let name = library_name();
    let mut found = Vec::new();
    #[cfg(test)]
    if let Some(dir) = TEST_RUNTIME.lock().clone() {
        found.push(dir.join(name));
    }
    if let Some(dir) = std::env::var_os("NDI_RUNTIME_DIR_V6") {
        found.push(PathBuf::from(dir).join(name));
    }
    if cfg!(target_os = "macos") {
        found.push(PathBuf::from("/usr/local/lib").join(name));
    }
    // Next to the program, then wherever the system looks.
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        found.push(dir.join(name));
    }
    found.push(PathBuf::from(name));
    found
}

/// Where the tests' stand-in for the runtime is, once built.
#[cfg(test)]
static TEST_RUNTIME: parking_lot::Mutex<Option<PathBuf>> = parking_lot::Mutex::new(None);

pub const NOT_INSTALLED: &str = "The NDI runtime isn't installed. Install the NDI Runtime or NDI \
     Tools from ndi.video, then try again";

/// Whether NDI's library can be found here (without starting it).
pub fn runtime_installed() -> bool {
    library_candidates()
        .iter()
        // SAFETY: loading a library runs its initialisers, which NDI's is
        // written for; nothing is called in it.
        .any(|p| unsafe { Library::new(p) }.is_ok())
}

impl Ndi {
    fn load() -> Result<Ndi, String> {
        let lib = library_candidates()
            .iter()
            // SAFETY: as in `runtime_installed`.
            .find_map(|p| unsafe { Library::new(p) }.ok())
            .ok_or_else(|| NOT_INSTALLED.to_string())?;
        macro_rules! get {
            ($name:literal) => {{
                // SAFETY: the type each is read as is the one in NDI's headers.
                let s: Symbol<_> = unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| format!("NDI's library has no {}: {e}", $name))?;
                *s
            }};
        }
        let ndi = Ndi {
            initialize: get!("NDIlib_initialize"),
            destroy: get!("NDIlib_destroy"),
            find_create: get!("NDIlib_find_create_v2"),
            find_destroy: get!("NDIlib_find_destroy"),
            find_wait: get!("NDIlib_find_wait_for_sources"),
            find_sources: get!("NDIlib_find_get_current_sources"),
            recv_create: get!("NDIlib_recv_create_v3"),
            recv_destroy: get!("NDIlib_recv_destroy"),
            recv_capture: get!("NDIlib_recv_capture_v3"),
            recv_free_video: get!("NDIlib_recv_free_video_v2"),
            send_create: get!("NDIlib_send_create"),
            send_destroy: get!("NDIlib_send_destroy"),
            send_video: get!("NDIlib_send_send_video_v2"),
            _lib: lib,
        };
        // SAFETY: the library is loaded; this is how NDI says to begin.
        if !unsafe { (ndi.initialize)() } {
            return Err("NDI can't run on this computer (its processor isn't supported)".into());
        }
        Ok(ndi)
    }
}

impl Drop for Ndi {
    fn drop(&mut self) {
        // SAFETY: balances `initialize`; everything made from it is gone first.
        unsafe { (self.destroy)() }
    }
}

/// A search for sources on the network.
struct Finder<'a> {
    ndi: &'a Ndi,
    handle: Instance,
}

impl<'a> Finder<'a> {
    fn new(ndi: &'a Ndi) -> Result<Finder<'a>, String> {
        let settings = FindCreate {
            show_local_sources: true,
            groups: std::ptr::null(),
            extra_ips: std::ptr::null(),
        };
        // SAFETY: `settings` outlives the call.
        let handle = unsafe { (ndi.find_create)(&settings) };
        if handle.is_null() {
            return Err("NDI could not start looking for sources".into());
        }
        Ok(Finder { ndi, handle })
    }

    /// The sources found so far: name and, for connecting, the structure NDI
    /// gave (valid until the next call here).
    fn sources(&self) -> Vec<(String, Source)> {
        let mut n: u32 = 0;
        // SAFETY: the instance is live; NDI sets `n` and keeps the list until
        // the next call.
        let list = unsafe { (self.ndi.find_sources)(self.handle, &mut n) };
        if list.is_null() {
            return Vec::new();
        }
        (0..n as usize)
            .filter_map(|i| {
                // SAFETY: `list` has `n` entries.
                let s = unsafe { *list.add(i) };
                if s.name.is_null() {
                    return None;
                }
                // SAFETY: NDI's names are NUL-terminated strings.
                let name = unsafe { CStr::from_ptr(s.name) }
                    .to_string_lossy()
                    .into_owned();
                Some((name, s))
            })
            .collect()
    }

    /// Wait up to `ms` for the list to change.
    fn wait(&self, ms: u32) -> bool {
        // SAFETY: the instance is live.
        unsafe { (self.ndi.find_wait)(self.handle, ms) }
    }
}

impl Drop for Finder<'_> {
    fn drop(&mut self) {
        // SAFETY: made by `find_create`, destroyed once.
        unsafe { (self.ndi.find_destroy)(self.handle) }
    }
}

// ----- What runs inside the helper ---------------------------------------------

/// `--ndi-list`: print the names of the sources found, one to a line.
pub fn run_list(out: &mut impl Write) -> i32 {
    let ndi = match Ndi::load() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let finder = match Finder::new(&ndi) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Sources announce themselves over a second or two.
    let mut names: Vec<String> = Vec::new();
    for _ in 0..4 {
        finder.wait(600);
        names = finder.sources().into_iter().map(|(n, _)| n).collect();
    }
    names.sort();
    names.dedup();
    for n in names {
        let _ = writeln!(out, "{n}");
    }
    0
}

/// `--ndi-receive <source> <width> <height> <601|709>`: write the source's
/// pictures to standard output as raw yuv420p frames of that size.
pub fn run_receive(args: &[String], out: &mut impl Write) -> i32 {
    let (Some(name), Some(w), Some(h)) = (
        args.first(),
        args.get(1).and_then(|s| s.parse::<usize>().ok()),
        args.get(2).and_then(|s| s.parse::<usize>().ok()),
    ) else {
        eprintln!("usage: --ndi-receive <source> <width> <height> <601|709>");
        return 1;
    };
    let bt709 = args.get(3).is_none_or(|m| m != "601");
    let (w, h) = ((w.max(2)) & !1, (h.max(2)) & !1);
    let ndi = match Ndi::load() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let finder = match Finder::new(&ndi) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Find the source by name (up to ten seconds), to connect with what NDI gave.
    let mut source = None;
    for _ in 0..20 {
        finder.wait(500);
        source = finder
            .sources()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s);
        if source.is_some() {
            break;
        }
    }
    let Some(source) = source else {
        eprintln!("The NDI source “{name}” was not found on the network");
        return 3;
    };
    let recv_name = CString::new("NewQ").unwrap_or_default();
    let settings = RecvCreate {
        source,
        color_format: COLOR_FORMAT_UYVY_BGRA,
        bandwidth: BANDWIDTH_HIGHEST,
        allow_video_fields: false,
        name: recv_name.as_ptr(),
    };
    // SAFETY: `settings` and what it points to outlive the call.
    let recv = unsafe { (ndi.recv_create)(&settings) };
    if recv.is_null() {
        eprintln!("NDI could not connect to “{name}”");
        return 3;
    }
    let mut frame = Vec::new();
    let mut said_format = false;
    let code = loop {
        let mut video = VideoFrame::empty();
        // SAFETY: `video` is ours; the audio and metadata pointers may be null
        // (those frames are then not delivered).
        let kind = unsafe {
            (ndi.recv_capture)(
                recv,
                &mut video,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1000,
            )
        };
        if kind != FRAME_TYPE_VIDEO {
            continue;
        }
        let ok = convert_frame(&video, w, h, bt709, &mut frame);
        // SAFETY: the frame came from this receiver and is freed once.
        unsafe { (ndi.recv_free_video)(recv, &video) };
        if !ok {
            if !said_format {
                said_format = true;
                eprintln!("NDI sent a picture format NewQ can't show");
            }
            continue;
        }
        if out.write_all(&frame).is_err() || out.flush().is_err() {
            break 0; // the show closed the pipe: it is done with us
        }
    };
    // SAFETY: made by `recv_create`, destroyed once, before the library goes.
    unsafe { (ndi.recv_destroy)(recv) };
    code
}

/// A frame from NDI, as yuv420p of `w`×`h` in `out`. False if the format
/// isn't one this converts.
fn convert_frame(v: &VideoFrame, w: usize, h: usize, bt709: bool, out: &mut Vec<u8>) -> bool {
    if v.data.is_null() || v.xres < 2 || v.yres < 2 || v.line_stride <= 0 {
        return false;
    }
    let (sw, sh, stride) = (v.xres as usize, v.yres as usize, v.line_stride as usize);
    let bytes_per_row = match v.fourcc {
        FOURCC_UYVY => 2 * sw,
        FOURCC_BGRA | FOURCC_BGRX | FOURCC_RGBA => 4 * sw,
        _ => return false,
    };
    if stride < bytes_per_row {
        return false;
    }
    // SAFETY: NDI gives `stride` bytes for each of the `yres` rows.
    let data = unsafe { std::slice::from_raw_parts(v.data, stride * sh) };
    let planes = match v.fourcc {
        FOURCC_UYVY => i420_from_uyvy(data, stride, sw & !1, sh & !1),
        FOURCC_BGRA | FOURCC_BGRX => i420_from_rgb(data, stride, sw & !1, sh & !1, true, bt709),
        _ => i420_from_rgb(data, stride, sw & !1, sh & !1, false, bt709),
    };
    let (sw, sh) = (sw & !1, sh & !1);
    out.clear();
    for (plane, (pw, ph), (dw, dh)) in [
        (&planes.0, (sw, sh), (w, h)),
        (&planes.1, (sw / 2, sh / 2), (w / 2, h / 2)),
        (&planes.2, (sw / 2, sh / 2), (w / 2, h / 2)),
    ] {
        if (pw, ph) == (dw, dh) {
            out.extend_from_slice(plane);
        } else {
            scale_plane(plane, pw, ph, dw, dh, out);
        }
    }
    true
}

/// The three planes of 4:2:0 from packed 4:2:2 UYVY (U Y V Y per two pixels):
/// the colour of two rows is averaged into one.
fn i420_from_uyvy(data: &[u8], stride: usize, w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    let (mut u, mut v) = (vec![0u8; w / 2 * (h / 2)], vec![0u8; w / 2 * (h / 2)]);
    for row in 0..h {
        let line = &data[row * stride..row * stride + 2 * w];
        for x in 0..w / 2 {
            y[row * w + 2 * x] = line[4 * x + 1];
            y[row * w + 2 * x + 1] = line[4 * x + 3];
        }
        if row % 2 == 0 {
            let next = &data[(row + 1) * stride..(row + 1) * stride + 2 * w];
            for x in 0..w / 2 {
                let at = (row / 2) * (w / 2) + x;
                u[at] = (line[4 * x] as u16 + next[4 * x] as u16).div_ceil(2) as u8;
                v[at] = (line[4 * x + 2] as u16 + next[4 * x + 2] as u16).div_ceil(2) as u8;
            }
        }
    }
    (y, u, v)
}

/// The three planes of 4:2:0 (limited range) from 8-bit BGRA/RGBA, with the
/// colour of each 2×2 block averaged.
fn i420_from_rgb(
    data: &[u8],
    stride: usize,
    w: usize,
    h: usize,
    bgr: bool,
    bt709: bool,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (kr, kb) = if bt709 {
        (0.2126, 0.0722)
    } else {
        (0.299, 0.114)
    };
    let kg = 1.0 - kr - kb;
    let rgb = |x: usize, row: usize| {
        let p = &data[row * stride + 4 * x..row * stride + 4 * x + 4];
        if bgr {
            (p[2] as f32, p[1] as f32, p[0] as f32)
        } else {
            (p[0] as f32, p[1] as f32, p[2] as f32)
        }
    };
    let mut y = vec![0u8; w * h];
    let (mut u, mut v) = (vec![0u8; w / 2 * (h / 2)], vec![0u8; w / 2 * (h / 2)]);
    for row in 0..h {
        for x in 0..w {
            let (r, g, b) = rgb(x, row);
            let luma = kr * r + kg * g + kb * b;
            y[row * w + x] = (16.0 + luma * 219.0 / 255.0).round().clamp(16.0, 235.0) as u8;
        }
    }
    for by in 0..h / 2 {
        for bx in 0..w / 2 {
            let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (pr, pg, pb) = rgb(2 * bx + dx, 2 * by + dy);
                r += pr / 4.0;
                g += pg / 4.0;
                b += pb / 4.0;
            }
            let luma = kr * r + kg * g + kb * b;
            let cb = (b - luma) / (2.0 * (1.0 - kb));
            let cr = (r - luma) / (2.0 * (1.0 - kr));
            u[by * (w / 2) + bx] = (128.0 + cb * 224.0 / 255.0).round().clamp(16.0, 240.0) as u8;
            v[by * (w / 2) + bx] = (128.0 + cr * 224.0 / 255.0).round().clamp(16.0, 240.0) as u8;
        }
    }
    (y, u, v)
}

/// Append `src` (`sw`×`sh`) scaled to `dw`×`dh` (bilinear) to `out`.
fn scale_plane(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, out: &mut Vec<u8>) {
    let (dw, dh) = (dw.max(1), dh.max(1));
    out.reserve(dw * dh);
    for dy in 0..dh {
        let fy = ((dy as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for dx in 0..dw {
            let fx = ((dx as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let top = src[y0 * sw + x0] as f32 * (1.0 - tx) + src[y0 * sw + x1] as f32 * tx;
            let bottom = src[y1 * sw + x0] as f32 * (1.0 - tx) + src[y1 * sw + x1] as f32 * tx;
            out.push((top * (1.0 - ty) + bottom * ty).round() as u8);
        }
    }
}

/// `--ndi-send <name> <width> <height> <fps>`: send what comes in on standard
/// input (raw RGBA frames of that size) as the NDI source `name`.
pub fn run_send(args: &[String], input: &mut impl Read) -> i32 {
    let (Some(name), Some(w), Some(h), Some(fps)) = (
        args.first(),
        args.get(1).and_then(|s| s.parse::<usize>().ok()),
        args.get(2).and_then(|s| s.parse::<usize>().ok()),
        args.get(3).and_then(|s| s.parse::<i32>().ok()),
    ) else {
        eprintln!("usage: --ndi-send <name> <width> <height> <fps>");
        return 1;
    };
    let Ok(cname) = CString::new(name.as_str()) else {
        eprintln!("the NDI name can't have a NUL in it");
        return 1;
    };
    let ndi = match Ndi::load() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let settings = SendCreate {
        name: cname.as_ptr(),
        groups: std::ptr::null(),
        // NDI paces the video to its frame rate.
        clock_video: true,
        clock_audio: false,
    };
    // SAFETY: `settings` and the name outlive the call.
    let send = unsafe { (ndi.send_create)(&settings) };
    if send.is_null() {
        eprintln!("NDI could not make the source “{name}”");
        return 3;
    }
    let mut frame = vec![0u8; w * h * 4];
    while input.read_exact(&mut frame).is_ok() {
        let video = VideoFrame {
            xres: w as i32,
            yres: h as i32,
            fourcc: FOURCC_RGBA,
            frame_rate_n: fps.max(1),
            frame_rate_d: 1,
            picture_aspect_ratio: w as f32 / h.max(1) as f32,
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: TIMECODE_SYNTHESIZE,
            data: frame.as_mut_ptr(),
            line_stride: (w * 4) as i32,
            metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: `video` points at `frame`, which holds every byte of it;
        // the synchronous call has finished with it when it returns.
        unsafe { (ndi.send_video)(send, &video) };
    }
    // SAFETY: made by `send_create`, destroyed once, before the library goes.
    unsafe { (ndi.send_destroy)(send) };
    0
}

// ----- What the show runs ------------------------------------------------------

/// A command that runs this program as an NDI helper with `args`.
pub fn helper(args: &[String]) -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find NewQ's program: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::media::ffmpeg::no_console(&mut cmd);
    Ok(cmd)
}

/// The NDI sources on the network, found by a helper (about two seconds).
/// Empty if the runtime isn't installed.
#[cfg_attr(test, allow(dead_code))]
pub fn list_sources() -> Vec<String> {
    let Ok(mut cmd) = helper(&["--ndi-list".into()]) else {
        return Vec::new();
    };
    cmd.stderr(Stdio::null());
    cmd.output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// If the arguments ask for an NDI helper, run it and say what to exit with.
pub fn run_helper_if_asked(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str)? {
        "--ndi-list" => Some(run_list(&mut std::io::stdout().lock())),
        "--ndi-receive" => Some(run_receive(&args[1..], &mut std::io::stdout().lock())),
        "--ndi-send" => Some(run_send(&args[1..], &mut std::io::stdin().lock())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_a_camera_device_by_its_prefix() {
        assert_eq!(device("STUDIO (OBS)"), "ndi:STUDIO (OBS)");
        assert_eq!(source_of("ndi:STUDIO (OBS)"), Some("STUDIO (OBS)"));
        assert_eq!(source_of("/dev/video0"), None);
    }

    #[test]
    fn the_structures_are_laid_out_as_the_headers_have_them() {
        let p = std::mem::size_of::<usize>();
        // NDIlib_video_frame_v2_t: 7 four-byte fields before the 8-byte time
        // code (with padding), then pointer, stride, pointer, time stamp.
        assert_eq!(std::mem::offset_of!(VideoFrame, timecode), 32);
        assert_eq!(std::mem::offset_of!(VideoFrame, data), 40);
        assert_eq!(std::mem::offset_of!(VideoFrame, line_stride), 40 + p);
        assert_eq!(std::mem::offset_of!(VideoFrame, metadata), 48 + p);
        assert_eq!(std::mem::size_of::<VideoFrame>(), 48 + 3 * p);
        assert_eq!(std::mem::size_of::<Source>(), 2 * p);
        assert_eq!(std::mem::offset_of!(FindCreate, groups), p);
        assert_eq!(std::mem::offset_of!(RecvCreate, color_format), 2 * p);
        assert_eq!(std::mem::offset_of!(SendCreate, clock_video), 2 * p);
        assert_eq!(FOURCC_UYVY, 0x59565955);
        assert_eq!(FOURCC_BGRA, 0x41524742);
    }

    /// A UYVY frame of one flat colour: `y`, `u`, `v`.
    fn flat_uyvy(w: usize, h: usize, y: u8, u: u8, v: u8) -> Vec<u8> {
        (0..w * h / 2).flat_map(|_| [u, y, v, y]).collect()
    }

    #[test]
    fn uyvy_becomes_planar_420_with_the_colour_of_row_pairs_averaged() {
        let (w, h) = (4, 4);
        let mut data = flat_uyvy(w, h, 100, 90, 200);
        // Rows 0 and 1 have different colour: the one chroma row is their mean.
        for x in 0..w / 2 {
            data[w * 2 + 4 * x] = 110;
        }
        let (y, u, v) = i420_from_uyvy(&data, w * 2, w, h);
        assert!(y.iter().all(|&p| p == 100));
        assert_eq!(&u[..2], &[100, 100]);
        assert_eq!(&u[2..], &[90, 90]);
        assert!(v.iter().all(|&p| p == 200));
    }

    #[test]
    fn white_and_black_rgb_land_on_the_ends_of_limited_range() {
        let px = |c: u8| [c, c, c, 255];
        let (w, h) = (2, 2);
        let white: Vec<u8> = (0..4).flat_map(|_| px(255)).collect();
        let black: Vec<u8> = (0..4).flat_map(|_| px(0)).collect();
        for bt709 in [true, false] {
            let (y, u, v) = i420_from_rgb(&white, w * 4, w, h, true, bt709);
            assert_eq!((y[0], u[0], v[0]), (235, 128, 128));
            let (y, u, v) = i420_from_rgb(&black, w * 4, w, h, true, bt709);
            assert_eq!((y[0], u[0], v[0]), (16, 128, 128));
        }
        // Red (as BGRA) has the red difference high and the blue low.
        let red: Vec<u8> = (0..4).flat_map(|_| [0, 0, 255, 255]).collect();
        let (_, u, v) = i420_from_rgb(&red, w * 4, w, h, true, true);
        assert!(v[0] > 200 && u[0] < 110, "{u:?} {v:?}");
        // The same red as RGBA gives the same colour.
        let red_rgba: Vec<u8> = (0..4).flat_map(|_| [255, 0, 0, 255]).collect();
        assert_eq!(i420_from_rgb(&red_rgba, w * 4, w, h, false, true).2, v);
    }

    #[test]
    fn scaling_keeps_a_flat_plane_flat_and_gives_the_size_asked() {
        let src = vec![77u8; 8 * 6];
        let mut out = Vec::new();
        scale_plane(&src, 8, 6, 5, 3, &mut out);
        assert_eq!(out.len(), 15);
        assert!(out.iter().all(|&p| p == 77));
        // A ramp stays a ramp, going up and coming down.
        let ramp: Vec<u8> = (0..4).flat_map(|_| [0, 85, 170, 255]).collect();
        let mut big = Vec::new();
        scale_plane(&ramp, 4, 4, 8, 4, &mut big);
        assert!(big[..8].windows(2).all(|p| p[0] <= p[1]), "{big:?}");
        assert_eq!((big[0], big[7]), (0, 255));
    }

    #[test]
    fn a_whole_frame_is_converted_to_the_size_wanted() {
        let (sw, sh) = (8usize, 4usize);
        let mut data = flat_uyvy(sw, sh, 120, 128, 128);
        let v = VideoFrame {
            xres: sw as i32,
            yres: sh as i32,
            fourcc: FOURCC_UYVY,
            line_stride: (sw * 2) as i32,
            data: data.as_mut_ptr(),
            ..VideoFrame::empty()
        };
        let mut out = Vec::new();
        assert!(convert_frame(&v, 4, 2, true, &mut out));
        assert_eq!(out.len(), 4 * 2 + 2 * 2);
        assert!(out[..8].iter().all(|&p| p == 120));
        // Frames in formats it doesn't know, or with no data, are refused.
        let odd = VideoFrame {
            fourcc: fourcc(b'P', b'2', b'1', b'6'),
            ..VideoFrame::empty()
        };
        assert!(!convert_frame(&odd, 4, 2, true, &mut out));
    }

    #[test]
    fn helpers_are_recognised_by_their_first_argument() {
        assert_eq!(run_helper_if_asked(&[]), None);
        assert_eq!(run_helper_if_asked(&["show.newq".into()]), None);
        // A helper with too few arguments says how to use it.
        assert_eq!(run_helper_if_asked(&["--ndi-receive".into()]), Some(1));
        assert_eq!(run_helper_if_asked(&["--ndi-send".into()]), Some(1));
    }

    /// A stand-in for NDI's library with the functions this uses, built as C
    /// from the headers' declarations: two sources, a flat grey UYVY picture
    /// for every capture, and every picture sent noted in a file. It checks
    /// the plumbing (loading, structures, loops, cleaning up), not that NDI's
    /// own library agrees with the headers.
    #[cfg(target_os = "linux")]
    const FAKE: &str = r#"
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef struct { const char* name; const char* url; } Source;
typedef struct { bool show_local; const char* groups; const char* extra; } FindCreate;
typedef struct { Source source; int color_format; int bandwidth; bool fields; const char* name; } RecvCreate;
typedef struct { const char* name; const char* groups; bool clock_video, clock_audio; } SendCreate;
typedef struct { int xres, yres; uint32_t fourcc; int fr_n, fr_d; float aspect; int format; int64_t timecode; uint8_t* data; int stride; const char* metadata; int64_t timestamp; } Video;
static Source sources[2] = { {"FAKE (Camera A)", "1.2.3.4:5960"}, {"FAKE (Camera B)", "1.2.3.5:5960"} };
static uint8_t picture[8 * 4 * 2];
static int dummy;
bool NDIlib_initialize(void) { return true; }
void NDIlib_destroy(void) {}
void* NDIlib_find_create_v2(const FindCreate* c) { return c && c->show_local ? &dummy : 0; }
void NDIlib_find_destroy(void* f) {}
bool NDIlib_find_wait_for_sources(void* f, uint32_t ms) { return true; }
const Source* NDIlib_find_get_current_sources(void* f, uint32_t* n) { *n = 2; return sources; }
void* NDIlib_recv_create_v3(const RecvCreate* c) {
  return c && c->source.name && strcmp(c->source.name, "FAKE (Camera A)") == 0 && c->color_format == 1 ? &dummy : 0; }
void NDIlib_recv_destroy(void* r) {}
int NDIlib_recv_capture_v3(void* r, Video* v, void* a, void* m, uint32_t ms) {
  for (int i = 0; i < 8 * 4; i += 2) { picture[2 * i] = 128; picture[2 * i + 1] = 120; picture[2 * i + 2] = 128; picture[2 * i + 3] = 120; }
  memset(v, 0, sizeof *v);
  v->xres = 8; v->yres = 4; v->fourcc = 0x59565955; v->fr_n = 30; v->fr_d = 1;
  v->data = picture; v->stride = 16; return 1; }
void NDIlib_recv_free_video_v2(void* r, const Video* v) {}
void* NDIlib_send_create(const SendCreate* c) { return c && c->name && c->clock_video ? &dummy : 0; }
void NDIlib_send_destroy(void* s) {}
void NDIlib_send_send_video_v2(void* s, const Video* v) {
  FILE* f = fopen("@LOG@", "a");
  fprintf(f, "%d %d %08x %d %d/%d %d\n", v->xres, v->yres, v->fourcc, v->stride, v->fr_n, v->fr_d, v->data[0]);
  fclose(f); }
"#;

    /// Build the fake library into a folder of its own, and point the helpers
    /// at it. None if there is no C compiler here.
    #[cfg(target_os = "linux")]
    fn fake_runtime() -> Option<PathBuf> {
        static BUILT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
        BUILT
            .get_or_init(|| {
                let dir =
                    std::env::temp_dir().join(format!("newq-fake-ndi-{}", std::process::id()));
                std::fs::create_dir_all(&dir).ok()?;
                let src = dir.join("fake.c");
                std::fs::write(
                    &src,
                    FAKE.replace("@LOG@", &dir.join("sent.log").to_string_lossy()),
                )
                .ok()?;
                let out = dir.join(library_name());
                let built = Command::new("cc")
                    .args(["-shared", "-fPIC", "-o"])
                    .arg(&out)
                    .arg(&src)
                    .status()
                    .ok()?
                    .success();
                built.then(|| *TEST_RUNTIME.lock() = Some(dir.clone()));
                built.then_some(dir)
            })
            .clone()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_helpers_talk_to_a_runtime_through_the_structures_in_the_headers() {
        let Some(dir) = fake_runtime() else {
            eprintln!("no C compiler: skipped");
            return;
        };
        assert!(runtime_installed());
        // Listing: both sources, once each, in order.
        let mut listed = Vec::new();
        assert_eq!(run_list(&mut listed), 0);
        assert_eq!(
            String::from_utf8(listed).unwrap(),
            "FAKE (Camera A)\nFAKE (Camera B)\n"
        );
        // Receiving: frames of the size asked, flat grey, until the reader
        // goes away (here, after three).
        struct Three(Vec<u8>, usize);
        impl Write for Three {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if self.1 == 3 {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                self.1 += 1;
                self.0.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = Three(Vec::new(), 0);
        let args: Vec<String> = ["FAKE (Camera A)", "4", "2", "709"]
            .map(String::from)
            .to_vec();
        assert_eq!(run_receive(&args, &mut out), 0);
        // yuv420p of 4×2: 8 + 2 + 2 bytes a frame.
        assert_eq!(out.0.len(), 3 * 12);
        assert!(out.0[..8].iter().all(|&p| p == 120));
        assert!(out.0[8..12].iter().all(|&p| p == 128));
        // A source that isn't there is said so, with the exit code for that.
        let mut none = Vec::new();
        let missing: Vec<String> = ["Nobody", "4", "2"].map(String::from).to_vec();
        assert_eq!(run_receive(&missing, &mut none), 3);
        assert!(none.is_empty());
        // Sending: each frame of RGBA read is sent as RGBA of the stride and
        // rate given.
        let log = dir.join("sent.log");
        let _ = std::fs::remove_file(&log);
        let mut frames: Vec<u8> = Vec::new();
        for n in 0..2u8 {
            frames.extend(std::iter::repeat_n(n + 10, 4 * 2 * 4));
        }
        let args: Vec<String> = ["NewQ test", "4", "2", "25"].map(String::from).to_vec();
        assert_eq!(run_send(&args, &mut frames.as_slice()), 0);
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(sent, "4 2 41424752 16 25/1 10\n4 2 41424752 16 25/1 11\n");
        // (RGBA is 0x41424752.)
        assert_eq!(FOURCC_RGBA, 0x41424752);
        let _ = std::fs::remove_dir_all(dir);
    }
}
