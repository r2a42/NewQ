//! Start at login (D8): NewQ writes its own entry so a display computer in a
//! truss comes back by itself after a power cut. Only when someone asks for it
//! (the tick on the computer itself, or on its row on the primary), never by
//! itself. A Mac LaunchAgent (`~/Library/LaunchAgents/<LABEL>.plist`), the
//! Windows `Run` key (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//! value `NewQ Display` for a display computer, else `NewQ`; 0.24 always
//! wrote `NewQ`), or a Linux autostart entry
//! (`~/.config/autostart/newq.desktop`). The contents are made by pure
//! functions (tested); the writing was tried on Linux only.

use std::path::{Path, PathBuf};

/// The LaunchAgent's label (and file name).
#[cfg_attr(windows, allow(dead_code))]
pub const LABEL: &str = "org.newq.start-at-login";
/// The Windows Run value's name for a computer that isn't a display computer,
/// and the one NewQ 0.24 wrote for every computer.
pub const RUN_VALUE: &str = "NewQ";
/// The Windows Run value's name for a display computer (what Task Manager's
/// Startup apps lists).
pub const RUN_VALUE_DISPLAY: &str = "NewQ Display";

/// The entry's name where the system shows one (the Windows Run value, the
/// Linux entry's `Name`): "NewQ Display" for a display computer.
pub fn name(display: bool) -> &'static str {
    if display {
        RUN_VALUE_DISPLAY
    } else {
        RUN_VALUE
    }
}

/// The Windows Run values to write and to remove for this choice: on, the
/// one named for the role, the other removed (so a `NewQ` value 0.24 wrote is
/// never left behind); off, both removed. Pure, for the tests.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn run_values(on: bool, display: bool) -> (Option<&'static str>, &'static [&'static str]) {
    match (on, display) {
        (true, true) => (Some(RUN_VALUE_DISPLAY), &[RUN_VALUE]),
        (true, false) => (Some(RUN_VALUE), &[RUN_VALUE_DISPLAY]),
        (false, _) => (None, &[RUN_VALUE_DISPLAY, RUN_VALUE]),
    }
}

/// Whether an entry starts NewQ as a display computer (0.24.0 wrote one
/// without `--display`: `fix_old_entry`). Pure, for the tests.
pub fn starts_as_display(entry: &str) -> bool {
    entry.contains("--display")
}

/// Where the entry is on this kind of system and how to turn it off there
/// (`os` as `std::env::consts::OS`). Pure, for the tests.
pub fn where_words(os: &str, display: bool) -> String {
    let n = name(display);
    match os {
        "windows" => format!(
            "Windows lists it as \"{n}\" in Task Manager, under Startup apps. Untick it here to \
             remove it (turned off there, it stays ticked here but doesn't start)."
        ),
        "macos" => "macOS lists it in System Settings, General, Login Items, under Allow in the \
                    Background. Untick it here to remove it."
            .to_string(),
        _ => "It is the file newq.desktop in ~/.config/autostart. Untick it here to remove it."
            .to_string(),
    }
}

/// XML's special characters escaped.
#[cfg_attr(windows, allow(dead_code))]
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// What NewQ is started with at login: `--display` for a display computer
/// (started by NewQ Display, or made one by its primary), so it comes back as
/// one with nothing asked; nothing otherwise.
pub fn args(display: bool) -> &'static [&'static str] {
    if display { &["--display"] } else { &[] }
}

/// The Mac LaunchAgent for the program at `exe`.
#[cfg_attr(windows, allow(dead_code))]
pub fn launch_agent(exe: &Path, args: &[&str]) -> String {
    let more: String = args
        .iter()
        .map(|a| format!("\n\t\t<string>{}</string>", xml(a)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>{more}
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        xml(&exe.to_string_lossy())
    )
}

/// The Linux autostart entry for the program at `exe`.
#[cfg_attr(windows, allow(dead_code))]
pub fn desktop_entry(exe: &Path, args: &[&str]) -> String {
    // The Exec key's quoting: inside double quotes, `"`, `` ` ``, `$` and `\`
    // are escaped with a backslash.
    let mut quoted = String::new();
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    // Plain words (`--display`): no quoting needed.
    let more: String = args.iter().map(|a| format!(" {a}")).collect();
    let name = name(starts_as_display(&more));
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment=Started at login for a show network\nExec=\"{quoted}\"{more}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// The Windows Run value for the program at `exe`: its path in quotes.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn run_value(exe: &Path, args: &[&str]) -> String {
    let more: String = args.iter().map(|a| format!(" {a}")).collect();
    format!("\"{}\"{more}", exe.to_string_lossy())
}

/// Where the entry goes on this system (None on Windows: the registry).
#[cfg_attr(windows, allow(dead_code))]
fn entry_path() -> Option<PathBuf> {
    let home = crate::relink::home_dir()?;
    if cfg!(target_os = "macos") {
        Some(
            home.join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
        )
    } else if cfg!(windows) {
        None
    } else {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Some(config.join("autostart").join("newq.desktop"))
    }
}

/// The program to start: this one (on a Mac the executable inside the app).
fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("where NewQ is isn't known ({e})"))
}

