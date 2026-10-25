//! Describing displays and placing windows on them.

use winit::dpi::{LogicalPosition, PhysicalPosition};
use winit::monitor::MonitorHandle;

/// What the system calls this display (the model: "Built-in Retina Display",
/// "SAMSUNG"), where it can say. winit's own name for a display on a Mac is
/// "Monitor #" and a number, which tells nobody anything.
pub fn model_name(m: &MonitorHandle) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSScreen;
        use winit::platform::macos::MonitorHandleExtMacOS;
        let screen = m.ns_screen()?;
        // SAFETY: winit hands out a pointer to the live NSScreen behind this
        // monitor, and displays are only scanned on the main thread.
        let screen: &NSScreen = unsafe { &*(screen as *const NSScreen) };
        let name = unsafe { screen.localizedName() }.to_string();
        Some(name).filter(|n| !n.trim().is_empty())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = m;
        None
    }
}

/// Where a display's top left corner is, as a window position. winit reports
/// it in that display's own pixels, but a window position given in pixels is
/// taken in the pixels of the display the window starts on: with displays of
/// different scales (a Retina laptop beside a TV) the window would start on
/// the wrong one. Logical points are the same on all of them.
pub fn window_origin(position: PhysicalPosition<i32>, scale: f64) -> LogicalPosition<f64> {
    position.to_logical(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_displays_origin_is_the_same_in_points_whatever_its_scale() {
        // A TV at 1x placed right of a 1512-point laptop screen...
        let tv = window_origin(PhysicalPosition::new(1512, 0), 1.0);
        // ...and the same place reported by a 2x display.
        let retina = window_origin(PhysicalPosition::new(3024, 0), 2.0);
        assert_eq!((tv.x, tv.y), (1512.0, 0.0));
        assert_eq!(tv, retina);
        // Negative origins (a display to the left or above) survive.
        assert_eq!(
            window_origin(PhysicalPosition::new(-1920, -540), 1.0),
            LogicalPosition::new(-1920.0, -540.0)
        );
    }
}
