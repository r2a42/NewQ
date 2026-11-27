//! Line-up across the show network, in the app (mapping plan 4.4,
//! `net::lineup`): the primary's `Sender` for the draft the line-up panes
//! keep (`UiState::line_up`), this computer's `Shown`, and the crosses'
//! group drawn last on the projector's monitor.
//!
//! One path alone and networked: when the projector's display is on this
//! computer the sender's messages go straight into this computer's own
//! `Shown` (`Source::Here`) and its `Seen` straight back; otherwise they are
//! told to the computer that drives it, urgent, and its `Seen` comes back
//! through `play_message`.
//!
//! Trust (`lineup::route`): `Show` and `End` are taken only from the primary
//! this computer follows (play_message's `primary`), for a display that is
//! on here; `Seen` only on a primary, and only for its sender's session and
//! computer. The crosses go when the lease runs out (`lineup::LEASE`), and at
//! once when this computer stops following the primary that sent them.

use super::App;
use super::play::mode_of;
use crate::engine::NetMode;
use crate::gpu::{self, Geometry, LayerTexture};
use crate::net::lineup::{self, LineUpMsg, Overlay, Route, Sender, Shown, Source, Target, Under};
use crate::net::node::NodeId;
use crate::net::proto::Msg;
use crate::render::{OutputGroup, OutputLayer};
use crate::show::{ColorAdjust, Fit, OutputId};
use crate::stage3d::lineup_overlay::{self, Look};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the loop comes round while a line-up is on (the lease, the
/// sender's repeat).
const LINE_UP_TICK: Duration = Duration::from_millis(100);

/// The words when the projector's display is this computer's but has no
/// window here (no monitor chosen, or it is switched off).
pub(super) fn no_window_here() -> String {
    format!(
        "This projector's display isn't shown on a monitor of this computer, so there is \
         nowhere to show the crosses: choose its monitor on {}.",
        crate::ui::settings_path(crate::ui::SettingsPage::Displays, None)
    )
}

/// What is left of the picture under the crosses when it is dimmed.
pub(super) const DIM: f32 = 0.3;

/// The app's line-up state.
#[derive(Default)]
pub(super) struct LineUpState {
    /// What this computer shows on its projector.
    pub shown: Shown,
    /// The primary's line-up in progress, if any.
    pub sender: Option<Sender>,
    /// One texture per cross's number and look, made once.
    sprites: HashMap<(u16, Look), Arc<LayerTexture>>,
    /// The outline's picture: for which count and size.
    outline: Option<(u64, (u32, u32), Arc<LayerTexture>)>,
    /// What each monitor last drew of it (`Shown::stamp`): a change is a
    /// redraw, though nothing plays.
    drawn: HashMap<usize, Option<(u64, u64, u64)>>,
}

impl App {
    /// The size of the monitor display `id` is shown on here, if it is one of
    /// this computer's whole displays with a window.
    fn line_up_monitor(&self, id: OutputId) -> Option<(u32, u32)> {
        let o = self.show.output(id)?;
        if !o.is_on(self.ui.net.me.id) || o.is_sub_display() {
            return None;
        }
        let t = self.targets.get(&id)?;
        self.screens.get(&t.monitor).map(|s| s.size)
    }

    /// A line-up message from computer `from` (`primary`: the primary this
    /// computer follows, as `play_message` has it).
    pub(super) fn line_up_message(&mut self, from: NodeId, m: LineUpMsg, primary: bool) {
        let now = Instant::now();
        let leading = mode_of(&self.ui.net) == NetMode::Primary;
        match lineup::route(&m, primary, leading) {
            Route::Shown => {
                let size = match &m {
                    LineUpMsg::Show(o) => self.line_up_monitor(o.display),
                    _ => None,
                };
                // A display of this computer's with no window here: said
                // back as seen on no monitor, so the primary doesn't take this
                // computer for an older NewQ.
                let unseen = match &m {
                    LineUpMsg::Show(o) if size.is_none() => self
                        .show
                        .output(o.display)
                        .filter(|d| d.is_on(self.ui.net.me.id))
                        .map(|_| LineUpMsg::Seen {
                            session: o.session,
                            seq: o.seq,
                            pixels: (0, 0),
                        }),
                    _ => None,
                };
                let seen = self
                    .line_up
                    .shown
                    .take(Source::Primary(from), m, |_| size, now)
                    .or(unseen);
                if let Some(seen) = seen {
                    self.net.tell(Some(from), Msg::LineUp(Box::new(seen)), true);
                }
            }
            Route::Sender => {
                if let Some(s) = self.line_up.sender.as_mut() {
                    s.heard(Target::Computer(from), &m, now);
                }
            }
            Route::Pass => {
                if m == LineUpMsg::Unknown {
                    log::debug!("line-up: a message from a later NewQ, not known here");
                }
            }
        }
    }

    /// Hand `msg` to the crosses' computer: this one's own `Shown`, or told.
    fn line_up_deliver(&mut self, target: Target, msg: LineUpMsg, now: Instant) {
        match target {
            Target::Here => {
                let size = match &msg {
                    LineUpMsg::Show(o) => self.line_up_monitor(o.display),
                    _ => None,
                };
                if let Some(seen) = self.line_up.shown.take(Source::Here, msg, |_| size, now)
                    && let Some(s) = self.line_up.sender.as_mut()
                {
                    s.heard(Target::Here, &seen, now);
                }
            }
            Target::Computer(n) => self.net.tell(Some(n), Msg::LineUp(Box::new(msg)), true),
        }
    }

    /// Each pass: the crosses' lease here, and the primary's draft sent.
    pub(super) fn line_up_pass(&mut self) {
        let now = Instant::now();
        // Crosses from a primary this computer doesn't follow now go at once;
        // any go when their lease runs out.
        let v = self.net.view();
        let follows = (mode_of(&v) == NetMode::Follower)
            .then_some(v.me.primary)
            .flatten();
        self.line_up.shown.keep_only(follows);
        self.line_up.shown.tick(now);

        // The draft: which line-up, for which display.
        let me = v.me.id;
        let wanted = self.ui.line_up.as_ref().and_then(|d| {
            let display = self
                .show
                .stage3d
                .projectors
                .iter()
                .find(|p| p.id == d.projector)?
                .display?;
            let o = self.show.output(display)?;
            let target = match o.computer {
                Some(c) if !o.is_on(me) => Target::Computer(c),
                _ => Target::Here,
            };
            Some((d.session, display, target))
        });
        let stale = self
            .line_up
            .sender
            .as_ref()
            .is_some_and(|s| wanted != Some((s.session, s.display, s.target)));
        if stale && let Some(s) = self.line_up.sender.take() {
            // *Done* or *Cancel* (or another projector): the crosses end.
            self.line_up_deliver(s.target, s.end(), now);
        }
        let Some((session, display, target)) = wanted else {
            return;
        };
        let here_but_no_window = target == Target::Here && self.line_up_monitor(display).is_none();
        let sender = self
            .line_up
            .sender
            .get_or_insert_with(|| Sender::new(session, display, target));
        let Some(d) = self.ui.line_up.as_mut() else {
            return;
        };
        let msg = sender.due(&d.overlay(display), d.outline.as_ref(), d.outline_rev, now);
        // How the link stands, in words for the panes.
        let (connected, name, version) = match target {
            Target::Here => (
                true,
                "this computer".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ),
            Target::Computer(n) => match v.peer(n) {
                Some(p) => (p.connected, p.name.clone(), p.version.clone()),
                None => (false, "the projector's computer".into(), String::new()),
            },
        };
        let link = sender.link(connected, now);
        d.link = Some(link);
        d.link_words = if here_but_no_window {
            Some(no_window_here())
        } else {
            lineup::link_words(link, &name, &version, d.pixels)
        };
        if let Some(m) = msg {
            self.line_up_deliver(target, m, now);
        }
    }

    /// When the loop must come round next for a line-up.
    pub(super) fn line_up_wake(&self, now: Instant) -> Option<Instant> {
        (self.line_up.shown.overlay().is_some() || self.line_up.sender.is_some())
            .then(|| now + LINE_UP_TICK)
    }

    /// The crosses' group for each monitor that shows the held overlay's
    /// display (`screens`: each monitor's displays and size), and what is
    /// under them; monitors whose crosses changed since they last drew are
    /// in the second list. None in the first for one whose textures aren't
    /// in yet (`waiting` is set).
    pub(super) fn line_up_groups(
        &mut self,
        gpu: &gpu::Gpu,
        screens: &[(usize, Vec<OutputId>, (u32, u32))],
        waiting: &mut bool,
    ) -> (HashMap<usize, (OutputGroup, Under)>, Vec<usize>) {
        let mut out = HashMap::new();
        let stamp = self.line_up.shown.stamp();
        let mut changed = Vec::new();
        for (m, outputs, size) in screens {
            let on = self
                .line_up
                .shown
                .overlay()
                .is_some_and(|o| outputs.contains(&o.display));
            let mine = if on { stamp } else { None };
            if self.line_up.drawn.get(m).copied().flatten() != mine {
                changed.push(*m);
            }
            self.line_up.drawn.insert(*m, mine);
            if !on {
                continue;
            }
            let Some(overlay) = self.line_up.shown.overlay().cloned() else {
                continue;
            };
            match self.overlay_group(gpu, &overlay, *size) {
                Some(g) => {
                    out.insert(*m, (g, overlay.under));
                }
                None => *waiting = true,
            }
        }
        self.line_up
            .drawn
            .retain(|m, _| screens.iter().any(|s| s.0 == *m));
        (out, changed)
    }

    /// The crosses in raw monitor pixels (`size`): the outline, then a sprite
    /// per mark. None until their textures are in.
    fn overlay_group(
        &mut self,
        gpu: &gpu::Gpu,
        overlay: &Overlay,
        size: (u32, u32),
    ) -> Option<OutputGroup> {
        let mut layers = Vec::new();
        if let Some((outline, rev)) = self.line_up.shown.outline() {
            let fresh = self
                .line_up
                .outline
                .as_ref()
                .is_some_and(|(r, s, _)| *r == rev && *s == size);
            if !fresh {
                let img = lineup_overlay::outline_picture(outline, overlay.pixels, size);
                let old = self.line_up.outline.as_ref().map(|(_, _, t)| t.clone());
                let tex = super::renew_rgba(gpu, self.uploader.as_ref(), old.as_ref(), img);
                self.line_up.outline = Some((rev, size, tex));
            }
            let tex = self.line_up.outline.as_ref()?.2.clone();
            if !tex.is_filled() {
                return None;
            }
            layers.push(sprite_layer(tex, crate::show::LayerTransform::default()));
        }
        for (i, mark) in overlay.marks.iter().enumerate() {
            let look = if !mark.on {
                Look::Off
            } else if overlay.active == Some(i as u8) {
                Look::Active
            } else {
                Look::Plain
            };
            let up = self.uploader.as_ref();
            let tex = self
                .line_up
                .sprites
                .entry((mark.n, look))
                .or_insert_with(|| {
                    let img = lineup_overlay::sprite(mark.n, look);
                    let t = Arc::new(LayerTexture::new_rgba(gpu, img.width, img.height));
                    crate::upload::first(up, gpu, &t, crate::upload::Kind::Rgba, &img.data);
                    t
                })
                .clone();
            if !tex.is_filled() {
                return None;
            }
            layers.push(sprite_layer(
                tex,
                lineup_overlay::sprite_transform(mark.at, overlay.pixels, size),
            ));
        }
        Some(OutputGroup {
            layers,
            geometry: Geometry {
                canvas: size,
                warp: gpu::IDENTITY,
                pin: gpu::IDENTITY,
                mesh: None,
                slice: Geometry::WHOLE,
            },
        })
    }
}

fn sprite_layer(texture: Arc<LayerTexture>, transform: crate::show::LayerTransform) -> OutputLayer {
    OutputLayer {
        texture,
        opacity: 1.0,
        fit: Fit::Stretch,
        transform,
        color: ColorAdjust::default(),
        fx: Default::default(),
    }
}

/// A monitor's list with a line-up's crosses on it (pure): black under them
/// (nothing else drawn), or the usual list dimmed, then the crosses' group
/// last. Its edge blend goes off and, on black, its mapped pictures aren't
/// drawn (the caller's).
pub(super) fn with_crosses(
    list: Vec<OutputGroup>,
    crosses: OutputGroup,
    under: Under,
) -> Vec<OutputGroup> {
    let mut out = match under {
        Under::Dimmed => list
            .into_iter()
            .map(|mut g| {
                for l in &mut g.layers {
                    l.opacity *= DIM;
                }
                g
            })
            .collect(),
        Under::Black | Under::Unknown => Vec::new(),
    };
    out.push(crosses);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapped::gpu_tests::{H, W, gpu, layer, pixel, plain, stretched, window};

    /// A sprite as the window draws it: `SPRITE` square, transparent but for
    /// a red dot at its middle, so where the dot lands is where the mark is.
    fn dot_sprite(gpu: &gpu::Gpu) -> Arc<LayerTexture> {
        let s = lineup_overlay::SPRITE;
        let mut data = vec![0u8; (s * s * 4) as usize];
        for y in s / 2 - 1..s / 2 + 1 {
            for x in s / 2 - 1..s / 2 + 1 {
                let k = ((y * s + x) * 4) as usize;
                data[k..k + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let t = LayerTexture::new_rgba(gpu, s, s);
        t.upload_rgba(&gpu.queue, &data);
        Arc::new(t)
    }

    /// On a card (lavapipe): a mark at a pixel of a picture the size of the
    /// monitor lands on that pixel, in raw monitor pixels; over black nothing
    /// of the picture is left, dimmed it is `DIM` of itself.
    #[test]
    fn a_cross_lands_on_its_pixel_over_black_or_the_picture_dimmed() {
        let Some(gpu) = gpu() else { return };
        let monitor = (W, H);
        let at = [20.0, 12.0];
        let crosses = OutputGroup {
            layers: vec![sprite_layer(
                dot_sprite(&gpu),
                lineup_overlay::sprite_transform(at, monitor, monitor),
            )],
            geometry: plain(monitor),
        };
        let white = vec![255u8; (W * H * 4) as usize];
        let picture = OutputGroup {
            layers: vec![stretched(layer(&gpu, &white))],
            geometry: plain(monitor),
        };
        for under in [Under::Black, Under::Dimmed] {
            let list = with_crosses(vec![picture.clone()], crosses.clone(), under);
            let got = window(&gpu, &mut None, 0, &list, &[]);
            // The dot is the four pixels round `at`.
            for (x, y) in [(19, 11), (20, 11), (19, 12), (20, 12)] {
                let p = pixel(&got, x, y);
                assert!(
                    p[0] > 200 && p[1] < 60,
                    "{under:?}: the dot at {x},{y}: {p:?}"
                );
            }
            // Elsewhere, under the sprite's clear part and beyond it.
            for (x, y) in [(5, 5), (40, 30), (60, 2)] {
                let p = pixel(&got, x, y);
                let want = match under {
                    Under::Dimmed => (255.0 * DIM) as i32,
                    _ => 0,
                };
                for (c, v) in p.iter().take(3).enumerate() {
                    assert!(
                        (*v as i32 - want).abs() <= 8,
                        "{under:?}: at {x},{y} channel {c} is {v}, not {want}"
                    );
                }
            }
        }
    }
}