/// Whether NewQ starts at login on this computer.
pub fn is_set() -> bool {
    entry().is_some()
}

/// The entry as written (the Run value's command, the file's text), if any.
pub fn entry() -> Option<String> {
    #[cfg(windows)]
    {
        windows::read(RUN_VALUE_DISPLAY).or_else(|| windows::read(RUN_VALUE))
    }
    #[cfg(not(windows))]
    {
        std::fs::read_to_string(entry_path()?).ok()
    }
}

/// Whether an entry found at start is to be written again as a display
/// computer's: there, on a display computer, and without `--display` (NewQ
/// 0.24.0 wrote it so, and a plain start is no longer display mode). Pure,
/// for the tests.
pub fn old_entry_to_fix(entry: Option<&str>, display_computer: bool) -> bool {
    display_computer && entry.is_some_and(|e| !starts_as_display(e))
}

/// An entry NewQ 0.24.0 wrote on a display computer, written again with
/// `--display` (`old_entry_to_fix`), so the next login starts display mode.
/// Nothing is switched on that wasn't.
pub fn fix_old_entry(display_computer: bool) {
    if !old_entry_to_fix(entry().as_deref(), display_computer) {
        return;
    }
    match set(true, true) {
        Ok(()) => log::info!("start at login: an older entry written again to start display mode"),
        Err(e) => log::warn!("start at login: an older entry couldn't be written again: {e}"),
    }
}

