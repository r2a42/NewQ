//! Show paths as text that any computer can compare.
//!
//! The primary's show names its media by paths on its own disk, which may be a
//! Windows disk (`C:\Show\clip.mov`) while the receiver is a Mac, or the
//! reverse. A path is therefore kept as the text the show has, and compared by
//! a key: separators as `/`, no doubled ones, the drive letter in capitals.

use std::path::Path;

/// The text a path is kept as in a manifest: the show's own spelling.
pub fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The key two spellings of the same show path share.
pub fn path_key(text: &str) -> String {
    let unified = text.replace('\\', "/");
    // A UNC path (`\\server\share`) keeps its leading pair.
    let (lead, rest) = match unified.strip_prefix("//") {
        Some(r) => ("//", r),
        None => ("", unified.as_str()),
    };
    let mut out = String::with_capacity(unified.len());
    out.push_str(lead);
    let mut last_slash = false;
    for ch in rest.chars() {
        if ch == '/' {
            if last_slash {
                continue;
            }
            last_slash = true;
        } else {
            last_slash = false;
        }
        out.push(ch);
    }
    while out.len() > 1 && out.ends_with('/') && !out.ends_with(":/") {
        out.pop();
    }
    // `c:/x` and `C:/x` are one place on Windows.
    let b = out.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        out.replace_range(0..1, &(b[0] as char).to_ascii_uppercase().to_string());
    }
    out
}

/// The key for a lookup that ignores letter case (Windows and a Mac's usual
/// disks don't tell `Clip.MOV` from `clip.mov`).
pub fn loose_key(text: &str) -> String {
    path_key(text).to_lowercase()
}

/// The parts of a path, as the key has them (no empty ones).
pub fn parts(text: &str) -> Vec<String> {
    path_key(text)
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The file's extension, lower case, letters and digits only (FFmpeg and the
/// picture decoders look at it). Empty when there is none worth keeping.
pub fn extension(text: &str) -> String {
    let key = path_key(text);
    let name = key.rsplit('/').next().unwrap_or("");
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return String::new();
    };
    if stem.is_empty() || ext.is_empty() || ext.len() > 12 {
        return String::new();
    }
    if !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return String::new();
    }
    ext.to_ascii_lowercase()
}

/// The path of `file` under `dir`, with `/` between the parts, when it is
/// under it (both as the show spells them).
pub fn relative_to(file: &str, dir: &str) -> Option<String> {
    let f = parts(file);
    let d = parts(dir);
    if d.is_empty() || f.len() <= d.len() {
        return None;
    }
    // The leading `/` of a Unix path is not a part; it must match too.
    if path_key(file).starts_with('/') != path_key(dir).starts_with('/') {
        return None;
    }
    if f[..d.len()]
        .iter()
        .zip(&d)
        .any(|(a, b)| a.to_lowercase() != b.to_lowercase())
    {
        return None;
    }
    Some(f[d.len()..].join("/"))
}

/// Whether `s` is a SHA-256 written as 64 lower-case hex digits (what object
/// and manifest names are; anything else from the network is refused).
pub fn is_sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_join_the_spellings_of_one_path() {
        assert_eq!(
            path_key(r"c:\Show\Act 1\clip.MOV"),
            "C:/Show/Act 1/clip.MOV"
        );
        assert_eq!(
            path_key("C:/Show//Act 1/clip.MOV"),
            "C:/Show/Act 1/clip.MOV"
        );
        assert_eq!(path_key("/Users/me/Show/"), "/Users/me/Show");
        assert_eq!(path_key(r"\\server\share\a.mp4"), "//server/share/a.mp4");
        assert_eq!(path_key("/"), "/");
        assert_eq!(path_key("C:/"), "C:/");
        assert_eq!(
            loose_key(r"C:\Show\Clip.MOV"),
            loose_key("c:/show/clip.mov")
        );
    }

    #[test]
    fn extensions_are_kept_lower_case_and_plain() {
        assert_eq!(extension(r"C:\Show\clip.MOV"), "mov");
        assert_eq!(extension("/a/b.c/noext"), "");
        assert_eq!(extension("/a/.hidden"), "");
        assert_eq!(extension("/a/x.tar.gz"), "gz");
        assert_eq!(extension("/a/x.m p4"), "");
        assert_eq!(extension("/a/fête.wav"), "wav");
    }

    #[test]
    fn a_path_under_the_show_folder_has_a_relative_form() {
        assert_eq!(
            relative_to("/Users/me/Show/media/a b.mp4", "/Users/me/Show"),
            Some("media/a b.mp4".into())
        );
        assert_eq!(
            relative_to(r"C:\Show\media\a.mp4", r"c:\show"),
            Some("media/a.mp4".into())
        );
        assert_eq!(relative_to("/elsewhere/a.mp4", "/Users/me/Show"), None);
        assert_eq!(relative_to("/Users/me/Show", "/Users/me/Show"), None);
        assert_eq!(relative_to("C:/Show/a.mp4", "/Show"), None);
    }

    #[test]
    fn only_hex_digests_are_names() {
        assert!(is_sha(&"a".repeat(64)));
        assert!(!is_sha(&"A".repeat(64)));
        assert!(!is_sha("../../etc/passwd"));
        assert!(!is_sha(&"a".repeat(63)));
    }
}
