//! Finding a show's media again after it has moved, and gathering it in one place.
//!
//! A show remembers where each media file was. Moved to another machine, or
//! with the media in a different folder, those places are gone. These
//! functions list what is missing, find it again by name in a folder you point
//! at, and copy a show's media into one folder so the show can travel.

use crate::show::{CueId, Show};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How deep into a folder the search goes.
const MAX_DEPTH: usize = 12;

/// Every cue whose media file isn't there, with where it was expected.
pub fn missing(show: &Show) -> Vec<(CueId, PathBuf)> {
    show.cues
        .iter()
        .filter_map(|c| {
            let file = &c.kind.file_media()?.file;
            (!file.as_os_str().is_empty() && !file.is_file()).then(|| (c.id, file.clone()))
        })
        .collect()
}

/// How many trailing path components two paths share (the file name counts).
fn shared_tail(a: &Path, b: &Path) -> usize {
    let parts = |p: &Path| -> Vec<OsString> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(n) => Some(n.to_os_string()),
                _ => None,
            })
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// All the files under `root`, as paths, in a stable order.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            // `metadata` follows links, which is what a shared media drive usually is.
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => files.push(path),
                Ok(m) if m.is_dir() && depth < MAX_DEPTH => stack.push((path, depth + 1)),
                _ => {}
            }
        }
    }
    files.sort();
    files
}

/// Look for the `wanted` files by name under `root`. Where a name turns up
/// more than once, the one whose folders match the original path best wins.
/// Returns what was found, by the path it was wanted at.
pub fn search(root: &Path, wanted: &[PathBuf]) -> HashMap<PathBuf, PathBuf> {
    let mut by_name: HashMap<OsString, Vec<PathBuf>> = HashMap::new();
    for f in walk(root) {
        if let Some(name) = f.file_name() {
            by_name.entry(name.to_os_string()).or_default().push(f);
        }
    }
    let mut found = HashMap::new();
    for want in wanted {
        let Some(candidates) = want.file_name().and_then(|n| by_name.get(n)) else {
            continue;
        };
        // Best match; ties go to the shallowest, then the first in order.
        let best = candidates.iter().max_by_key(|c| {
            (
                shared_tail(want, c),
                std::cmp::Reverse(c.components().count()),
            )
        });
        if let Some(best) = best {
            found.insert(want.clone(), best.clone());
        }
    }
    found
}

/// Point every cue that used a file at its new place. Returns how many cues changed.
pub fn apply(show: &mut Show, moved: &HashMap<PathBuf, PathBuf>) -> usize {
    let mut n = 0;
    for cue in &mut show.cues {
        if let Some(m) = cue.kind.media_mut()
            && let Some(new) = moved.get(&m.file)
        {
            m.file = new.clone();
            n += 1;
        }
    }
    n
}