/// Start at login, or not (`display`: as a display computer, `args`).
pub fn set(on: bool, display: bool) -> Result<(), String> {
    let args = args(display);
    #[cfg(windows)]
    {
        let (write, remove) = run_values(on, display);
        if let Some(name) = write {
            windows::write(name, &run_value(&exe()?, args))?;
        }
        for name in remove.iter() {
            windows::delete(name)?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let path = entry_path().ok_or("no home folder")?;
        let exe = exe()?;
        let text = if cfg!(target_os = "macos") {
            launch_agent(&exe, args)
        } else {
            desktop_entry(&exe, args)
        };
        write_entry(&path, on.then_some(text.as_str()))
    }
}

/// Write the entry at `path` (None: remove it; not there is fine).
#[cfg_attr(windows, allow(dead_code))]
fn write_entry(path: &Path, text: Option<&str>) -> Result<(), String> {
    let Some(text) = text else {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read(name: &str) -> Option<String> {
        let (key, name) = (wide(KEY), wide(name));
        let mut buf = vec![0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: the strings are NUL-terminated and the buffer is `len` bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        (r == 0).then(|| {
            let n = (len as usize / 2).saturating_sub(1).min(buf.len());
            String::from_utf16_lossy(&buf[..n])
        })
    }

    pub fn write(name: &str, value: &str) -> Result<(), String> {
        let (key, name, data) = (wide(KEY), wide(name), wide(value));
        // SAFETY: NUL-terminated strings; `data` is the given number of bytes.
        let r = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(format!("the registry refused (error {r})"))
        }
    }

    pub fn delete(name: &str) -> Result<(), String> {
        let (key, name) = (wide(KEY), wide(name));
        // SAFETY: NUL-terminated strings.
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        // 2: it wasn't there.
        if r == 0 || r == 2 {
            Ok(())
        } else {
            Err(format!("the registry refused (error {r})"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_launch_agent_runs_newq_at_load() {
        let p = launch_agent(
            Path::new("/Applications/NewQ & Co.app/Contents/MacOS/newq"),
            &[],
        );
        assert!(p.contains(
            "<string>/Applications/NewQ &amp; Co.app/Contents/MacOS/newq</string>\n\t</array>"
        ));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains(&format!("<string>{LABEL}</string>")));
        assert!(p.starts_with("<?xml"));
    }

    #[test]
    fn a_linux_entry_quotes_the_path() {
        let e = desktop_entry(Path::new("/opt/new q/$bin/newq"), &[]);
        assert!(e.contains("Exec=\"/opt/new q/\\$bin/newq\"\n"), "{e}");
        assert!(e.starts_with("[Desktop Entry]\nType=Application\n"));
    }

    #[test]
    fn the_windows_value_is_the_path_in_quotes() {
        assert_eq!(
            run_value(Path::new(r"C:\Program Files\NewQ\newq.exe"), &[]),
            r#""C:\Program Files\NewQ\newq.exe""#
        );
    }

    #[test]
    fn a_display_computer_starts_at_login_as_one() {
        assert_eq!(args(true), ["--display"]);
        assert!(args(false).is_empty());
        let p = launch_agent(
            Path::new("/Applications/NewQ.app/Contents/MacOS/newq"),
            args(true),
        );
        assert!(
            p.contains(
                "<string>/Applications/NewQ.app/Contents/MacOS/newq</string>\n\t\t\
                 <string>--display</string>\n\t</array>"
            ),
            "{p}"
        );
        let e = desktop_entry(Path::new("/opt/new q/newq"), args(true));
        assert!(e.contains("Exec=\"/opt/new q/newq\" --display\n"), "{e}");
        assert_eq!(
            run_value(Path::new(r"C:\NewQ\newq.exe"), args(true)),
            r#""C:\NewQ\newq.exe" --display"#
        );
    }

    #[test]
    fn a_display_computers_entry_is_named_newq_display() {
        assert_eq!(name(true), "NewQ Display");
        assert_eq!(name(false), "NewQ");
        let e = desktop_entry(Path::new("/opt/newq"), args(true));
        assert!(e.contains("\nName=NewQ Display\n"), "{e}");
        let e = desktop_entry(Path::new("/opt/newq"), args(false));
        assert!(e.contains("\nName=NewQ\n"), "{e}");
    }

    /// Windows: switched on, the value named for the role is written and the
    /// other removed; off, both go (0.24's `NewQ` is never left behind).
    #[test]
    fn no_windows_value_is_left_behind() {
        assert_eq!(
            run_values(true, true),
            (Some("NewQ Display"), &["NewQ"][..])
        );
        assert_eq!(
            run_values(true, false),
            (Some("NewQ"), &["NewQ Display"][..])
        );
        for display in [true, false] {
            let (write, remove) = run_values(false, display);
            assert_eq!(write, None);
            assert!(remove.contains(&"NewQ") && remove.contains(&"NewQ Display"));
        }
    }

    /// 0.24.0 wrote a display computer's entry without `--display`; 0.24.1
    /// and later with it. Only the older one is written again, only on a
    /// display computer, and only if there is one.
    #[test]
    fn an_older_entry_on_a_display_computer_is_written_again_with_display() {
        let old = run_value(Path::new(r"C:\NewQ\newq.exe"), args(false));
        let new = run_value(Path::new(r"C:\NewQ\newq.exe"), args(true));
        assert!(old_entry_to_fix(Some(&old), true));
        assert!(!old_entry_to_fix(Some(&new), true));
        assert!(
            !old_entry_to_fix(Some(&old), false),
            "not a display computer"
        );
        assert!(!old_entry_to_fix(None, true), "never switched on by itself");
        let agent = launch_agent(Path::new("/Applications/NewQ.app/Contents/MacOS/newq"), &[]);
        assert!(old_entry_to_fix(Some(&agent), true));
        let entry = desktop_entry(Path::new("/opt/newq"), args(true));
        assert!(!old_entry_to_fix(Some(&entry), true));
    }

    #[test]
    fn the_page_says_where_the_entry_is_on_each_system() {
        let w = where_words("windows", true);
        assert!(
            w.contains("\"NewQ Display\"") && w.contains("Task Manager"),
            "{w}"
        );
        assert!(where_words("macos", true).contains("Login Items"));
        assert!(where_words("linux", false).contains("~/.config/autostart"));
    }

    /// Writing and removing the entry, in a folder of the test's own.
    #[test]
    fn the_entry_is_written_and_removed() {
        let dir = std::env::temp_dir().join(format!("newq-autostart-{}", std::process::id()));
        let path = dir.join("autostart/newq.desktop");
        write_entry(&path, Some("x")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
        write_entry(&path, None).unwrap();
        assert!(!path.exists());
        write_entry(&path, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
