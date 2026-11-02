//! A presenter-style remote (a PerfectCue, a slide clicker) as the GO and Back
//! buttons of a show.
//!
//! Such a remote is a keyboard to the computer: each button is a key. Rather
//! than guess which, NewQ *learns* them: press Learn in the settings, then the
//! button. The keys are then noticed wherever the focus is: with a hook on
//! Windows and an event tap on a Mac (which asks for the Input Monitoring
//! permission); elsewhere only while a NewQ window has the focus. The keys are
//! only compared with the two learned ones: nothing typed is kept or logged,
//! and the keys still reach whatever program has the focus.
//!
//! What the buttons do (`decide`):
//! - GO: fire the next cue; while everything is paused, resume instead.
//! - Back: pause everything; pressed again, fade everything out and stop.
//!   A second press within a second is ignored, so a double tap can't end a
//!   show by accident.

use crate::remote::Command;
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Go,
    Back,
}

/// What a press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Go,
    Pause,
    Resume,
    /// Fade everything out and stop (Esc: a second press stops at once).
    FadeOut,
    Nothing,
}

/// How soon after pausing a second Back is taken as a double tap.
pub const BACK_GUARD: Duration = Duration::from_secs(1);

/// What a press of `button` does, given whether everything is `paused`,
/// whether anything is `playing`, and how long ago Back last paused.
pub fn decide(
    button: Button,
    paused: bool,
    playing: bool,
    since_pause: Option<Duration>,
    guard: Duration,
) -> Do {
    match (button, paused) {
        (Button::Go, true) => Do::Resume,
        (Button::Go, false) => Do::Go,
        (Button::Back, true) => {
            if since_pause.is_some_and(|d| d < guard) {
                Do::Nothing
            } else {
                Do::FadeOut
            }
        }
        (Button::Back, false) => {
            if playing {
                Do::Pause
            } else {
                Do::Nothing
            }
        }
    }
}

/// What the keys are, the key being learned, and the way back to the show.
pub struct Shared {
    go: AtomicU32,
    back: AtomicU32,
    enabled: AtomicBool,
    /// 0 = not learning, 1 = GO, 2 = Back.
    learning: AtomicU8,
    learned: Mutex<Option<(Button, u32)>>,
    held: Mutex<HashSet<u32>>,
    /// The keys are noticed system-wide (else only in NewQ's windows).
    global_ok: AtomicBool,
    status: Mutex<String>,
    tx: Sender<Command>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    pub fn new(tx: Sender<Command>, wake: Arc<dyn Fn() + Send + Sync>) -> Arc<Shared> {
        Arc::new(Shared {
            go: AtomicU32::new(0),
            back: AtomicU32::new(0),
            enabled: AtomicBool::new(false),
            learning: AtomicU8::new(0),
            learned: Mutex::new(None),
            held: Mutex::new(HashSet::new()),
            global_ok: AtomicBool::new(false),
            status: Mutex::new(String::new()),
            tx,
            wake,
        })
    }

    /// The learned keys (0 = none) and whether they are acted on.
    pub fn set(&self, enabled: bool, go: Option<u32>, back: Option<u32>) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.go.store(go.unwrap_or(0), Ordering::Relaxed);
        self.back.store(back.unwrap_or(0), Ordering::Relaxed);
    }

    /// The next key pressed is the one for `button`.
    pub fn learn(&self, button: Button) {
        self.learning.store(
            match button {
                Button::Go => 1,
                Button::Back => 2,
            },
            Ordering::Relaxed,
        );
    }

    pub fn cancel_learning(&self) {
        self.learning.store(0, Ordering::Relaxed);
    }

    pub fn learning(&self) -> Option<Button> {
        match self.learning.load(Ordering::Relaxed) {
            1 => Some(Button::Go),
            2 => Some(Button::Back),
            _ => None,
        }
    }

    /// The key just learned, once.
    pub fn take_learned(&self) -> Option<(Button, u32)> {
        self.learned.lock().take()
    }

    pub fn global_ok(&self) -> bool {
        self.global_ok.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> String {
        self.status.lock().clone()
    }

    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    fn set_status(&self, ok: bool, text: impl Into<String>) {
        self.global_ok.store(ok, Ordering::Relaxed);
        *self.status.lock() = text.into();
    }

    /// A key went down (`down`) or up. `code` is the system's own code for it.
    pub fn key(&self, code: u32, down: bool) {
        if code == 0 {
            return;
        }
        if !down {
            self.held.lock().remove(&code);
            return;
        }
        // Holding a key repeats it; one press is one press.
        if !self.held.lock().insert(code) {
            return;
        }
        if let Some(b) = self.learning() {
            *self.learned.lock() = Some((b, code));
            self.cancel_learning();
            (self.wake)();
            return;
        }
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let button = if code == self.go.load(Ordering::Relaxed) {
            Button::Go
        } else if code == self.back.load(Ordering::Relaxed) {
            Button::Back
        } else {
            return;
        };
        let _ = self.tx.send(Command::Clicker(button));
        (self.wake)();
    }
}

