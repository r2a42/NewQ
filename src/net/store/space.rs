//! How much room a disk has.

use std::path::Path;
use std::sync::Arc;

/// Asks how many bytes can still be written on the disk that holds a path.
/// None when it can't be told (then nothing is refused for want of room).
/// Given to a store, so tests can pretend a disk is nearly full.
pub type FreeSpace = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// The real answer from the system.
pub fn system() -> FreeSpace {
    Arc::new(|p: &Path| free_bytes(p))
}

/// Free bytes for this user on the disk holding `path` (or the nearest folder
/// above it that exists).
pub fn free_bytes(path: &Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    os_free(p)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn os_free(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated string and `st` is written by
    // the call before it is read.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
}

#[cfg(windows)]
fn os_free(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and the out pointers are valid or null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(any(unix, windows)))]
fn os_free(_path: &Path) -> Option<u64> {
    None
}

/// Bytes as an artist reads them: "1.4 GB", "350 MB", "12 KB".
pub fn words(bytes: u64) -> String {
    const K: f64 = 1000.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.0} MB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temporary_folder_has_some_room() {
        let free = free_bytes(&std::env::temp_dir().join("not/there/yet"));
        assert!(free.is_some_and(|f| f > 0), "{free:?}");
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(words(12), "12 bytes");
        assert_eq!(words(12_000), "12 KB");
        assert_eq!(words(350_000_000), "350 MB");
        assert_eq!(words(1_400_000_000), "1.4 GB");
    }
}
