//! How a Mac's output windows cover their display.
//!
//! winit's "simple fullscreen" hides the menu bar and Dock by changing the
//! application's presentation options, which belong to the whole app: the
//! control window's display loses its menu bar too, and the hiding is only
//! "auto": the menu bar comes back when the app is activated again (opening a
//! Finder window from a browser does it) or the pointer touches the top edge.
//! The window is an ordinary one besides, which Mission Control moves aside.
//!
//! Instead the output window is a borderless one that covers its display
//! exactly, above the menu bar's level (so the menu bar and Dock are under
//! it, on that display only), on every Space, and told to stay put when
//! Mission Control or Exposé rearrange windows. It has no shadow: a
//! borderless window's shadow is a faint line round its edge.
//!
//! Mac only, and not tried on a real Mac by its author: `create_screen` falls
//! back to simple fullscreen when any of this can't be done.

use crate::monitor::Rect;
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSColor, NSCursor, NSScreen, NSView,
    NSWindowCollectionBehavior,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject, NSRect};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::monitor::MonitorHandle;
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The menu bar's window level is 24 (`NSMainMenuWindowLevel`); the Dock's is
/// lower. One above covers both.
const ABOVE_THE_MENU_BAR: isize = 25;

fn rect_of(r: NSRect) -> Rect {
    Rect {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    }
}

/// The window's frame and its monitor's, in the system's global coordinates
/// (see `monitor::Rect`). The monitor's is None when the system has no screen
/// for it now (a lid closed, a display unplugged).
pub fn frames(window: &Window, monitor: &MonitorHandle) -> Option<(Rect, Option<Rect>)> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return None;
    };
    // SAFETY: as in `native`.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let ns_window = view.window()?;
    let screen = monitor.ns_screen().map(|s| {
        // SAFETY: winit's pointer to the monitor's live NSScreen.
        let screen: &NSScreen = unsafe { &*(s as *const NSScreen) };
        rect_of(screen.frame())
    });
    Some((rect_of(ns_window.frame()), screen))
}

/// Hide the pointer until the mouse is moved. The system shows it again when
/// it rearranges displays and windows (a lid closing puts it on the output),
/// and winit's own hiding doesn't survive that.
pub fn hide_pointer() {
    // SAFETY: a plain class method.
    unsafe { NSCursor::setHiddenUntilMouseMoves(true) };
}

/// Set when the system says the displays changed (see `watch_screens`).
static SCREENS_CHANGED: AtomicBool = AtomicBool::new(false);

/// Whether the displays changed since this was last asked; asking clears it.
pub fn take_screens_changed() -> bool {
    SCREENS_CHANGED.swap(false, Ordering::AcqRel)
}

/// Ask the system to tell NewQ the moment the displays are rearranged (a lid
/// closed or opened, a monitor plugged in), by setting a flag and calling
/// `wake` to bring the event loop round. The polling the loop does anyway is
/// the safety net: this only makes the answer prompt. The returned observer
/// must be kept; dropping it does not remove it, but keep it anyway.
pub fn watch_screens(wake: impl Fn() + 'static) -> objc2::rc::Retained<NSObject> {
    let block = block2::RcBlock::new(move |_note: NonNull<NSNotification>| {
        SCREENS_CHANGED.store(true, Ordering::Release);
        wake();
    });
    // SAFETY: the notification name is the system's own constant; the block
    // is called on the thread that posts the notification, the main one.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    }
}

