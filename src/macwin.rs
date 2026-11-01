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

use objc2_app_kit::{NSScreen, NSView, NSWindowCollectionBehavior};
use winit::monitor::MonitorHandle;
use winit::platform::macos::MonitorHandleExtMacOS;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The menu bar's window level is 24 (`NSMainMenuWindowLevel`); the Dock's is
/// lower. One above covers both.
const ABOVE_THE_MENU_BAR: isize = 25;

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
