//! Describing displays and placing windows on them.

use winit::dpi::{LogicalPosition, PhysicalPosition};
use winit::monitor::MonitorHandle;

/// What a monitor is, as far as the system will say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    /// What to call it ("SAMSUNG", "Built-in Retina Display").
    pub title: Option<String>,
    /// What stays the same for this monitor whichever port it is on and
    /// whatever else is plugged in: its make and model, and its serial number
    /// where it has one.
    pub hardware: Option<String>,
}

/// Ask the system what a display is. winit's own name for it follows the
/// port or the order the system found it in ("Monitor #" and a number on a
/// Mac, `\\.\DISPLAY2` on Windows), which tells nobody anything and isn't the
/// same monitor after a reconnect.
pub fn identify(m: &MonitorHandle) -> Identity {
    #[cfg(target_os = "macos")]
    {
        mac_identity(m)
    }
    #[cfg(windows)]
    {
        let name = m.name().unwrap_or_default();
        windows_edid(&name)
            .and_then(|b| crate::edid::parse(&b))
            .map_or_else(Identity::default, |e| Identity {
                title: Some(e.title()),
                hardware: Some(e.hardware_id()),
            })
    }
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    {
        let name = m.name().unwrap_or_default();
        linux_edid(&name)
            .and_then(|b| crate::edid::parse(&b))
            .map_or_else(Identity::default, |e| Identity {
                title: Some(e.title()),
                hardware: Some(e.hardware_id()),
            })
    }
}

#[cfg(target_os = "macos")]
fn mac_identity(m: &MonitorHandle) -> Identity {
    use objc2_app_kit::NSScreen;
    use winit::platform::macos::MonitorHandleExtMacOS;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayVendorNumber(display: u32) -> u32;
        fn CGDisplayModelNumber(display: u32) -> u32;
        fn CGDisplaySerialNumber(display: u32) -> u32;
    }

    let title = m.ns_screen().and_then(|screen| {
        // SAFETY: winit hands out a pointer to the live NSScreen behind this
        // monitor, and displays are only scanned on the main thread.
        let screen: &NSScreen = unsafe { &*(screen as *const NSScreen) };
        let name = unsafe { screen.localizedName() }.to_string();
        Some(name).filter(|n| !n.trim().is_empty())
    });
    let id = m.native_id();
    // SAFETY: plain queries of a display by its id.
    let (vendor, model, serial) = unsafe {
        (
            CGDisplayVendorNumber(id),
            CGDisplayModelNumber(id),
            CGDisplaySerialNumber(id),
        )
    };
    Identity {
        title,
        hardware: mac_hardware_id(vendor, model, serial),
    }
}

/// A Mac's make, model and serial number for a display as one id; None when
/// it knows none of them (a virtual display).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_hardware_id(vendor: u32, model: u32, serial: u32) -> Option<String> {
    // 0xFFFFFFFF is what it answers when it doesn't know.
    let unknown = |v: u32| v == 0 || v == u32::MAX;
    if unknown(vendor) && unknown(model) {
        return None;
    }
    let base = format!("{vendor:04X}:{model:04X}");
    Some(if unknown(serial) {
        base
    } else {
        format!("{base}:{serial:08X}")
    })
}

