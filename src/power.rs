//! Whether the computer is running on its battery.
//!
//! A laptop whose lid is closed keeps showing on an external display only
//! while it is on mains power (a Mac's "clamshell" mode); on battery it goes to
//! sleep and the show stops, and nothing a program can ask for prevents that.
//! So NewQ can only say so: a warning while outputs are open on battery.
//!
//! The question is asked on a thread of its own (a Mac's is answered by a
//! program that is run), every 20 seconds, and the main loop reads the answer.

use std::sync::Once;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const UNKNOWN: u8 = 0;
const MAINS: u8 = 1;
const BATTERY: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);
static STARTED: Once = Once::new();

/// How often the power source is looked at.
const EVERY: Duration = Duration::from_secs(20);

/// Start looking at the power source (once; later calls do nothing).
pub fn start_watching() {
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("power-watch".into())
            .spawn(|| {
                loop {
                    let state = match ask() {
                        Some(true) => BATTERY,
                        Some(false) => MAINS,
                        None => UNKNOWN,
                    };
                    STATE.store(state, Ordering::Release);
                    std::thread::sleep(EVERY);
                }
            });
    });
}

/// What the last look found: Some(true) on battery, Some(false) on mains,
/// None if not known (yet, or this computer can't say).
pub fn on_battery() -> Option<bool> {
    match STATE.load(Ordering::Acquire) {
        BATTERY => Some(true),
        MAINS => Some(false),
        _ => None,
    }
}

/// Read a Mac's `pmset -g ps`: its first line says "Now drawing from 'Battery
/// Power'" or "'AC Power'". None when it says neither (a desktop has no
/// battery and says "AC Power"; anything else is not understood).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_pmset(text: &str) -> Option<bool> {
    let first = text.lines().next()?;
    if first.contains("Battery Power") {
        Some(true)
    } else if first.contains("AC Power") {
        Some(false)
    } else {
        None
    }
}

/// Read what a Linux kernel says about its power supplies: on battery when
/// there is a battery and no mains supply is online. `supplies` is each
/// supply's (type, online) as the kernel writes them.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_supplies(supplies: &[(String, Option<bool>)]) -> Option<bool> {
    let has_battery = supplies.iter().any(|(t, _)| t == "Battery");
    let mains_online = supplies
        .iter()
        .any(|(t, on)| t == "Mains" && *on == Some(true));
    let mains_known = supplies.iter().any(|(t, on)| t == "Mains" && on.is_some());
    match (has_battery, mains_online, mains_known) {
        (false, _, _) => Some(false),
        (true, true, _) => Some(false),
        (true, false, true) => Some(true),
        (true, false, false) => None,
    }
}

#[cfg(target_os = "macos")]
fn ask() -> Option<bool> {
    let out = std::process::Command::new("/usr/bin/pmset")
        .args(["-g", "ps"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_pmset(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn ask() -> Option<bool> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: a plain query filling in a structure we own.
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return None;
    }
    match status.ACLineStatus {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn ask() -> Option<bool> {
    let mut supplies = Vec::new();
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let dir = entry.path();
        let kind = std::fs::read_to_string(dir.join("type"))
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        let online = std::fs::read_to_string(dir.join("online"))
            .ok()
            .map(|t| t.trim() == "1");
        supplies.push((kind, online));
    }
    parse_supplies(&supplies)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn ask() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_macs_power_source_is_read_from_the_first_line_of_pmset() {
        assert_eq!(
            parse_pmset(
                "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1) 54%; discharging"
            ),
            Some(true)
        );
        assert_eq!(
            parse_pmset("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1) 100%; charged"),
            Some(false)
        );
        // A desktop has no battery line at all.
        assert_eq!(parse_pmset("Now drawing from 'AC Power'\n"), Some(false));
        // 'UPS Power' or anything unexpected is not guessed at.
        assert_eq!(parse_pmset("Now drawing from 'UPS Power'\n"), None);
        assert_eq!(parse_pmset(""), None);
        // A later line mentioning the battery's source doesn't count.
        assert_eq!(
            parse_pmset("Now drawing from 'AC Power'\n battery at 'Battery Power' sign"),
            Some(false)
        );
    }

    #[test]
    fn a_linux_computer_is_on_battery_only_with_a_battery_and_no_mains() {
        let s = |t: &str, on: Option<bool>| (t.to_string(), on);
        assert_eq!(parse_supplies(&[]), Some(false), "a desktop");
        assert_eq!(
            parse_supplies(&[s("Mains", Some(true)), s("Battery", None)]),
            Some(false)
        );
        assert_eq!(
            parse_supplies(&[s("Mains", Some(false)), s("Battery", None)]),
            Some(true)
        );
        assert_eq!(parse_supplies(&[s("Battery", None)]), None, "can't tell");
    }
}
