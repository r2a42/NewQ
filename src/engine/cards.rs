//! In a show network, a cue whose file isn't on this computer yet (rulings
//! R13, R20): it plays all the same for its timing, with a card in its
//! picture's place ("Media transferring", or "Media not found") or, for a
//! sound, silence. About once a second the card is brought up to date; when
//! the file is here, whole and verified (`net::media::local_path`), the cue
//! takes it up where it is by then in its time, as a jump that keeps its
//! time. A partial file is never opened: only `Local::Here`'s path is.
//!
//! Alone or on the primary nothing here runs: every file is `Here`.

use super::{ActiveKind, Engine, PreparedKind, Seek};
use crate::media::ProbeState;
use crate::media::video::RgbaImage;
use crate::net::media::words::{self, Card};
use crate::net::media::{Finder, Local};
use crate::show::{CueKind, Fit, Show};
use crossbeam_channel::Receiver;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a card looks again at its file.
const CARD_EVERY: Duration = Duration::from_millis(1000);

/// How far ahead of the cue's time the decoders of a file that has arrived
/// are started, to have a frame by the time the cue gets there.
const TAKE_UP_LEAD: f64 = 0.25;

/// A cue showing a card (or silence) for a file on its way.
pub(super) struct CardState {
    /// The file as the show names it.
    pub file: PathBuf,
    /// The fit its picture has once the file is here (the card is stretched).
    pub fit: Fit,
    /// What the picture on screen says (and whether it is black).
    shown: Option<(Card, bool)>,
    looked: Option<Instant>,
    pending: Option<Receiver<Result<RgbaImage, String>>>,
}

impl CardState {
    pub fn new(file: PathBuf, fit: Fit) -> CardState {
        CardState {
            file,
            fit,
            shown: None,
            looked: Some(Instant::now()),
            pending: None,
        }
    }
}

/// Draw the card for `file` on a thread.
pub(super) fn draw_on_thread(
    file: &Path,
    local: &Local,
    size: (u32, u32),
    black: bool,
) -> Receiver<Result<RgbaImage, String>> {
    let text = crate::net::store::paths::path_text(file);
    let card = words::card(&text, local).unwrap_or_else(|| Card {
        heading: "Media transferring".into(),
        name: words::file_name(&text),
        detail: String::new(),
    });
    draw_card_on_thread(card, size, black)
}

fn draw_card_on_thread(
    card: Card,
    size: (u32, u32),
    black: bool,
) -> Receiver<Result<RgbaImage, String>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("media-card".into())
        .spawn(move || {
            let _ = tx.send(Ok(crate::net::media::card::draw(&card, size, black)));
        });
    if let Err(e) = spawned {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = tx.send(Err(format!("could not draw the card: {e}")));
        return rx;
    }
    rx
}