/// What copying a show's media together did.
#[derive(Debug, Default, PartialEq)]
pub struct Collected {
    /// Old path → where the file is now (in the folder).
    pub moved: HashMap<PathBuf, PathBuf>,
    pub copied: usize,
    /// Files that were already in the folder.
    pub already_there: usize,
    /// Files that were missing, or couldn't be copied, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// Where each file goes in `dest`: its own name, or `name_2.ext`, `name_3.ext`…
/// when another file has taken it. The same source always gets the same place.
pub fn plan_collect(dest: &Path, files: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let mut taken: HashMap<PathBuf, PathBuf> = HashMap::new(); // target → source
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
    for src in files {
        if plan.iter().any(|(s, _)| s == src) {
            continue;
        }
        let name = src.file_name().map(PathBuf::from).unwrap_or_default();
        let stem = name
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = name
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut n = 1;
        let target = loop {
            let candidate = if n == 1 {
                dest.join(&name)
            } else {
                dest.join(format!("{stem}_{n}{ext}"))
            };
            match taken.get(&candidate) {
                // A file that is already in the folder keeps its place.
                None => break candidate,
                Some(owner) if owner == src => break candidate,
                Some(_) => n += 1,
            }
        };
        taken.insert(target.clone(), src.clone());
        plan.push((src.clone(), target));
    }
    plan
}

/// Copy the media of the cues in `files` into `dest` (made if needed).
/// `progress` counts files done, for a progress bar.
pub fn collect(dest: &Path, files: &[PathBuf], progress: &Arc<AtomicUsize>) -> Collected {
    let mut out = Collected::default();
    if let Err(e) = std::fs::create_dir_all(dest) {
        out.failed
            .extend(files.iter().map(|f| (f.clone(), format!("{e}"))));
        return out;
    }
    let dest_real = std::fs::canonicalize(dest).unwrap_or_else(|_| dest.to_path_buf());
    let mut unique: Vec<PathBuf> = Vec::new();
    for f in files {
        if !unique.contains(f) {
            unique.push(f.clone());
        }
    }
    // A file already in the folder stays where it is; the others are placed around it.
    let (inside, outside): (Vec<_>, Vec<_>) = unique.iter().cloned().partition(|f| {
        std::fs::canonicalize(f).is_ok_and(|c| c.parent() == Some(dest_real.as_path()))
    });
    for _ in &inside {
        out.already_there += 1;
        progress.fetch_add(1, Ordering::Relaxed);
    }
    let mut all = inside.clone();
    all.extend(outside.iter().cloned());
    for (src, target) in plan_collect(dest, &all) {
        if inside.contains(&src) {
            continue;
        }
        if !src.is_file() {
            out.failed.push((src, "the file is missing".into()));
        } else {
            match std::fs::copy(&src, &target) {
                Ok(_) => {
                    out.copied += 1;
                    out.moved.insert(src, target);
                }
                Err(e) => out.failed.push((src, e.to_string())),
            }
        }
        progress.fetch_add(1, Ordering::Relaxed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{CueKind, CueType};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_relink_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path, content: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn show_with(files: &[PathBuf]) -> Show {
        let mut show = Show::default();
        for f in files {
            let c = show.make_cue(CueType::Video, Some(f.clone()), show.cues.len());
            show.cues.push(c);
        }
        let n = show.make_cue(CueType::Wait, None, show.cues.len());
        show.cues.push(n);
        show
    }

    fn file_of(show: &Show, i: usize) -> PathBuf {
        show.cues[i].kind.file_media().unwrap().file.clone()
    }

    #[test]
    fn lists_the_cues_whose_files_are_gone() {
        let d = dir("missing");
        touch(&d.join("here.mp4"), "x");
        let show = show_with(&[d.join("here.mp4"), d.join("gone.mp4")]);
        let m = missing(&show);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, show.cues[1].id);
        assert_eq!(m[0].1, d.join("gone.mp4"));
        // A cue with no file yet isn't "missing"; it just isn't set up.
        let mut show = show;
        let blank = show.make_cue(CueType::Video, None, 0);
        show.cues.push(blank);
        assert_eq!(missing(&show).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_files_by_name_in_a_new_place() {
        let d = dir("search");
        touch(&d.join("new/media/act1/intro.mp4"), "x");
        touch(&d.join("new/media/act2/outro.mp4"), "x");
        touch(&d.join("new/media/tone.wav"), "x");
        let wanted = vec![
            PathBuf::from("/old/show/act1/intro.mp4"),
            PathBuf::from("/old/show/act2/outro.mp4"),
            PathBuf::from("/old/tone.wav"),
            PathBuf::from("/old/nowhere.mp4"),
        ];
        let found = search(&d.join("new"), &wanted);
        assert_eq!(found.len(), 3);
        assert_eq!(found[&wanted[0]], d.join("new/media/act1/intro.mp4"));
        assert_eq!(found[&wanted[1]], d.join("new/media/act2/outro.mp4"));
        assert_eq!(found[&wanted[2]], d.join("new/media/tone.wav"));
        assert!(!found.contains_key(&wanted[3]), "not found stays not found");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_same_name_in_two_folders_picks_the_better_match() {
        let d = dir("ambiguous");
        touch(&d.join("a/act1/loop.mp4"), "1");
        touch(&d.join("a/act2/loop.mp4"), "2");
        touch(&d.join("b/loop.mp4"), "3");
        let want1 = PathBuf::from("/was/act2/loop.mp4");
        let want2 = PathBuf::from("/was/elsewhere/loop.mp4");
        let found = search(&d, &[want1.clone(), want2.clone()]);
        assert_eq!(
            found[&want1],
            d.join("a/act2/loop.mp4"),
            "the folder name matches"
        );
        // No folder matches: the shallowest wins, not an arbitrary one.
        assert_eq!(found[&want2], d.join("b/loop.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn relinking_changes_every_cue_that_used_the_file() {
        let d = dir("apply");
        touch(&d.join("new.mp4"), "x");
        let old = PathBuf::from("/old/a.mp4");
        let mut show = show_with(&[old.clone(), old.clone(), PathBuf::from("/old/b.mp4")]);
        let moved = HashMap::from([(old.clone(), d.join("new.mp4"))]);
        assert_eq!(apply(&mut show, &moved), 2);
        assert_eq!(file_of(&show, 0), d.join("new.mp4"));
        assert_eq!(file_of(&show, 1), d.join("new.mp4"));
        assert_eq!(file_of(&show, 2), PathBuf::from("/old/b.mp4"));
        assert_eq!(missing(&show).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collecting_gives_clashing_names_a_number_and_shares_repeats() {
        let dest = Path::new("/dest");
        let files = vec![
            PathBuf::from("/a/clip.mp4"),
            PathBuf::from("/b/clip.mp4"),
            PathBuf::from("/a/clip.mp4"),
            PathBuf::from("/c/clip.mp4"),
            PathBuf::from("/a/tone.wav"),
        ];
        let plan = plan_collect(dest, &files);
        assert_eq!(
            plan,
            vec![
                (
                    PathBuf::from("/a/clip.mp4"),
                    PathBuf::from("/dest/clip.mp4")
                ),
                (
                    PathBuf::from("/b/clip.mp4"),
                    PathBuf::from("/dest/clip_2.mp4")
                ),
                (
                    PathBuf::from("/c/clip.mp4"),
                    PathBuf::from("/dest/clip_3.mp4")
                ),
                (
                    PathBuf::from("/a/tone.wav"),
                    PathBuf::from("/dest/tone.wav")
                ),
            ]
        );
    }

    #[test]
    fn collecting_copies_the_files_and_reports_what_it_could_not() {
        let d = dir("collect");
        touch(&d.join("x/clip.mp4"), "one");
        touch(&d.join("y/clip.mp4"), "two");
        touch(&d.join("y/tone.wav"), "tone");
        let dest = d.join("show/media");
        let files = vec![
            d.join("x/clip.mp4"),
            d.join("y/clip.mp4"),
            d.join("y/tone.wav"),
            d.join("y/tone.wav"),
            d.join("lost.mp4"),
        ];
        let progress = Arc::new(AtomicUsize::new(0));
        let got = collect(&dest, &files, &progress);
        assert_eq!(got.copied, 3);
        assert_eq!(got.failed.len(), 1);
        assert_eq!(got.failed[0].0, d.join("lost.mp4"));
        assert_eq!(
            std::fs::read_to_string(dest.join("clip.mp4")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("clip_2.mp4")).unwrap(),
            "two"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("tone.wav")).unwrap(),
            "tone"
        );
        assert_eq!(got.moved[&d.join("y/clip.mp4")], dest.join("clip_2.mp4"));
        assert_eq!(progress.load(Ordering::Relaxed), 4, "one per distinct file");
        // The originals are left alone.
        assert!(d.join("x/clip.mp4").is_file());

        // The show's cues now point into the folder, and nothing is missing.
        let mut show = show_with(&files[..4]);
        assert_eq!(apply(&mut show, &got.moved), 4);
        assert!(missing(&show).is_empty());
        // Doing it again changes nothing: the files are already there.
        let again = collect(
            &dest,
            &[dest.join("clip.mp4"), dest.join("clip_2.mp4")],
            &Arc::new(AtomicUsize::new(0)),
        );
        assert_eq!((again.copied, again.already_there), (0, 2));
        assert!(again.moved.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_name_that_collides_with_one_already_in_the_folder_is_renamed() {
        let d = dir("collide");
        touch(&d.join("src/clip.mp4"), "incoming");
        touch(&d.join("dest/clip.mp4"), "already here");
        let got = collect(
            &d.join("dest"),
            &[d.join("src/clip.mp4"), d.join("dest/clip.mp4")],
            &Arc::new(AtomicUsize::new(0)),
        );
        assert_eq!((got.copied, got.already_there), (1, 1));
        assert_eq!(
            std::fs::read_to_string(d.join("dest/clip.mp4")).unwrap(),
            "already here",
            "an existing file is never overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("dest/clip_2.mp4")).unwrap(),
            "incoming"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_cues_with_files_are_considered() {
        let show = show_with(&[]);
        assert!(missing(&show).is_empty());
        assert!(matches!(show.cues[0].kind, CueKind::Wait(_)));
    }
}
