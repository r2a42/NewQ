//! Line-up on the 3D stage's page (mapping plan 4.4, old plan 5.2): the
//! points in progress (`LineUpDraft`, `UiState::line_up`), never in the show
//! until *Done*.
//!
//! The app (`app.rs`, `net::lineup::Sender`) reads the draft each pass: while
//! there is one it sends `draft.overlay(..)` to the computer that drives the
//! projector's display (this one alone: one path), and sends `End` when the
//! draft is gone (*Done* or *Cancel*) or its session changes. It writes back
//! how the link stands (`link`, `link_words`), never saved. *Done* is one
//! ordinary edit of the show (placement, lens, `LineUp`), sent by show sync.
//!
//! (Round 2, agent A: the panes, the solve, the draft file.)

#![allow(dead_code)]

use crate::net::lineup::{Link, Mark, Overlay, Under};
use crate::show::OutputId;
use crate::stage3d::projector::{Before, LinePoint, ProjectorId};
use serde::{Deserialize, Serialize};

/// A line-up in progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineUpDraft {
    pub projector: ProjectorId,
    /// A random number for this line-up (`Overlay::session`): a new one for
    /// each start, so the crosses of an earlier one are passed by.
    #[serde(default)]
    pub session: u64,
    /// The points so far, as the show keeps them (`at` in a screen's own
    /// frame when on one).
    #[serde(default)]
    pub points: Vec<LinePoint>,
    /// The picture's size the points are placed in.
    #[serde(default)]
    pub pixels: (u32, u32),
    /// The point being moved (an index into `points`).
    #[serde(default)]
    pub active: Option<usize>,
    /// What is under the crosses on the projector.
    #[serde(default)]
    pub under: Under,
    /// Where the projector stood, and its lens, when line-up began.
    #[serde(default)]
    pub before: Option<Before>,
    /// The model's outline as the present solve throws it, in `pixels`
    /// (`calib::project`); None before a solve. Set by the panes at each
    /// solve, with `outline_rev` counted up.
    #[serde(skip)]
    pub outline: Option<Vec<Vec<[f32; 2]>>>,
    #[serde(skip)]
    pub outline_rev: u64,
    /// How the link to the crosses stands (the app, each pass).
    #[serde(skip)]
    pub link: Option<Link>,
    /// It in words, when there is something to say (`lineup::link_words`).
    #[serde(skip)]
    pub link_words: Option<String>,
}

impl LineUpDraft {
    pub fn new(projector: ProjectorId, pixels: (u32, u32), session: u64) -> LineUpDraft {
        LineUpDraft {
            projector,
            session,
            points: Vec::new(),
            pixels,
            active: None,
            under: Under::Black,
            before: None,
            outline: None,
            outline_rev: 0,
            link: None,
            link_words: None,
        }
    }

    /// What the crosses show now, for `display` (the session and count are
    /// the sender's; the outline goes separately): a mark per point, at its
    /// pixel, numbered from 1.
    pub fn overlay(&self, display: OutputId) -> Overlay {
        Overlay {
            session: self.session,
            seq: 0,
            display,
            pixels: self.pixels,
            marks: self
                .points
                .iter()
                .enumerate()
                .map(|(i, p)| Mark {
                    n: (i + 1).min(u16::MAX as usize) as u16,
                    at: p.pixel,
                    on: p.on,
                })
                .collect(),
            active: self
                .active
                .filter(|a| *a < self.points.len())
                .and_then(|a| u8::try_from(a).ok()),
            outline: None,
            under: self.under,
        }
    }
}
