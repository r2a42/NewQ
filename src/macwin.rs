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
    NSApplicationDidChangeScreenParametersNotification, NSColor, NSScreen, NSView,
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
