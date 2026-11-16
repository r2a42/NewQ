//! Start at login (D8): NewQ writes its own entry so a display computer in a
//! truss comes back by itself after a power cut. A Mac LaunchAgent
//! (`~/Library/LaunchAgents/<LABEL>.plist`), the Windows `Run` key
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `NewQ`), or a
//! Linux autostart entry (`~/.config/autostart/newq.desktop`). The contents are
//! made by pure functions (tested); the writing was tried on Linux only.

use std::path::{Path, PathBuf};

/// The LaunchAgent's label (and file name).
#[cfg_attr(windows, allow(dead_code))]
pub const LABEL: &str = "org.newq.start-at-login";
/// The Windows Run value's name.
#[cfg_attr(not(windows), allow(dead_code))]
pub const RUN_VALUE: &str = "NewQ";

/// XML's special characters escaped.
#[cfg_attr(windows, allow(dead_code))]
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The Mac LaunchAgent for the program at `exe`.
#[cfg_attr(windows, allow(dead_code))]
pub fn launch_agent(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
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
pub fn desktop_entry(exe: &Path) -> String {
    // The Exec key's quoting: inside double quotes, `"`, `` ` ``, `$` and `\`
    // are escaped with a backslash.
    let mut quoted = String::new();
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    format!(
        "[Desktop Entry]\nType=Application\nName=NewQ\nComment=Started at login for a show network\nExec=\"{quoted}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// The Windows Run value for the program at `exe`: its path in quotes.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\"", exe.to_string_lossy())
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
    #[cfg(windows)]
    {
        windows::read().is_some()
    }
    #[cfg(not(windows))]
    {
        entry_path().is_some_and(|p| p.is_file())
    }
}

/// Start at login, or not.
pub fn set(on: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        if on {
            windows::write(&run_value(&exe()?))
        } else {
            windows::delete()
        }
    }
    #[cfg(not(windows))]
    {
        let path = entry_path().ok_or("no home folder")?;
        let exe = exe()?;
        let text = if cfg!(target_os = "macos") {
            launch_agent(&exe)
        } else {
            desktop_entry(&exe)
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

    pub fn read() -> Option<String> {
        let (key, name) = (wide(KEY), wide(super::RUN_VALUE));
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

    pub fn write(value: &str) -> Result<(), String> {
        let (key, name, data) = (wide(KEY), wide(super::RUN_VALUE), wide(value));
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

    pub fn delete() -> Result<(), String> {
        let (key, name) = (wide(KEY), wide(super::RUN_VALUE));
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
        let p = launch_agent(Path::new("/Applications/NewQ & Co.app/Contents/MacOS/newq"));
        assert!(p.contains("<string>/Applications/NewQ &amp; Co.app/Contents/MacOS/newq</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains(&format!("<string>{LABEL}</string>")));
        assert!(p.starts_with("<?xml"));
    }

    #[test]
    fn a_linux_entry_quotes_the_path() {
        let e = desktop_entry(Path::new("/opt/new q/$bin/newq"));
        assert!(e.contains("Exec=\"/opt/new q/\\$bin/newq\"\n"), "{e}");
        assert!(e.starts_with("[Desktop Entry]\nType=Application\n"));
    }

    #[test]
    fn the_windows_value_is_the_path_in_quotes() {
        assert_eq!(
            run_value(Path::new(r"C:\Program Files\NewQ\newq.exe")),
            r#""C:\Program Files\NewQ\newq.exe""#
        );
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