/// How a learned key is shown.
pub fn describe(code: Option<u32>) -> String {
    match code {
        Some(c) => format!("learned (key code {c:#x})"),
        None => "not set".into(),
    }
}

/// Whether the presenter remote is offered at all. Off until it knows which
/// keyboard a key comes from (so that Right Arrow on the laptop doesn't fire
/// cues) and doesn't ask a Mac for the Input Monitoring permission at every
/// launch; the code stays so the work isn't lost.
pub const AVAILABLE: bool = false;

/// Start noticing keys system-wide, where that can be done (Windows, a Mac).
pub fn start_global(shared: Arc<Shared>) {
    #[cfg(windows)]
    win::start(shared);
    #[cfg(target_os = "macos")]
    mac::start(shared);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = shared;
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, SetWindowsHookExW,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

    /// The system calls this for every key, before the program that has the
    /// focus: it only looks, and passes the key on.
    unsafe extern "system" fn hook(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 {
            // SAFETY: for WH_KEYBOARD_LL, lparam points at a KBDLLHOOKSTRUCT.
            let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let message = wparam as u32;
            let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let up = message == WM_KEYUP || message == WM_SYSKEYUP;
            if (down || up)
                && let Some(shared) = SHARED.get()
            {
                // The scan code, with 0xE000 for the extended keys: what the
                // key is, not what it types.
                let extended = if k.flags & LLKHF_EXTENDED != 0 {
                    0xE000
                } else {
                    0
                };
                shared.key(k.scanCode | extended, down);
            }
        }
        // SAFETY: passing the call on as the system documents.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    pub fn start(shared: Arc<Shared>) {
        let _ = SHARED.set(shared.clone());
        let _ = std::thread::Builder::new()
            .name("remote-keys".into())
            .spawn(move || {
                // SAFETY: a hook on this thread, which then runs the message loop it needs.
                unsafe {
                    let hook = SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(hook),
                        GetModuleHandleW(std::ptr::null()),
                        0,
                    );
                    if hook.is_null() {
                        shared.set_status(
                            false,
                            "Windows would not let NewQ watch the keys; the buttons work while a NewQ window has the focus",
                        );
                        return;
                    }
                    shared.set_status(true, "watching the keys on this computer");
                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
                }
            });
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        EventField,
    };

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }

    const NEEDS_PERMISSION: &str = "Allow NewQ in System Settings ▸ Privacy & Security ▸ Input Monitoring so the buttons work when NewQ isn't in front; until then they work only while a NewQ window has the focus";

    pub fn start(shared: Arc<Shared>) {
        let _ = std::thread::Builder::new()
            .name("remote-keys".into())
            .spawn(move || {
                // SAFETY: plain permission queries.
                let allowed = unsafe { CGPreflightListenEventAccess() };
                if !allowed {
                    // Asks the user once; the answer applies the next time the tap is tried.
                    unsafe { CGRequestListenEventAccess() };
                }
                loop {
                    let tap_shared = shared.clone();
                    let tap = CGEventTap::new(
                        CGEventTapLocation::HID,
                        CGEventTapPlacement::HeadInsertEventTap,
                        // Listen only: the keys go on to whatever has the focus.
                        CGEventTapOptions::ListenOnly,
                        vec![CGEventType::KeyDown, CGEventType::KeyUp],
                        move |_proxy, kind, event| {
                            let code =
                                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                            match kind {
                                CGEventType::KeyDown => tap_shared.key(code as u32, true),
                                CGEventType::KeyUp => tap_shared.key(code as u32, false),
                                _ => {}
                            }
                            None
                        },
                    );
                    match tap {
                        Ok(tap) => {
                            let Ok(source) = tap.mach_port.create_runloop_source(0) else {
                                shared.set_status(false, NEEDS_PERMISSION);
                                std::thread::sleep(Duration::from_secs(5));
                                continue;
                            };
                            // SAFETY: the loop source belongs to this thread's run loop.
                            unsafe {
                                CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
                            }
                            tap.enable();
                            shared.set_status(true, "watching the keys on this computer");
                            CFRunLoop::run_current();
                            // The loop ended (the tap was turned off): try again.
                            shared.set_status(false, NEEDS_PERMISSION);
                        }
                        Err(()) => shared.set_status(false, NEEDS_PERMISSION),
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUARD: Duration = Duration::from_secs(1);

    #[test]
    fn go_goes_and_resumes_a_pause_and_back_pauses_then_fades_out() {
        let d = |b, paused, playing, since| decide(b, paused, playing, since, GUARD);
        assert_eq!(d(Button::Go, false, true, None), Do::Go);
        assert_eq!(d(Button::Go, false, false, None), Do::Go);
        // GO after Back: carry on from where it was.
        assert_eq!(d(Button::Go, true, true, None), Do::Resume);
        // Back with something playing pauses it; with nothing it does nothing.
        assert_eq!(d(Button::Back, false, true, None), Do::Pause);
        assert_eq!(d(Button::Back, false, false, None), Do::Nothing);
        // Back again fades out and stops...
        let slow = Some(Duration::from_secs(3));
        assert_eq!(d(Button::Back, true, true, slow), Do::FadeOut);
        // ...unless it is a double tap.
        let quick = Some(Duration::from_millis(300));
        assert_eq!(d(Button::Back, true, true, quick), Do::Nothing);
        assert_eq!(d(Button::Back, true, true, None), Do::FadeOut);
    }

    fn shared() -> (Arc<Shared>, crossbeam_channel::Receiver<Command>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (Shared::new(tx, Arc::new(|| {})), rx)
    }

    #[test]
    fn learned_keys_send_their_button_once_per_press_and_nothing_else_does() {
        let (s, rx) = shared();
        s.set(true, Some(0x51), Some(0x49));
        s.key(0x51, true);
        s.key(0x51, true); // held down: repeats
        s.key(0x51, true);
        s.key(0x51, false);
        s.key(0x49, true);
        s.key(0x49, false);
        s.key(0x1e, true); // some other key
        s.key(0x1e, false);
        let got: Vec<Command> = rx.try_iter().collect();
        assert_eq!(
            got,
            vec![Command::Clicker(Button::Go), Command::Clicker(Button::Back)]
        );
        // Pressed again after being let go: another press.
        s.key(0x51, true);
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn a_key_is_ignored_when_off_and_nothing_is_set_for_key_zero() {
        let (s, rx) = shared();
        s.set(false, Some(0x51), Some(0x49));
        s.key(0x51, true);
        assert_eq!(rx.try_iter().count(), 0);
        s.set(true, None, None);
        s.key(0, true);
        s.key(0x51, true);
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn the_next_key_is_learned_for_the_button_asked_and_does_not_fire_it() {
        let (s, rx) = shared();
        s.set(true, Some(0x51), Some(0x49));
        s.learn(Button::Back);
        assert_eq!(s.learning(), Some(Button::Back));
        s.key(0x33, true);
        s.key(0x33, false);
        assert_eq!(s.take_learned(), Some((Button::Back, 0x33)));
        assert_eq!(s.take_learned(), None, "once");
        assert_eq!(s.learning(), None);
        assert_eq!(rx.try_iter().count(), 0, "learning is not pressing");
        assert_eq!(describe(Some(0x33)), "learned (key code 0x33)");
        assert_eq!(describe(None), "not set");
    }
}