/// Make `window` cover all of `monitor`. False if the system's window or screen
/// couldn't be reached, in which case nothing was changed.
pub fn cover_display(window: &Window, monitor: &MonitorHandle) -> bool {
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return false;
    };
    let Some(screen) = monitor.ns_screen() else {
        return false;
    };
    // SAFETY: winit's pointers to the window's live NSView and the monitor's
    // NSScreen, used on the main thread where windows are made.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let screen: &NSScreen = unsafe { &*(screen as *const NSScreen) };
    let Some(ns_window) = view.window() else {
        return false;
    };
    ns_window.setFrame_display(screen.frame(), true);
    ns_window.setLevel(ABOVE_THE_MENU_BAR);
    ns_window.setHasShadow(false);
    ns_window.setOpaque(true);
    // Black behind the picture: whenever the window has nothing to show yet
    // (the system is rearranging displays and holds back the next frame) the
    // audience sees black, not the window's default colour or what is behind.
    // SAFETY: a plain class method returning a shared colour.
    let black = unsafe { NSColor::blackColor() };
    ns_window.setBackgroundColor(Some(&black));
    // SAFETY: plain setters on a live window.
    unsafe {
        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        ns_window.setHidesOnDeactivate(false);
        ns_window.setCanHide(false);
    }
    true
}

/// Set when a Quit from the app menu (Cmd+Q, the Dock's Quit) was turned into
/// a question in the window (see `ask_before_quitting`).
static QUIT_ASKED: AtomicBool = AtomicBool::new(false);
/// Whether a Quit now may go straight through: nothing to ask
/// (`ui::quit_would_ask`), or a display computer. Set by the app each pass.
static QUIT_FREELY: AtomicBool = AtomicBool::new(true);

thread_local! {
    /// What brings the event loop round once a Quit has been asked for.
    static QUIT_WAKE: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Whether the app menu's Quit was turned into a question since this was last
/// asked; asking clears it.
pub fn take_quit_asked() -> bool {
    QUIT_ASKED.swap(false, Ordering::AcqRel)
}

/// Whether the app menu's Quit may end NewQ without a question now.
pub fn set_quit_freely(free: bool) {
    QUIT_FREELY.store(free, Ordering::Release);
}

/// The method's type: NSUInteger `applicationShouldTerminate:(id)sender`.
type ShouldTerminate = extern "C" fn(
    *mut objc2::runtime::AnyObject,
    objc2::runtime::Sel,
    *mut objc2::runtime::AnyObject,
) -> usize;

/// `-[NSApplicationDelegate applicationShouldTerminate:]`: straight through
/// when there is nothing to ask; else cancelled, and the question put up in
/// the window (the flag, and the event loop woken to see it).
extern "C" fn should_terminate(
    _this: *mut objc2::runtime::AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut objc2::runtime::AnyObject,
) -> usize {
    // NSTerminateCancel = 0, NSTerminateNow = 1.
    if QUIT_FREELY.load(Ordering::Acquire) {
        return 1;
    }
    QUIT_ASKED.store(true, Ordering::Release);
    QUIT_WAKE.with(|w| {
        if let Some(wake) = &*w.borrow() {
            wake();
        }
    });
    0
}

/// The app menu's Quit (Cmd+Q) and the Dock's send `terminate:`, which
/// winit's application delegate lets through at once: NewQ ended with cues
/// playing and changes unsaved, without a word. This gives winit's delegate
/// an `applicationShouldTerminate:` (it has none), which cancels the quit and
/// calls `wake` whenever the window would ask (`set_quit_freely`), for the app
/// to ask there. Once, on the main thread, after the event loop has started.
/// False if the delegate's class wasn't found or already had the method
/// (Quit then goes through at once, as before).
pub fn ask_before_quitting(wake: impl Fn() + 'static) -> bool {
    QUIT_WAKE.with(|w| *w.borrow_mut() = Some(Box::new(wake)));
    let Some(class) = objc2::runtime::AnyClass::get("WinitApplicationDelegate") else {
        return false;
    };
    let imp: ShouldTerminate = should_terminate;
    // SAFETY: the class is winit's, registered; the function has the method's
    // signature (NSUInteger return; self, _cmd, sender), which the type
    // encoding says; a method winit's class doesn't have changes nothing it does.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class as *const objc2::runtime::AnyClass as *mut objc2::ffi::objc_class,
            objc2::sel!(applicationShouldTerminate:).as_ptr(),
            Some(std::mem::transmute::<ShouldTerminate, unsafe extern "C" fn()>(imp)),
            c"Q@:@".as_ptr(),
        )
    };
    added != objc2::ffi::NO
}