/// The EDID Windows holds for the monitor on the adapter output named
/// `device` (`\\.\DISPLAY2`), from the registry.
#[cfg(windows)]
fn windows_edid(device: &str) -> Option<Vec<u8>> {
    use std::iter::once;
    use windows_sys::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    /// Asks for the device interface path in `DeviceID` (EDD_GET_DEVICE_INTERFACE_NAME).
    const GET_DEVICE_INTERFACE_NAME: u32 = 1;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(once(0)).collect() };
    // SAFETY: a zeroed DISPLAY_DEVICEW with its size filled in is what the call wants.
    let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
    dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
    let device = wide(device);
    // The first monitor on that output, named by its device interface path:
    // `\\?\DISPLAY#SAM0F6B#5&1d8cd1c&0&UID4353#{guid}`.
    // SAFETY: `device` is nul-terminated and `dd` is a valid out parameter.
    if unsafe { EnumDisplayDevicesW(device.as_ptr(), 0, &mut dd, GET_DEVICE_INTERFACE_NAME) } == 0 {
        return None;
    }
    let len = dd
        .DeviceID
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(dd.DeviceID.len());
    let path = String::from_utf16_lossy(&dd.DeviceID[..len]);
    let key = registry_key_for_interface(&path)?;
    let key = wide(&key);
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: `key` is nul-terminated; `hkey` is closed below.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
        return None;
    }
    let value = wide("EDID");
    let mut buf = vec![0u8; 512];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` and `size` describe the same memory.
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            value.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut size,
        )
    };
    // SAFETY: opened above.
    unsafe { RegCloseKey(hkey) };
    (status == 0).then(|| {
        buf.truncate(size as usize);
        buf
    })
}

/// The registry key holding a monitor's EDID, from its device interface
/// path (`\\?\DISPLAY#MODEL#INSTANCE#{guid}`).
#[cfg_attr(not(windows), allow(dead_code))]
fn registry_key_for_interface(path: &str) -> Option<String> {
    let mut parts = path.trim_start_matches(r"\\?\").split('#');
    let (class, model, instance) = (parts.next()?, parts.next()?, parts.next()?);
    if !class.eq_ignore_ascii_case("DISPLAY") || model.is_empty() || instance.is_empty() {
        return None;
    }
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{model}\{instance}\Device Parameters"
    ))
}

/// The EDID the kernel holds for a connector called `name`. X names them
/// "HDMI-1" and "DP-1"; the kernel (and Wayland) "HDMI-A-1" and "DP-1".
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn linux_edid(name: &str) -> Option<Vec<u8>> {
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        // "card0-HDMI-A-1": the card, then the connector.
        let Some((_, connector)) = file.split_once('-') else {
            continue;
        };
        if same_connector(connector, name)
            && let Ok(bytes) = std::fs::read(entry.path().join("edid"))
            && !bytes.is_empty()
        {
            return Some(bytes);
        }
    }
    None
}

#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn same_connector(kernel: &str, x: &str) -> bool {
    kernel == x || kernel.replacen("-A-", "-", 1) == x
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
    fn a_connector_is_the_same_under_the_kernels_and_xs_names() {
        assert!(same_connector("HDMI-A-1", "HDMI-1"));
        assert!(same_connector("HDMI-A-1", "HDMI-A-1"));
        assert!(same_connector("DP-2", "DP-2"));
        assert!(same_connector("eDP-1", "eDP-1"));
        assert!(!same_connector("HDMI-A-1", "HDMI-2"));
        assert!(!same_connector("DP-1", "DP-2"));
    }

    #[test]
    fn windows_edids_are_found_from_the_device_interface_path() {
        assert_eq!(
            registry_key_for_interface(
                r"\\?\DISPLAY#SAM0F6B#5&1d8cd1c&0&UID4353#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM0F6B\5&1d8cd1c&0&UID4353\Device Parameters"
            )
        );
        assert_eq!(registry_key_for_interface(r"\\?\USB#VID_1#2#{x}"), None);
        assert_eq!(registry_key_for_interface("nonsense"), None);
    }

    #[test]
    fn a_macs_vendor_model_and_serial_make_one_id() {
        assert_eq!(
            mac_hardware_id(0x4C2D, 0x0F6B, 0x1234).as_deref(),
            Some("4C2D:0F6B:00001234")
        );
        assert_eq!(
            mac_hardware_id(0x4C2D, 0x0F6B, 0).as_deref(),
            Some("4C2D:0F6B")
        );
        assert_eq!(
            mac_hardware_id(0, 0, 0),
            None,
            "it knows nothing of this one"
        );
        assert_eq!(mac_hardware_id(u32::MAX, u32::MAX, u32::MAX), None);
    }

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