impl Engine {
    /// Use this finder rather than the app's (tests: a follower's engine).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_media_finder(&mut self, f: Option<Arc<dyn Finder>>) {
        self.media_finder = f.clone();
        self.probe.finder = f;
    }

    /// Where this computer has the file a cue names (`net::media::local_path`).
    pub(super) fn local_file(&self, p: &Path) -> Local {
        match self.media_finder.clone().or_else(crate::net::media::finder) {
            Some(f) => f.local(p),
            None => Local::Here(p.to_path_buf()),
        }
    }

    /// The cues showing a card now, with what it says (empty for a sound's
    /// silence): for the tests and the operator's log.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cards(&self) -> Vec<(crate::show::CueId, Option<Card>)> {
        self.active
            .iter()
            .filter_map(|a| {
                let c = a.card.as_ref()?;
                Some((a.cue_id, c.shown.as_ref().map(|s| s.0.clone())))
            })
            .collect()
    }

    /// Bring the cards up to date and take up the files that have arrived.
    pub(super) fn tend_cards(&mut self, show: &Show) {
        if !self.active.iter().any(|a| a.card.is_some()) {
            return;
        }
        let now = Instant::now();
        let black = !self.transfer_cards;
        let mut due: Vec<(u64, PathBuf, Option<crate::show::OutputId>)> = Vec::new();
        for a in &mut self.active {
            let Some(c) = a.card.as_mut() else {
                continue;
            };
            if let Some(rx) = &c.pending
                && let Ok(r) = rx.try_recv()
            {
                c.pending = None;
                if let (Ok(img), ActiveKind::Image(im)) = (r, &mut a.kind) {
                    im.image = Some(Arc::new(img));
                    im.version += 1;
                }
            }
            if a.stopping || a.seeking.is_some() || c.pending.is_some() {
                continue;
            }
            let output = match &a.kind {
                ActiveKind::Image(i) => i.output,
                _ => None,
            };
            if c.looked.is_none_or(|t| now.duration_since(t) >= CARD_EVERY) {
                due.push((a.instance, c.file.clone(), output));
            }
        }
        for (instance, file, output) in due {
            let local = self.local_file(&file);
            if local.is_here() {
                // Tried each tick until its decoders are under way.
                if !self.take_up(show, instance)
                    && let Some(c) = self.card_of(instance)
                {
                    c.looked = None;
                }
                continue;
            }
            let text = crate::net::store::paths::path_text(&file);
            let card = words::card(&text, &local);
            let size = self.output_size(output);
            let Some(a) = self.active.iter_mut().find(|a| a.instance == instance) else {
                continue;
            };
            let visual = matches!(a.kind, ActiveKind::Image(_));
            let Some(c) = a.card.as_mut() else {
                continue;
            };
            c.looked = Some(now);
            let Some(card) = card else {
                continue;
            };
            let key = (card.clone(), black);
            if c.shown.as_ref() != Some(&key) {
                c.shown = Some(key);
                if visual {
                    c.pending = Some(draw_card_on_thread(card, size, black));
                }
            }
        }
    }

    fn card_of(&mut self, instance: u64) -> Option<&mut CardState> {
        self.active
            .iter_mut()
            .find(|a| a.instance == instance)
            .and_then(|a| a.card.as_deref_mut())
    }

    /// The file of a cue showing a card is here: start its decoders where the
    /// cue will be in a moment (they are swapped in once they have a frame,
    /// the cue's time going on as it was). False if it must wait (its file
    /// is still being read).
    fn take_up(&mut self, show: &Show, instance: u64) -> bool {
        let Some(a) = self.active.iter().find(|a| a.instance == instance) else {
            return true;
        };
        let Some(cue) = show.cue(a.cue_id).cloned() else {
            return true;
        };
        let Some(m) = cue.kind.file_media() else {
            return true;
        };
        let ff = self.ff.clone();
        let state = match self.probe.get_urgent(ff.as_ref(), &m.file) {
            ProbeState::Ready(info) => Ok(info.clone()),
            ProbeState::Failed(e) => Err(Some(e.clone())),
            ProbeState::Pending => Err(None),
        };
        let info = match state {
            Ok(info) if self.probe.read_here(&m.file) => info,
            Err(Some(e)) => {
                self.problem(format!("Cue {} ({}): {e}", cue.number, cue.name));
                if let Some(c) = self.card_of(instance) {
                    c.looked = Some(Instant::now());
                }
                return true;
            }
            _ => return false,
        };
        let (elapsed, lead, duration) = (a.elapsed, a.lead, a.duration);
        let shift = if self.paused {
            elapsed
        } else {
            elapsed + TAKE_UP_LEAD
        };
        // Nearly over: the card sees it out.
        if duration.is_some_and(|d| shift >= d - 0.05) {
            if let Some(c) = self.card_of(instance) {
                c.looked = Some(Instant::now() + Duration::from_secs(3600));
            }
            return true;
        }
        let (shift, lead) = if matches!(cue.kind, CueKind::Image(_)) {
            (0.0, 0.0)
        } else {
            (shift, lead)
        };
        let p = match self.prepare_from(&cue, &info, shift, lead) {
            Ok(p) => p,
            Err(e) => {
                self.problem(format!("Cue {} ({}): {e}", cue.number, cue.name));
                if let Some(c) = self.card_of(instance) {
                    c.looked = Some(Instant::now());
                }
                return true;
            }
        };
        log::info!(
            "cue {} ({}): its file has arrived and is taken up at {shift:.2} s",
            cue.number,
            cue.name
        );
        let Some(a) = self.active.iter_mut().find(|a| a.instance == instance) else {
            return true;
        };
        match p.kind {
            PreparedKind::Image(loader) => {
                if let ActiveKind::Image(im) = &mut a.kind {
                    im.loader = Some(loader);
                    if let Some(c) = &a.card {
                        im.fit = c.fit;
                    }
                }
                a.card = None;
            }
            PreparedKind::Video { player, voice } => {
                a.seeking = Some(Seek {
                    player: Some(player),
                    voice,
                    base: shift,
                    file_from: p.file_from,
                    spawned_at: Instant::now(),
                    duration: None,
                    keep_time: true,
                });
                a.pass_len = p.pass_len;
                a.speed = p.speed;
            }
            PreparedKind::Audio(voice) => {
                a.seeking = Some(Seek {
                    player: None,
                    voice: Some(voice),
                    base: shift,
                    file_from: p.file_from,
                    spawned_at: Instant::now(),
                    duration: None,
                    keep_time: true,
                });
                a.pass_len = p.pass_len;
                a.speed = p.speed;
            }
            // Gone again meanwhile (a new version): the card stays.
            PreparedKind::Card(_) | PreparedKind::Silent | PreparedKind::Text(_) => {}
        }
        true
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
