//! Show file data model: cues, outputs, and (de)serialization.

use crate::clock::TimeOfDay;
use crate::media::playback::Playback;
use crate::timecode::Timecode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SHOW_EXTENSION: &str = "newq";
/// Volumes at or below this are treated as silence.
pub const MIN_DB: f32 = -60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct CueId(pub u64);

/// A display or a sub display of the show: what a cue is sent to. (Code calls
/// both outputs.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioOutputId(pub u64);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Show {
    /// 1 = shows with outputs placed on displays (before displays and
    /// outputs were one thing), 2 = displays and sub displays.
    pub version: u32,
    pub name: String,
    /// The show's displays and sub displays, which cues are sent to. A
    /// display is assigned by the user to a monitor and stays assigned: if
    /// the monitor goes away it shows nothing until it is back or is
    /// assigned to another.
    pub outputs: Vec<OutputConfig>,
    /// Shows from before displays and outputs were one thing kept these
    /// lists; `migrate_legacy` folds them into `outputs`.
    #[serde(default, rename = "displays", skip_serializing)]
    pub(crate) legacy_displays: Vec<LegacyDisplaySlot>,
    #[serde(
        default,
        rename = "sub_displays",
        alias = "virtual_displays",
        skip_serializing
    )]
    pub(crate) legacy_sub_displays: Vec<LegacySubDisplay>,
    /// Named destinations for sound, each on an audio device. With none, all
    /// sound goes to the system's default device. The first is the default
    /// for cues that don't choose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_outputs: Vec<AudioOutputConfig>,
    pub cues: Vec<Cue>,
    pub next_id: u64,
    /// Fade time used by the Panic / Stop All button.
    #[serde(default = "default_panic_fade")]
    pub panic_fade: f64,
    /// Lining sound up with picture. Nothing is held back unless set.
    #[serde(default, skip_serializing_if = "SyncSettings::is_default")]
    pub sync: SyncSettings,
    /// OSC and MIDI control. Off unless the show asks for it.
    #[serde(default, skip_serializing_if = "RemoteSettings::is_default")]
    pub remote: RemoteSettings,
}

/// A named audio output, on one of the machine's audio devices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioOutputConfig {
    pub id: AudioOutputId,
    pub name: String,
    /// The device's name; None = the system's default device.
    #[serde(default)]
    pub device: Option<String>,
    /// Milliseconds this output's sound is held back, to line it up with the
    /// picture (a Bluetooth speaker, a long cable run, a slow display).
    #[serde(default, skip_serializing_if = "no_delay")]
    pub delay_ms: u32,
}

fn no_delay(v: &u32) -> bool {
    *v == 0
}

/// The most a delay can be set to, in milliseconds.
pub const MAX_DELAY_MS: u32 = 2000;

/// How sound and picture are lined up. Each is a delay, so whichever is
/// ahead is the one to hold back. These apply to everything; each audio output
/// and each video output can add its own on top.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    /// Milliseconds all sound is held back.
    pub audio_ms: u32,
    /// Milliseconds all pictures are held back.
    pub video_ms: u32,
}

impl SyncSettings {
    pub fn is_default(&self) -> bool {
        *self == SyncSettings::default()
    }
}

/// Which remote-control listeners run while this show is open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteSettings {
    /// Listen for OSC over UDP.
    pub osc: bool,
    pub osc_port: u16,
    /// Listen to a MIDI input.
    pub midi: bool,
    /// Name of the MIDI input to use.
    pub midi_port: Option<String>,
    /// Only accept channel messages on this channel (0 to 15); None = any.
    pub midi_channel: Option<u8>,
    /// Only act on MIDI Show Control messages for this device id (0 to 111;
    /// messages to everyone, 127, always count); None = any.
    pub msc_device: Option<u8>,
    /// Follow MIDI Time Code on that input, firing cues at their timecodes.
    pub mtc: bool,
    /// Serve the web remote (a page for a phone or tablet) on `web_port`.
    pub web: bool,
    pub web_port: u16,
    /// Asked for by the web remote; empty = none. Kept in the show file.
    pub web_pin: String,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings {
            osc: false,
            osc_port: 53000,
            midi: false,
            midi_port: None,
            midi_channel: None,
            msc_device: None,
            mtc: false,
            web: false,
            web_port: 8080,
            web_pin: String::new(),
        }
    }
}

impl RemoteSettings {
    pub fn is_default(&self) -> bool {
        *self == RemoteSettings::default()
    }
}

fn default_panic_fade() -> f64 {
    1.0
}

impl Default for Show {
    fn default() -> Self {
        let mut show = Show {
            version: 2,
            name: "Untitled Show".into(),
            outputs: Vec::new(),
            legacy_displays: Vec::new(),
            legacy_sub_displays: Vec::new(),
            audio_outputs: Vec::new(),
            cues: Vec::new(),
            next_id: 1,
            panic_fade: default_panic_fade(),
            sync: SyncSettings::default(),
            remote: RemoteSettings::default(),
        };
        let id = OutputId(show.alloc_id());
        show.outputs.push(OutputConfig::new(id, "Display 1".into()));
        show
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DisplayRef {
    /// What the monitor was called: its model where the system says, else the
    /// name the system reported.
    pub name: String,
    /// Index in the OS monitor list, which tells look-alike monitors apart.
    pub index: usize,
    /// What stays the same for this monitor (make, model, serial number), where
    /// the system tells it; the display is found by this when it is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<String>,
}

/// A connected monitor as the operating system describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorKey {
    /// What to call it and recognise it by: its make and model when the
    /// system says, else the name it reports.
    pub identity: String,
    /// The name the system reports (all that older shows recorded).
    pub raw: String,
    /// Its make, model and serial number, where the system tells them.
    pub hardware: Option<String>,
}

impl MonitorKey {
    fn matches(&self, name: &str) -> bool {
        self.identity == name || self.raw == name
    }

    /// The reference a display assigned to this monitor keeps.
    pub fn reference(&self, index: usize) -> DisplayRef {
        DisplayRef {
            name: self.identity.clone(),
            index,
            hardware: self.hardware.clone(),
        }
    }
}

/// How many of the connected monitors look just like monitor `i` (the same
/// make and model, or when the system says nothing, the same name).
pub fn twins_of(monitors: &[MonitorKey], i: usize) -> u32 {
    let m = &monitors[i];
    monitors
        .iter()
        .filter(|k| match (&m.hardware, &k.hardware) {
            (Some(a), Some(b)) => hardware_model(a) == hardware_model(b),
            _ => k.identity == m.identity,
        })
        .count()
        .max(1) as u32
}

/// The make-and-model part of a hardware id: what look-alikes share.
fn hardware_model(id: &str) -> &str {
    id.split(':').next().unwrap_or(id)
}

/// What a display of the show is attached to right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayState {
    /// No monitor chosen for it.
    Unassigned,
    /// The monitor it was assigned to isn't connected.
    Missing,
    /// Assigned to this connected monitor (an index into the list of monitors).
    Connected(usize),
    /// Assigned to the monitor the main window is on, which outputs never
    /// cover.
    OnMainWindowDisplay(usize),
}

/// Find the monitor a display of the show was assigned to. Only that monitor
/// will do: when it isn't there, no other is picked in its place. `twins` is
/// how many look-alikes there were when it was assigned.
pub fn display_state(
    monitor: Option<&DisplayRef>,
    twins: u32,
    monitors: &[MonitorKey],
    control: Option<usize>,
) -> DisplayState {
    let Some(r) = monitor else {
        return DisplayState::Unassigned;
    };
    // By hardware identity when both have one; by name when the system
    // didn't say (an older show, or a system that tells nothing).
    let by_hardware = r.hardware.is_some() && monitors.iter().any(|m| m.hardware.is_some());
    let same: Vec<usize> = (0..monitors.len())
        .filter(|&i| match (&r.hardware, by_hardware) {
            (Some(h), true) => monitors[i].hardware.as_ref() == Some(h),
            _ => monitors[i].matches(&r.name),
        })
        .collect();
    let found = match same.len() {
        0 => None,
        1 if twins <= 1 => Some(same[0]),
        // A hardware id with a serial number names one monitor outright, even
        // when other monitors of its model were there (`twins` counts those).
        1 if by_hardware && r.hardware.as_deref().is_some_and(|h| h.contains(':')) => Some(same[0]),
        // Look-alikes can only be told apart by their order, and then only
        // when they are all there.
        n if n >= twins as usize && same.contains(&r.index) => Some(r.index),
        _ => None,
    };
    match found {
        None => DisplayState::Missing,
        Some(i) if Some(i) == control => DisplayState::OnMainWindowDisplay(i),
        Some(i) => DisplayState::Connected(i),
    }
}

/// What shows from before displays and outputs were one thing recorded.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LegacyDisplaySlot {
    id: u64,
    name: String,
    #[serde(default)]
    monitor: Option<DisplayRef>,
    #[serde(default = "one_twin")]
    twins: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LegacySubDisplay {
    id: u64,
    name: String,
    #[serde(default)]
    display: Option<u64>,
    #[serde(default)]
    parent: DisplayRef,
    area: Corners,
}

#[derive(Clone, Debug, Default, Deserialize)]
enum LegacyPlacement {
    /// Picked automatically: a monitor other than the main window's.
    #[default]
    Auto,
    MainWindow,
    Display(DisplayRef),
    Slot(u64),
    #[serde(alias = "Virtual")]
    SubDisplay(u64),
}

fn one_twin() -> u32 {
    1
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

fn is_unnamed(d: &DisplayRef) -> bool {
    *d == DisplayRef::default()
}

fn is_whole(c: &Corners) -> bool {
    *c == Corners::default()
}

fn is_default_pattern(p: &PatternKind) -> bool {
    *p == PatternKind::default()
}

/// The pictures a display can show to line it up (see `pattern`). The first
/// is the one displays have always shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PatternKind {
    #[default]
    Bars,
    Grid,
    Crosshatch,
    Checkerboard,
    Circles,
    GreySteps,
    Ramp,
    White,
    Black,
    Colours,
    Frame,
    Focus,
}

impl PatternKind {
    pub const ALL: [PatternKind; 12] = [
        PatternKind::Bars,
        PatternKind::Grid,
        PatternKind::Crosshatch,
        PatternKind::Checkerboard,
        PatternKind::Circles,
        PatternKind::GreySteps,
        PatternKind::Ramp,
        PatternKind::White,
        PatternKind::Black,
        PatternKind::Colours,
        PatternKind::Frame,
        PatternKind::Focus,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PatternKind::Bars => "Colour bars and number",
            PatternKind::Grid => "Grid",
            PatternKind::Crosshatch => "Crosshatch",
            PatternKind::Checkerboard => "Checkerboard",
            PatternKind::Circles => "Circles",
            PatternKind::GreySteps => "Grey steps",
            PatternKind::Ramp => "Grey ramp",
            PatternKind::White => "White",
            PatternKind::Black => "Black",
            PatternKind::Colours => "Red, green, blue, cyan, magenta, yellow",
            PatternKind::Frame => "Edges and safe areas",
            PatternKind::Focus => "Focus lines",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            PatternKind::Bars => {
                "Colour bars with a grid and the display's number: which screen is which, and colour"
            }
            PatternKind::Grid => {
                "A coarse grid with a centre cross and the number: squareness, keystone and where the picture lands"
            }
            PatternKind::Crosshatch => {
                "A fine grid of square cells, one pixel lines: straightness and warp, mesh and corner pin"
            }
            PatternKind::Checkerboard => {
                "Black and white squares: edges, sharpness and the shape of a warped picture"
            }
            PatternKind::Circles => {
                "Concentric circles and diagonals: they are round when the picture has the right shape"
            }
            PatternKind::GreySteps => {
                "Eleven steps from black to white: brightness and contrast, and that dark greys can be told apart"
            }
            PatternKind::Ramp => {
                "A smooth ramp from black to white: banding and whether the whole range shows"
            }
            PatternKind::White => {
                "All white: uniformity, hot spots, dust, and how bright the screen is"
            }
            PatternKind::Black => {
                "All black: black level and light leaking from the projector or panel"
            }
            PatternKind::Colours => {
                "Pure red, green and blue over cyan, magenta and yellow: colour balance and dead pixels"
            }
            PatternKind::Frame => {
                "Frames at the very edge, at 90% (action safe) and at 80% (title safe), with diagonals: overscan and cropping"
            }
            PatternKind::Focus => {
                "Lines one and two pixels wide in both directions: focus, and whether pixels are shown one for one"
            }
        }
    }

    /// Whether the pattern is made of single pixels, so it is drawn at the
    /// display's own size rather than a smaller one scaled up.
    pub fn pixel_exact(self) -> bool {
        matches!(self, PatternKind::Crosshatch | PatternKind::Focus)
    }
}

/// A display, assigned to a monitor, or a sub display, an area of one. Cues
/// are sent to either.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputConfig {
    pub id: OutputId,
    pub name: String,
    /// Whether it is shown at all.
    pub enabled: bool,
    #[serde(default)]
    pub test_pattern: bool,
    /// Which test pattern it shows when `test_pattern` is on.
    #[serde(default, skip_serializing_if = "is_default_pattern")]
    pub pattern: PatternKind,
    /// Keystone / corner pin applied to everything on a display. (A sub
    /// display's area is its keystone.)
    #[serde(default)]
    pub corners: Corners,
    /// A finer warp than four corners (see `mesh`). When there is one it takes
    /// the place of `corners` on a display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<crate::mesh::Mesh>,
    /// Display: it shows part of another display's picture instead of its own,
    /// so several projectors can show one wide picture between them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_of: Option<PartOf>,
    /// The size of the picture in pixels when it isn't the size of the monitor
    /// the display is on: a wide picture that projectors share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<(u32, u32)>,
    /// Display: edges that fade out where projectors overlap.
    #[serde(default, skip_serializing_if = "EdgeBlend::is_off")]
    pub blend: EdgeBlend,
    /// Display: its picture is also sent out as a video stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamOut>,
    /// Milliseconds everything on it is held back (a slow projector).
    #[serde(default, skip_serializing_if = "no_delay")]
    pub delay_ms: u32,
    /// Display: the monitor it is assigned to; None = not assigned, so it is
    /// shown only in the main window's preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<DisplayRef>,
    /// How many look-alike monitors (the same make and model) were connected
    /// when it was assigned. With identical monitors the display can only be
    /// told apart by their order, so it is not used unless they are all
    /// connected: a guess could put the picture on the wrong one.
    #[serde(default = "one_twin", skip_serializing_if = "is_one")]
    pub twins: u32,
    /// Sub display: the display it is an area of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<OutputId>,
    /// Sub display: its four corners on that display. Any convex shape; the
    /// whole display by default.
    #[serde(default, skip_serializing_if = "is_whole")]
    pub area: Corners,
    /// Older shows: where an output was placed, folded into the fields
    /// above by `Show::migrate_legacy`.
    #[serde(default, skip_serializing)]
    placement: Option<LegacyPlacement>,
    /// Older shows: the monitor chosen before there were placements.
    #[serde(default, skip_serializing)]
    display: Option<DisplayRef>,
    /// Set by `migrate_legacy` for an output that was picked a monitor
    /// automatically, until `assign_automatic_displays` has chosen once.
    #[serde(skip)]
    needs_monitor: bool,
}

impl OutputConfig {
    /// A display, not assigned to a monitor yet.
    pub fn new(id: OutputId, name: String) -> Self {
        OutputConfig {
            id,
            name,
            enabled: true,
            test_pattern: false,
            pattern: PatternKind::default(),
            corners: Corners::default(),
            mesh: None,
            part_of: None,
            picture: None,
            blend: EdgeBlend::default(),
            stream: None,
            delay_ms: 0,
            monitor: None,
            twins: 1,
            within: None,
            area: Corners::default(),
            placement: None,
            display: None,
            needs_monitor: false,
        }
    }

    pub fn is_sub_display(&self) -> bool {
        self.within.is_some()
    }
}

/// A display showing part of another display's picture: which display, and
/// which part, as fractions of that picture (left, top, width, height).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartOf {
    pub display: OutputId,
    pub area: [f32; 4],
}

/// A display's picture sent out as video, to a virtual camera or over the
/// network, so a video call or a streaming program can use it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamOut {
    pub enabled: bool,
    pub kind: StreamKind,
    pub fps: u32,
    /// How tall the picture is sent; the width follows its shape.
    pub height: u32,
}

impl Default for StreamOut {
    fn default() -> Self {
        StreamOut {
            enabled: false,
            kind: StreamKind::default(),
            fps: 30,
            height: 720,
        }
    }
}

impl StreamOut {
    /// The same with its numbers in a range FFmpeg and the machine can take.
    pub fn clamped(&self) -> StreamOut {
        StreamOut {
            fps: self.fps.clamp(1, 60),
            height: self.height.clamp(16, 2160),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StreamKind {
    /// A Video4Linux loopback device (Linux): what Zoom, Teams and OBS
    /// see as a camera.
    VirtualCamera { device: String },
    /// MPEG-TS over UDP, which OBS can take as a media source.
    Udp { host: String, port: u16 },
    /// Any FFmpeg output: the arguments that follow the input.
    Custom { args: String },
    /// An NDI source of this name on the network (needs NDI's runtime).
    Ndi { name: String },
}

impl Default for StreamKind {
    fn default() -> Self {
        if cfg!(target_os = "linux") {
            StreamKind::VirtualCamera {
                device: String::new(),
            }
        } else {
            StreamKind::Udp {
                host: "127.0.0.1".into(),
                port: 5000,
            }
        }
    }
}

/// How far in from each edge of a display its picture fades to black, as
/// fractions of the display's width (left and right) and height (top and
/// bottom), so that where two projectors overlap their light adds up to one
/// even picture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeBlend {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    /// The projector's gamma, which the fade allows for (2.2 is typical).
    #[serde(default = "default_blend_gamma")]
    pub gamma: f32,
}

fn default_blend_gamma() -> f32 {
    2.2
}

impl Default for EdgeBlend {
    fn default() -> Self {
        EdgeBlend {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
            gamma: default_blend_gamma(),
        }
    }
}

impl EdgeBlend {
    /// No edge fades.
    pub fn is_off(&self) -> bool {
        self.left <= 0.0 && self.right <= 0.0 && self.top <= 0.0 && self.bottom <= 0.0
    }

    /// The same with every width kept in range (0 to half the display) and
    /// the gamma sensible. A show file may hold anything.
    pub fn clamped(&self) -> EdgeBlend {
        let w = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 0.5)
            } else {
                0.0
            }
        };
        EdgeBlend {
            left: w(self.left),
            right: w(self.right),
            top: w(self.top),
            bottom: w(self.bottom),
            gamma: if self.gamma.is_finite() {
                self.gamma.clamp(1.0, 4.0)
            } else {
                default_blend_gamma()
            },
        }
    }
}

/// Where the corners of an output's picture land on its screen, as fractions
/// of the screen (0,0 = top-left, 1,1 = bottom-right), in the order
/// top-left, top-right, bottom-right, bottom-left.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Corners(pub [[f32; 2]; 4]);

impl Default for Corners {
    fn default() -> Self {
        Corners([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]])
    }
}

impl Corners {
    pub const LABELS: [&'static str; 4] = ["Top left", "Top right", "Bottom right", "Bottom left"];

    /// An axis-aligned rectangle: left, top, width, height.
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Corners {
        Corners([[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
    }

    /// Left, top, width and height if the corners form an axis-aligned rectangle.
    pub fn as_rect(&self) -> Option<[f32; 4]> {
        let [tl, tr, br, bl] = self.0;
        let rect = tl[1] == tr[1]
            && bl[1] == br[1]
            && tl[0] == bl[0]
            && tr[0] == br[0]
            && tr[0] > tl[0]
            && bl[1] > tl[1];
        rect.then(|| [tl[0], tl[1], tr[0] - tl[0], bl[1] - tl[1]])
    }

    /// The smallest rectangle containing the corners: left, top, width, height.
    pub fn bounds(&self) -> [f32; 4] {
        let min = |v: [f32; 4]| v.into_iter().fold(f32::MAX, f32::min);
        let max = |v: [f32; 4]| v.into_iter().fold(f32::MIN, f32::max);
        let (xs, ys) = (self.0.map(|c| c[0]), self.0.map(|c| c[1]));
        [min(xs), min(ys), max(xs) - min(xs), max(ys) - min(ys)]
    }

    /// Size in pixels of the shape on a `screen`-sized display. For shapes that
    /// aren't rectangles, opposite edge lengths are averaged.
    pub fn size_on(&self, screen: (u32, u32)) -> (u32, u32) {
        let (sw, sh) = (screen.0 as f32, screen.1 as f32);
        let len = |a: [f32; 2], b: [f32; 2]| ((b[0] - a[0]) * sw).hypot((b[1] - a[1]) * sh);
        let [tl, tr, br, bl] = self.0;
        let w = (len(tl, tr) + len(bl, br)) / 2.0;
        let h = (len(tl, bl) + len(tr, br)) / 2.0;
        (w.round().max(1.0) as u32, h.round().max(1.0) as u32)
    }

    pub fn is_identity(&self) -> bool {
        *self == Corners::default()
    }

    /// The four corners form a convex quadrilateral in order (no twists or
    /// dents), which a projector keystone needs.
    pub fn is_convex(&self) -> bool {
        let p = self.0;
        let mut sign = 0.0f32;
        for i in 0..4 {
            let (a, b, c) = (p[i], p[(i + 1) % 4], p[(i + 2) % 4]);
            let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
            if cross.abs() < 1e-6 {
                return false;
            }
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ContinueMode {
    #[default]
    DoNotContinue,
    /// Fire the next cue as soon as this one starts.
    AutoContinue,
    /// Fire the next cue when this one finishes.
    AutoFollow,
}

impl ContinueMode {
    pub const ALL: [ContinueMode; 3] = [
        ContinueMode::DoNotContinue,
        ContinueMode::AutoContinue,
        ContinueMode::AutoFollow,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ContinueMode::DoNotContinue => "Do not continue",
            ContinueMode::AutoContinue => "Auto-continue",
            ContinueMode::AutoFollow => "Auto-follow",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Fit {
    #[default]
    Contain,
    Cover,
    Stretch,
}

impl Fit {
    pub const ALL: [Fit; 3] = [Fit::Contain, Fit::Cover, Fit::Stretch];
    pub fn label(self) -> &'static str {
        match self {
            Fit::Contain => "Fit (letterbox)",
            Fit::Cover => "Fill (crop)",
            Fit::Stretch => "Stretch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Curve {
    Linear,
    #[default]
    SCurve,
}

impl Curve {
    pub const ALL: [Curve; 2] = [Curve::Linear, Curve::SCurve];
    pub fn label(self) -> &'static str {
        match self {
            Curve::Linear => "Linear",
            Curve::SCurve => "S-curve",
        }
    }
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::SCurve => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// How a visual cue enters, relative to what is already on its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Transition {
    /// Fade in on top of whatever is showing, leaving it running.
    #[default]
    FadeIn,
    /// Replace what is showing instantly.
    Cut,
    /// Fade in while everything else on the output fades out and stops.
    Crossfade,
    /// Fade what is showing to black, then fade up the new content.
    DipToBlack,
    /// Fade what is showing to white, then fade up the new content.
    DipToWhite,
}

impl Transition {
    pub const ALL: [Transition; 5] = [
        Transition::Crossfade,
        Transition::DipToBlack,
        Transition::DipToWhite,
        Transition::Cut,
        Transition::FadeIn,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Transition::FadeIn => "Fade in over",
            Transition::Cut => "Cut",
            Transition::Crossfade => "Crossfade",
            Transition::DipToBlack => "Dip to black",
            Transition::DipToWhite => "Dip to white",
        }
    }
    pub fn short_label(self) -> &'static str {
        match self {
            Transition::FadeIn => "Fade in",
            Transition::Cut => "Cut",
            Transition::Crossfade => "X-fade",
            Transition::DipToBlack => "Dip black",
            Transition::DipToWhite => "Dip white",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Transition::FadeIn => {
                "Fades in on top of whatever is already showing and leaves it running."
            }
            Transition::Cut => "Replaces whatever is on this output instantly.",
            Transition::Crossfade => {
                "Fades in while everything else on this output fades out and stops."
            }
            Transition::DipToBlack => {
                "Fades the output to black (first half), then fades this cue up (second half)."
            }
            Transition::DipToWhite => {
                "Fades the output to white (first half), then fades this cue up (second half)."
            }
        }
    }
    /// The colour a dip passes through.
    pub fn dip_color(self) -> Option<[u8; 4]> {
        match self {
            Transition::DipToBlack => Some([0, 0, 0, 255]),
            Transition::DipToWhite => Some([255, 255, 255, 255]),
            _ => None,
        }
    }
    pub fn uses_time(self) -> bool {
        self != Transition::Cut
    }
}

/// Fade used when fading out a cue by hand whose own fade-out time is zero.
pub const DEFAULT_FADE_OUT: f64 = 2.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cue {
    pub id: CueId,
    pub number: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pre_wait: f64,
    #[serde(default)]
    pub continue_mode: ContinueMode,
    /// A disarmed cue is skipped as if it weren't in the list: GO passes
    /// over it, continues don't fire it, and a group doesn't start it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub armed: bool,
    /// The group this cue belongs to. A group's children follow it
    /// directly in `Show::cues`, and groups don't nest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<CueId>,
    /// A key that fires this cue (an egui key name such as "F1" or "Q"); see
    /// `Show::set_hotkey`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    /// A MIDI note that fires this cue; see `Show::set_midi_note`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_note: Option<u8>,
    /// Show this cue as a button on the cart.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cart: bool,
    /// Fire this cue when incoming MIDI Time Code reaches this position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timecode: Option<Timecode>,
    /// Fire this cue when the time of day reaches this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<TimeOfDay>,
    /// A colour label, to find it by in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<CueColor>,
    pub kind: CueKind,
}

/// The colours a cue can be labelled with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CueColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Pink,
}

impl CueColor {
    pub const ALL: [CueColor; 7] = [
        CueColor::Red,
        CueColor::Orange,
        CueColor::Yellow,
        CueColor::Green,
        CueColor::Blue,
        CueColor::Purple,
        CueColor::Pink,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CueColor::Red => "Red",
            CueColor::Orange => "Orange",
            CueColor::Yellow => "Yellow",
            CueColor::Green => "Green",
            CueColor::Blue => "Blue",
            CueColor::Purple => "Purple",
            CueColor::Pink => "Pink",
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            CueColor::Red => [226, 72, 72],
            CueColor::Orange => [232, 140, 52],
            CueColor::Yellow => [222, 196, 60],
            CueColor::Green => [84, 184, 96],
            CueColor::Blue => [76, 140, 232],
            CueColor::Purple => [158, 104, 216],
            CueColor::Pink => [226, 112, 176],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CueKind {
    Video(MediaCue),
    Image(MediaCue),
    Audio(MediaCue),
    Fade(FadeCue),
    Stop(StopCue),
    Wait(WaitCue),
    Group(GroupCue),
    Text(TextCue),
    Camera(CameraCue),
    Send(SendCue),
    LiveAudio(LiveAudioCue),
    Control(ControlCue),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueType {
    Video,
    Image,
    Audio,
    Fade,
    Stop,
    Wait,
    Group,
    Text,
    Camera,
    Send,
    LiveAudio,
    Control,
    /// Ways to make a text cue that shows the time (a cue's own type is
    /// still `Text`).
    Clock,
    Timer,
}

impl CueType {
    pub fn label(self) -> &'static str {
        match self {
            CueType::Video => "Video",
            CueType::Image => "Image",
            CueType::Audio => "Audio",
            CueType::Fade => "Fade",
            CueType::Stop => "Stop",
            CueType::Wait => "Wait",
            CueType::Group => "Group",
            CueType::Text => "Text",
            CueType::Camera => "Camera",
            CueType::Send => "Send",
            CueType::LiveAudio => "Live audio",
            CueType::Control => "Control",
            CueType::Clock => "Clock",
            CueType::Timer => "Timer",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            CueType::Video => "🎬",
            CueType::Image => "🖼",
            CueType::Audio => "🔊",
            CueType::Fade => "📉",
            CueType::Stop => "⏹",
            CueType::Wait => "⏱",
            CueType::Group => "📁",
            CueType::Text => "📝",
            CueType::Camera => "📷",
            CueType::Send => "📡",
            CueType::LiveAudio => "🎙",
            CueType::Control => "🎛",
            CueType::Clock => "🕒",
            CueType::Timer => "⏳",
        }
    }
}

impl CueKind {
    pub fn cue_type(&self) -> CueType {
        match self {
            CueKind::Video(_) => CueType::Video,
            CueKind::Image(_) => CueType::Image,
            CueKind::Audio(_) => CueType::Audio,
            CueKind::Fade(_) => CueType::Fade,
            CueKind::Stop(_) => CueType::Stop,
            CueKind::Wait(_) => CueType::Wait,
            CueKind::Group(_) => CueType::Group,
            CueKind::Text(_) => CueType::Text,
            CueKind::Camera(_) => CueType::Camera,
            CueKind::Send(_) => CueType::Send,
            CueKind::LiveAudio(_) => CueType::LiveAudio,
            CueKind::Control(_) => CueType::Control,
        }
    }
    /// The playback settings of a cue that plays something: transitions,
    /// fades, output, layer and so on. A text cue has them too.
    pub fn media(&self) -> Option<&MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            CueKind::Text(t) => Some(&t.media),
            CueKind::Camera(c) => Some(&c.media),
            CueKind::LiveAudio(c) => Some(&c.media),
            _ => None,
        }
    }
    pub fn media_mut(&mut self) -> Option<&mut MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            CueKind::Text(t) => Some(&mut t.media),
            CueKind::Camera(c) => Some(&mut c.media),
            CueKind::LiveAudio(c) => Some(&mut c.media),
            _ => None,
        }
    }
    /// True for the cues whose decoders can be started ahead of GO.
    pub fn preloads(&self) -> bool {
        matches!(
            self,
            CueKind::Video(_) | CueKind::Image(_) | CueKind::Audio(_) | CueKind::Camera(_)
        )
    }
    /// Like `media`, but only for cues that read a media file.
    pub fn file_media(&self) -> Option<&MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            _ => None,
        }
    }
}

/// Where a visual cue sits on its output, beyond its fit mode: moved, scaled,
/// rotated and cropped. The default leaves the cue exactly as fitted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerTransform {
    /// Offset of the cue's centre, as a fraction of the output's width/height
    /// (right and down are positive).
    pub x: f32,
    pub y: f32,
    /// 1.0 = as fitted; negative values mirror.
    pub scale_x: f32,
    pub scale_y: f32,
    /// Degrees, clockwise.
    pub rotation: f32,
    /// Fractions of the picture cut from the left, top, right and bottom.
    pub crop: [f32; 4],
}

impl Default for LayerTransform {
    fn default() -> Self {
        LayerTransform {
            x: 0.0,
            y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            crop: [0.0; 4],
        }
    }
}

impl LayerTransform {
    pub fn is_identity(&self) -> bool {
        *self == LayerTransform::default()
    }
}

/// A colour to make clear (a green or blue screen).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChromaKey {
    /// The colour of the screen.
    pub color: [u8; 3],
    /// How far from that colour (on the chroma plane, 0 to about 0.7) is
    /// made fully clear.
    pub tolerance: f32,
    /// A soft edge beyond the tolerance, where it fades back in.
    pub softness: f32,
    /// How much of the key colour's spill is pulled out of what is kept (0 to 1).
    pub spill: f32,
}

impl Default for ChromaKey {
    fn default() -> Self {
        ChromaKey {
            color: [0, 255, 0],
            tolerance: 0.2,
            softness: 0.1,
            spill: 0.5,
        }
    }
}

/// Colour correction for a visual cue, and an optional chroma key. The
/// default leaves the picture as it is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorAdjust {
    /// −1 to 1, added to each colour (0 = none).
    pub brightness: f32,
    /// 0 to 2 about mid grey (1 = none).
    pub contrast: f32,
    /// 0 (grey) to 2 (vivid); 1 = none.
    pub saturation: f32,
    /// Degrees around the colour wheel (0 = none).
    pub hue: f32,
    pub key: Option<ChromaKey>,
}

impl Default for ColorAdjust {
    fn default() -> Self {
        ColorAdjust {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            hue: 0.0,
            key: None,
        }
    }
}

impl ColorAdjust {
    pub fn is_default(&self) -> bool {
        *self == ColorAdjust::default()
    }
}

/// One keyframe of an animated parameter: its value `v` at `t` seconds into
/// the cue, easing to the next keyframe by `curve`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub t: f64,
    pub v: f32,
    #[serde(default = "linear", skip_serializing_if = "is_linear")]
    pub curve: Curve,
}

fn linear() -> Curve {
    Curve::Linear
}

fn is_linear(c: &Curve) -> bool {
    *c == Curve::Linear
}

/// A number that can change over the length of a cue: `value` while it has no
/// keyframes, otherwise the keyframes (kept in time order) say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Animated {
    pub value: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Keyframe>,
}

impl Animated {
    pub fn new(value: f32) -> Animated {
        Animated {
            value,
            keys: Vec::new(),
        }
    }

    pub fn is_animated(&self) -> bool {
        !self.keys.is_empty()
    }

    /// The value `t` seconds into the cue: the first keyframe's before it,
    /// the last's after the last, and between two the first's value eased
    /// towards the second's.
    pub fn at(&self, t: f64) -> f32 {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return self.value;
        };
        if t <= first.t || t.is_nan() {
            return first.v;
        }
        if t >= last.t {
            return last.v;
        }
        // Strictly between the first and the last, so there are two to ease
        // between (kept in range should the keys be out of order).
        let i = self
            .keys
            .partition_point(|k| k.t <= t)
            .clamp(1, self.keys.len() - 1)
            - 1;
        let (a, b) = (self.keys[i], self.keys[i + 1]);
        let f = a.curve.apply((t - a.t) / (b.t - a.t).max(1e-9)) as f32;
        a.v + (b.v - a.v) * f
    }

    /// Put a keyframe at `t` (replacing the value of one already there, to
    /// the nearest millisecond). Returns its index.
    pub fn set_key(&mut self, t: f64, v: f32) -> usize {
        let t = t.max(0.0);
        if let Some(i) = self.keys.iter().position(|k| (k.t - t).abs() < 0.001) {
            self.keys[i].v = v;
            return i;
        }
        let i = self.keys.partition_point(|k| k.t < t);
        self.keys.insert(
            i,
            Keyframe {
                t,
                v,
                curve: Curve::Linear,
            },
        );
        i
    }

    /// Move keyframe `i` to `t` and `v`, keeping the order. Returns where it
    /// is now.
    pub fn move_key(&mut self, i: usize, t: f64, v: f32) -> usize {
        if i >= self.keys.len() {
            return i;
        }
        let mut key = self.keys.remove(i);
        key.t = t.max(0.0);
        key.v = v;
        let at = self.keys.partition_point(|k| k.t < key.t);
        self.keys.insert(at, key);
        at
    }

    pub fn remove_key(&mut self, i: usize) {
        if i < self.keys.len() {
            self.keys.remove(i);
        }
    }

    /// Put the keyframes in time order, which `at` relies on and a
    /// hand-edited file may not keep. A value that isn't a number becomes
    /// `default` and keyframes that aren't are dropped (a file can't hold
    /// those, but nothing else here should have to check).
    pub fn sanitize(&mut self, default: f32) {
        if !self.value.is_finite() {
            self.value = default;
        }
        self.keys.retain(|k| k.t.is_finite() && k.v.is_finite());
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    }
}

/// What an effect on a cue does. The first five act on the picture, the rest
/// on the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    Blur,
    Sharpen,
    Vignette,
    Pixelate,
    Negative,
    Eq,
    Filter,
    Compressor,
    Echo,
    Reverb,
}

/// One number an effect takes.
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
    pub decimals: usize,
}

const fn spec(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    decimals: usize,
) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit,
        decimals,
    }
}

impl EffectKind {
    pub const VIDEO: [EffectKind; 5] = [
        EffectKind::Blur,
        EffectKind::Sharpen,
        EffectKind::Vignette,
        EffectKind::Pixelate,
        EffectKind::Negative,
    ];
    pub const AUDIO: [EffectKind; 5] = [
        EffectKind::Eq,
        EffectKind::Filter,
        EffectKind::Compressor,
        EffectKind::Echo,
        EffectKind::Reverb,
    ];

    pub fn is_video(self) -> bool {
        Self::VIDEO.contains(&self)
    }

    pub fn label(self) -> &'static str {
        match self {
            EffectKind::Blur => "Blur",
            EffectKind::Sharpen => "Sharpen",
            EffectKind::Vignette => "Vignette",
            EffectKind::Pixelate => "Pixelate",
            EffectKind::Negative => "Negative",
            EffectKind::Eq => "3-band EQ",
            EffectKind::Filter => "High / low-pass filter",
            EffectKind::Compressor => "Compressor",
            EffectKind::Echo => "Echo",
            EffectKind::Reverb => "Reverb",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            EffectKind::Blur => "Softens the picture (a disc blur, drawn on the graphics card)",
            EffectKind::Sharpen => "Brings out edges and detail",
            EffectKind::Vignette => "Darkens the edges of the picture",
            EffectKind::Pixelate => "Turns the picture into big blocks",
            EffectKind::Negative => "Inverts the colours",
            EffectKind::Eq => "Boosts or cuts the low, middle and high of the sound",
            EffectKind::Filter => "Cuts the lows (high-pass) and the highs (low-pass)",
            EffectKind::Compressor => "Evens out loud and quiet",
            EffectKind::Echo => "Repeats the sound after a delay",
            EffectKind::Reverb => "Puts the sound in a room",
        }
    }

    /// The numbers it takes, in order (at most four).
    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            EffectKind::Blur => &BLUR,
            EffectKind::Sharpen => &SHARPEN,
            EffectKind::Vignette => &VIGNETTE,
            EffectKind::Pixelate => &PIXELATE,
            EffectKind::Negative => &NEGATIVE,
            EffectKind::Eq => &EQ,
            EffectKind::Filter => &FILTER,
            EffectKind::Compressor => &COMPRESSOR,
            EffectKind::Echo => &ECHO,
            EffectKind::Reverb => &REVERB,
        }
    }
}

static BLUR: [ParamSpec; 1] = [spec("Radius", 0.0, 100.0, 8.0, " px", 1)];
static SHARPEN: [ParamSpec; 1] = [spec("Amount", 0.0, 5.0, 1.0, "", 2)];
static VIGNETTE: [ParamSpec; 2] = [
    spec("Amount", 0.0, 1.0, 0.6, "", 2),
    spec("Softness", 0.05, 1.0, 0.5, "", 2),
];
static PIXELATE: [ParamSpec; 1] = [spec("Block size", 1.0, 200.0, 16.0, " px", 0)];
static NEGATIVE: [ParamSpec; 1] = [spec("Amount", 0.0, 1.0, 1.0, "", 2)];
static EQ: [ParamSpec; 3] = [
    spec("Low", -18.0, 18.0, 0.0, " dB", 1),
    spec("Mid", -18.0, 18.0, 0.0, " dB", 1),
    spec("High", -18.0, 18.0, 0.0, " dB", 1),
];
static FILTER: [ParamSpec; 2] = [
    spec("High-pass at", 20.0, 20_000.0, 20.0, " Hz", 0),
    spec("Low-pass at", 20.0, 20_000.0, 20_000.0, " Hz", 0),
];
static COMPRESSOR: [ParamSpec; 4] = [
    spec("Threshold", -60.0, 0.0, -18.0, " dB", 1),
    spec("Ratio", 1.0, 20.0, 4.0, " : 1", 1),
    spec("Make-up gain", 0.0, 24.0, 0.0, " dB", 1),
    spec("Release", 20.0, 1000.0, 150.0, " ms", 0),
];
static ECHO: [ParamSpec; 3] = [
    spec("Time", 20.0, 2000.0, 350.0, " ms", 0),
    spec("Feedback", 0.0, 0.95, 0.4, "", 2),
    spec("Mix", 0.0, 1.0, 0.35, "", 2),
];
static REVERB: [ParamSpec; 3] = [
    spec("Size", 0.0, 1.0, 0.6, "", 2),
    spec("Damping", 0.0, 1.0, 0.5, "", 2),
    spec("Mix", 0.0, 1.0, 0.25, "", 2),
];

/// An effect on a cue, with each of its numbers (which can be animated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    pub p: Vec<Animated>,
}

impl Effect {
    pub fn new(kind: EffectKind) -> Effect {
        Effect {
            kind,
            enabled: true,
            p: kind
                .params()
                .iter()
                .map(|s| Animated::new(s.default))
                .collect(),
        }
    }

    /// Its numbers `t` seconds into the cue, each kept within its range;
    /// missing ones (a hand-edited file) are the defaults.
    pub fn values(&self, t: f64) -> [f32; 4] {
        let mut out = [0.0; 4];
        for (i, spec) in self.kind.params().iter().enumerate() {
            let v = self.p.get(i).map_or(spec.default, |a| a.at(t));
            // Not a number (keys at infinity): the default rather than a
            // NaN, which the sound filters would never recover from.
            out[i] = if v.is_finite() { v } else { spec.default }.clamp(spec.min, spec.max);
        }
        out
    }

    /// Put a hand-edited file's numbers right (see `Animated::sanitize`).
    pub fn sanitize(&mut self) {
        for (a, spec) in self.p.iter_mut().zip(self.kind.params()) {
            a.sanitize(spec.default);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MediaCue {
    pub file: PathBuf,
    /// Output for visual cues. Ignored for audio cues.
    pub output: Option<OutputId>,
    /// Higher layers draw on top.
    #[serde(default)]
    pub layer: i32,
    #[serde(default)]
    pub fit: Fit,
    /// How a visual cue enters. Audio cues always fade in.
    #[serde(default)]
    pub transition: Transition,
    /// Transition (or fade-in) time in seconds. For dips, the total of the
    /// fade to the colour and the fade up from it.
    #[serde(default)]
    pub fade_in: f64,
    /// This cue's own fade-out: used before the media ends naturally, and
    /// whenever the cue is faded out by hand or by a Stop cue using own fades.
    #[serde(default)]
    pub fade_out: f64,
    #[serde(default)]
    pub curve: Curve,
    #[serde(default = "one")]
    pub opacity: f32,
    /// Where the sound goes (None = the show's first audio output).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output: Option<AudioOutputId>,
    /// The first device channel the sound goes to, counting from 0: the left
    /// channel plays there and the right on the next.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub audio_channel: u16,
    /// Position, scale, rotation and crop on the output (visual cues).
    #[serde(default, skip_serializing_if = "LayerTransform::is_identity")]
    pub transform: LayerTransform,
    /// Colour correction and chroma key (visual cues).
    #[serde(default, skip_serializing_if = "ColorAdjust::is_default")]
    pub color: ColorAdjust,
    #[serde(default)]
    pub volume_db: f32,
    /// In point, seconds.
    #[serde(default)]
    pub start: f64,
    /// Out point, seconds. For images: how long to hold (None = until stopped).
    #[serde(default)]
    pub end: Option<f64>,
    /// Repeat the slice between the in and out points (the whole file if there is no out point).
    #[serde(default)]
    pub loop_media: bool,
    /// How many times a looping cue plays in all; 0 = until stopped.
    #[serde(default)]
    pub loop_count: u32,
    /// Video: hold the last frame when the media ends instead of stopping.
    #[serde(default)]
    pub hold_last_frame: bool,
    /// Effects on the picture and the sound, in order (see `EffectKind`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Video and audio files: playback speed, 1.0 = as recorded. Sound keeps
    /// its pitch.
    #[serde(default = "unit_rate", skip_serializing_if = "is_unit_rate")]
    pub rate: f64,
    /// In a slide show: this slide's own fade-in time, in place of the show's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_fade: Option<f64>,
    /// In a slide show: how long this slide is shown in full before the next
    /// one starts to fade in, in place of the show's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_duration: Option<f64>,
    /// Legacy (pre-transition) crossfade flag, migrated on load.
    #[serde(default, skip_serializing)]
    crossfade: bool,
}

fn one() -> f32 {
    1.0
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

fn yes() -> bool {
    true
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_true(v: &bool) -> bool {
    *v
}

pub const MIN_RATE: f64 = 0.25;
pub const MAX_RATE: f64 = 4.0;

fn unit_rate() -> f64 {
    1.0
}

fn is_unit_rate(v: &f64) -> bool {
    *v == 1.0
}

impl MediaCue {
    pub fn new(file: PathBuf, output: Option<OutputId>) -> Self {
        MediaCue {
            file,
            output,
            layer: 0,
            fit: Fit::Contain,
            transition: Transition::FadeIn,
            fade_in: 0.0,
            fade_out: 0.0,
            curve: Curve::SCurve,
            opacity: 1.0,
            audio_output: None,
            audio_channel: 0,
            transform: LayerTransform::default(),
            color: ColorAdjust::default(),
            volume_db: 0.0,
            start: 0.0,
            end: None,
            loop_media: false,
            loop_count: 0,
            hold_last_frame: false,
            effects: Vec::new(),
            rate: 1.0,
            slide_fade: None,
            slide_duration: None,
            crossfade: false,
        }
    }

    /// New visual cues crossfade by default, so changing content needs no extra cues.
    pub fn new_visual(file: PathBuf, output: Option<OutputId>) -> Self {
        MediaCue {
            transition: Transition::Crossfade,
            fade_in: 1.0,
            fade_out: 1.0,
            ..MediaCue::new(file, output)
        }
    }

    /// The playback speed, kept to what FFmpeg's filters do well.
    pub fn speed(&self) -> f64 {
        if self.rate.is_finite() {
            self.rate.clamp(MIN_RATE, MAX_RATE)
        } else {
            1.0
        }
    }

    /// How many passes a looping cue makes; None = forever.
    pub fn loop_passes(&self) -> Option<u32> {
        (self.loop_count > 0).then_some(self.loop_count)
    }

    /// Whether a decoder started for `other` would do for this cue too: the
    /// same file, the same slice of it, looped the same way, fitted and
    /// routed the same way. Levels, fades, geometry and colour don't change
    /// what is decoded, so editing them leaves a preload alone.
    pub fn decodes_like(&self, other: &MediaCue) -> bool {
        self.file == other.file
            && self.fit == other.fit
            && self.start == other.start
            && self.end == other.end
            && self.loop_media == other.loop_media
            && self.loop_count == other.loop_count
            && self.speed() == other.speed()
            && self.audio_output == other.audio_output
            && self.audio_channel == other.audio_channel
            && self.sound_chain() == other.sound_chain()
    }

    /// The sound effects, in order, whether or not they are on. A voice is
    /// built with them (switching one on or off is done live), so a change
    /// here is a change to what must be decoded for.
    pub fn sound_chain(&self) -> Vec<EffectKind> {
        self.effects
            .iter()
            .filter(|e| !e.kind.is_video())
            .map(|e| e.kind)
            .collect()
    }

    /// How this cue's decoders should run, given the file's duration if known.
    pub fn playback(&self, total: Option<f64>) -> Playback {
        Playback::plan(
            self.start,
            self.end,
            total,
            self.loop_media,
            self.loop_passes(),
        )
    }

    /// The fade time to use when this cue is faded out by hand.
    pub fn manual_fade_out(&self) -> f64 {
        if self.fade_out > 0.0 {
            self.fade_out
        } else {
            DEFAULT_FADE_OUT
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FadeCue {
    pub target: Option<CueId>,
    pub duration: f64,
    #[serde(default)]
    pub curve: Curve,
    /// Target opacity (visual cues), None = leave unchanged.
    pub opacity: Option<f32>,
    /// Target volume in dB, None = leave unchanged.
    pub volume_db: Option<f32>,
    /// Stop the target when the fade completes.
    pub stop_target: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StopCue {
    /// None = stop everything.
    pub target: Option<CueId>,
    pub fade: f64,
    /// Fade each stopped cue out with its own fade-out time instead of `fade`.
    #[serde(default)]
    pub use_own_fade: bool,
}

/// What a control cue does to its target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlAction {
    /// Fire the target, as if it were fired by hand (honouring its pre-wait).
    #[default]
    Start,
    /// Hold the target (and a group's children) where it is.
    Pause,
    /// Let a paused target go on.
    Resume,
    /// Get the target ready ahead of time, so starting it is instant.
    Load,
    /// Stop the target at once and forget what was ready of it.
    Reset,
    /// Put the playhead on the target (without firing it).
    GoTo,
    /// Arm the target, so GO and continues fire it.
    Arm,
    /// Disarm the target, so it is passed over as if it weren't there.
    Disarm,
    /// Let a loop that is playing finish its pass and go on, instead of going round again.
    Devamp,
    /// Does nothing: a note in the cue list.
    Memo,
}

impl ControlAction {
    pub const ALL: [ControlAction; 10] = [
        ControlAction::Start,
        ControlAction::Pause,
        ControlAction::Resume,
        ControlAction::Load,
        ControlAction::Reset,
        ControlAction::GoTo,
        ControlAction::Arm,
        ControlAction::Disarm,
        ControlAction::Devamp,
        ControlAction::Memo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ControlAction::Start => "Start",
            ControlAction::Pause => "Pause",
            ControlAction::Resume => "Resume",
            ControlAction::Load => "Load",
            ControlAction::Reset => "Reset",
            ControlAction::GoTo => "Go to",
            ControlAction::Arm => "Arm",
            ControlAction::Disarm => "Disarm",
            ControlAction::Devamp => "Devamp",
            ControlAction::Memo => "Memo",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ControlAction::Start => "Fire the target as if by hand, whatever the playhead is on",
            ControlAction::Pause => "Hold the target where it is, picture and sound",
            ControlAction::Resume => "Let a paused target carry on",
            ControlAction::Load => "Get the target ready ahead of time, so it starts at once",
            ControlAction::Reset => "Stop the target at once and forget what was ready of it",
            ControlAction::GoTo => "Put the playhead on the target, without firing it",
            ControlAction::Arm => "Arm the target so GO and continues fire it",
            ControlAction::Disarm => "Disarm the target so it is passed over",
            ControlAction::Devamp => {
                "Let a loop that is playing finish its pass and go on, rather than go round again"
            }
            ControlAction::Memo => "Does nothing: a note in the cue list",
        }
    }

    /// Whether it acts on a cue.
    pub fn needs_target(self) -> bool {
        self != ControlAction::Memo
    }
}

/// A cue that controls other cues (or the list). It takes no time.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ControlCue {
    #[serde(default)]
    pub action: ControlAction,
    pub target: Option<CueId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WaitCue {
    pub duration: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

impl TextAlign {
    pub const ALL: [TextAlign; 3] = [TextAlign::Left, TextAlign::Center, TextAlign::Right];
    pub fn label(self) -> &'static str {
        match self {
            TextAlign::Left => "Left",
            TextAlign::Center => "Centre",
            TextAlign::Right => "Right",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextVAlign {
    Top,
    Middle,
    #[default]
    Bottom,
}

impl TextVAlign {
    pub const ALL: [TextVAlign; 3] = [TextVAlign::Top, TextVAlign::Middle, TextVAlign::Bottom];
    pub fn label(self) -> &'static str {
        match self {
            TextVAlign::Top => "Top",
            TextVAlign::Middle => "Middle",
            TextVAlign::Bottom => "Bottom",
        }
    }
}

/// Text drawn over an output, such as a title or surtitle. It plays like an
/// image cue (its `media` holds the output, layer, transition, fades and
/// hold time), with the picture made from the text instead of a file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextCue {
    pub text: String,
    /// Font size as a fraction of the output's height.
    pub size: f32,
    /// Straight (not premultiplied) RGBA.
    pub color: [u8; 4],
    pub align: TextAlign,
    /// Where the block of text sits on the output (before any geometry).
    pub valign: TextVAlign,
    /// Width available to the text before it wraps, as a fraction of the output's width.
    pub width: f32,
    /// A dark shadow behind the letters, so they read over video.
    pub shadow: bool,
    /// Thicken the letters (the bundled font is a light weight).
    #[serde(default)]
    pub bold: bool,
    /// Where the words come from: typed in, or the time of day or a timer
    /// that the text shows as it runs. See `live_text`.
    #[serde(default, skip_serializing_if = "TextSource::is_static")]
    pub source: TextSource,
    pub media: MediaCue,
}

/// What a text cue shows. A live source replaces `{time}` in the cue's text
/// with the time (or is the whole text when there is no `{time}`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum TextSource {
    #[default]
    Static,
    /// The time of day on this computer.
    Clock { hour24: bool, seconds: bool },
    /// A speaker timer: counts `total` seconds down (turning amber at `warn`
    /// seconds left and red at zero, then counting the overrun), or up.
    Timer { total: f64, up: bool, warn: f64 },
}

impl TextSource {
    pub fn is_static(&self) -> bool {
        matches!(self, TextSource::Static)
    }
    pub fn is_live(&self) -> bool {
        !self.is_static()
    }
}

impl TextCue {
    pub fn new(output: Option<OutputId>) -> TextCue {
        let mut media = MediaCue::new(PathBuf::new(), output);
        media.fit = Fit::Stretch;
        media.fade_in = 0.5;
        media.fade_out = 0.5;
        TextCue {
            text: "Text".into(),
            size: 0.07,
            color: [255, 255, 255, 255],
            align: TextAlign::Center,
            valign: TextVAlign::Bottom,
            width: 0.9,
            shadow: true,
            bold: false,
            source: TextSource::Static,
            media,
        }
    }

    /// A clock: the time of day, large and centred.
    pub fn clock(output: Option<OutputId>) -> TextCue {
        let mut t = TextCue::new(output);
        t.text = crate::live_text::PLACEHOLDER.into();
        t.source = TextSource::Clock {
            hour24: true,
            seconds: false,
        };
        t.size = 0.2;
        t.valign = TextVAlign::Middle;
        t
    }

    /// A ten minute countdown for a speaker, amber for the last minute.
    pub fn timer(output: Option<OutputId>) -> TextCue {
        let mut t = TextCue::clock(output);
        t.source = TextSource::Timer {
            total: 600.0,
            up: false,
            warn: 60.0,
        };
        t
    }
}

/// A cue that sends a message out when it fires: OSC to another program or
/// desk, or MIDI to a device. It takes no time; Continue works as for a Stop cue.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SendCue {
    pub target: SendTarget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SendTarget {
    Osc(OscOut),
    Midi(MidiOut),
    /// Power, shutter or input of a projector, over PJLink.
    Projector(ProjectorOut),
    /// Bytes to any device on the network, over TCP or UDP.
    Network(NetworkOut),
}

impl Default for SendTarget {
    fn default() -> Self {
        SendTarget::Osc(OscOut::default())
    }
}

/// A command for a projector that speaks PJLink (class 1, TCP port 4352).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectorOut {
    pub host: String,
    pub port: u16,
    /// The PJLink password, if the projector has one. It is kept in the show
    /// file as typed.
    #[serde(default)]
    pub password: String,
    pub action: ProjectorAction,
}

impl Default for ProjectorOut {
    fn default() -> Self {
        ProjectorOut {
            host: "192.168.0.100".into(),
            port: 4352,
            password: String::new(),
            action: ProjectorAction::PowerOn,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectorAction {
    PowerOn,
    PowerOff,
    /// Close the shutter: mute the picture (the lamp stays on).
    ShutterClosed,
    ShutterOpen,
    /// Select an input: PJLink's two digits, the kind (1 RGB, 2 video, 3
    /// digital, 4 storage, 5 network) then its number.
    Input(u8),
}

impl ProjectorAction {
    pub fn label(self) -> String {
        match self {
            ProjectorAction::PowerOn => "Power on".into(),
            ProjectorAction::PowerOff => "Power off".into(),
            ProjectorAction::ShutterClosed => "Shutter closed".into(),
            ProjectorAction::ShutterOpen => "Shutter open".into(),
            ProjectorAction::Input(code) => format!("Input {}", input_name(code)),
        }
    }
}

/// An input as PJLink numbers it: "HDMI/DVI 1" for 31.
pub fn input_name(code: u8) -> String {
    let kind = match code / 10 {
        1 => "RGB",
        2 => "Video",
        3 => "Digital",
        4 => "Storage",
        5 => "Network",
        _ => "?",
    };
    format!("{kind} {}", code % 10)
}

/// Bytes sent to a host and port, as typed with escapes (see `output::unescape`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetworkOut {
    pub host: String,
    pub port: u16,
    /// TCP (connect, send, close) or, when false, UDP (one packet).
    pub tcp: bool,
    pub payload: String,
}

impl Default for NetworkOut {
    fn default() -> Self {
        NetworkOut {
            host: "192.168.0.100".into(),
            port: 23,
            tcp: true,
            payload: String::new(),
        }
    }
}

/// An OSC message to a UDP address.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscOut {
    /// An address or a name; an address avoids a name lookup.
    pub host: String,
    pub port: u16,
    pub address: String,
    /// Arguments separated by spaces: whole numbers, decimals, or text (in
    /// double quotes if it has spaces).
    #[serde(default)]
    pub args: String,
}

impl Default for OscOut {
    fn default() -> Self {
        OscOut {
            host: "127.0.0.1".into(),
            port: 53000,
            address: "/go".into(),
            args: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MscCommand {
    Go,
    Stop,
    Resume,
    Load,
    AllOff,
    Reset,
}

impl MscCommand {
    pub fn label(self) -> &'static str {
        match self {
            MscCommand::Go => "GO",
            MscCommand::Stop => "STOP",
            MscCommand::Resume => "RESUME",
            MscCommand::Load => "LOAD",
            MscCommand::AllOff => "ALL OFF",
            MscCommand::Reset => "RESET",
        }
    }
    /// Whether the command takes a cue number.
    pub fn takes_cue(self) -> bool {
        !matches!(
            self,
            MscCommand::Resume | MscCommand::AllOff | MscCommand::Reset
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiKind {
    NoteOn,
    NoteOff,
    ControlChange,
    ProgramChange,
    /// MIDI Show Control, which lighting and sound desks understand.
    ShowControl(MscCommand),
}

impl MidiKind {
    pub const ALL: [MidiKind; 10] = [
        MidiKind::NoteOn,
        MidiKind::NoteOff,
        MidiKind::ControlChange,
        MidiKind::ProgramChange,
        MidiKind::ShowControl(MscCommand::Go),
        MidiKind::ShowControl(MscCommand::Stop),
        MidiKind::ShowControl(MscCommand::Resume),
        MidiKind::ShowControl(MscCommand::Load),
        MidiKind::ShowControl(MscCommand::AllOff),
        MidiKind::ShowControl(MscCommand::Reset),
    ];
    pub fn label(self) -> String {
        match self {
            MidiKind::NoteOn => "Note on".into(),
            MidiKind::NoteOff => "Note off".into(),
            MidiKind::ControlChange => "Control change".into(),
            MidiKind::ProgramChange => "Program change".into(),
            MidiKind::ShowControl(c) => format!("Show Control {}", c.label()),
        }
    }
}

/// A MIDI message to an output port.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MidiOut {
    /// The output's name; None until one is chosen.
    pub port: Option<String>,
    pub kind: MidiKind,
    /// Channel 0 to 15 (shown as 1 to 16).
    pub channel: u8,
    /// Note, controller or program number (0 to 127).
    pub data1: u8,
    /// Velocity or controller value (0 to 127).
    pub data2: u8,
    /// The cue number for Show Control; empty for the desk's next cue.
    #[serde(default)]
    pub cue: String,
}

impl Default for MidiOut {
    fn default() -> Self {
        MidiOut {
            port: None,
            kind: MidiKind::NoteOn,
            channel: 0,
            data1: 60,
            data2: 100,
            cue: String::new(),
        }
    }
}

/// The device name that stands for NewQ's built-in moving test picture, so a
/// camera cue can be tried without a camera.
pub const TEST_PATTERN_DEVICE: &str = "NewQ test pattern";

/// A live picture from a camera or capture card, shown as a layer. It has the
/// same output, layer, geometry and transition settings as a video cue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraCue {
    /// The device as the operating system names it (see `media::camera`);
    /// `TEST_PATTERN_DEVICE` for the test picture, empty if none is chosen.
    pub device: String,
    /// The mode asked of the device. It must be one the device supports.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Where, when and how it shows. `end` is how long it runs (None = until stopped).
    pub media: MediaCue,
}

/// Sound from an audio input (a Dante Virtual Soundcard, a mixer, a line
/// input) played live on one of the show's audio outputs. Routing, volume,
/// fades and a time to run for are the shared media settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveAudioCue {
    /// The input device by its name; None is the system's default input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The first input channel, counting from 0: it is the left, the next the
    /// right (the same again if there is no next).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input_channel: u16,
    /// `end` is how long it runs (None = until stopped).
    pub media: MediaCue,
}

impl Default for LiveAudioCue {
    fn default() -> Self {
        LiveAudioCue {
            device: None,
            input_channel: 0,
            media: MediaCue::new(PathBuf::new(), None),
        }
    }
}

impl CameraCue {
    pub fn new(output: Option<OutputId>) -> CameraCue {
        let mut media = MediaCue::new_visual(PathBuf::new(), output);
        media.fit = Fit::Contain;
        CameraCue {
            device: String::new(),
            width: 1280,
            height: 720,
            fps: 30,
            media,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupMode {
    /// Every child is fired when the group is; a child's pre-wait is its
    /// offset from the start of the group.
    #[default]
    Timeline,
    /// The children play one after another: each is fired when the one
    /// before it has finished.
    Playlist,
}

impl GroupMode {
    pub const ALL: [GroupMode; 2] = [GroupMode::Timeline, GroupMode::Playlist];

    pub fn label(self) -> &'static str {
        match self {
            GroupMode::Timeline => "Start all together",
            GroupMode::Playlist => "Play in order",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            GroupMode::Timeline => {
                "Every child starts with the group; a child's pre-wait is its offset from the start."
            }
            GroupMode::Playlist => {
                "Each child starts when the one before it finishes (after its own pre-wait)."
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct GroupCue {
    #[serde(default)]
    pub mode: GroupMode,
    /// Hide the children in the cue list.
    #[serde(default)]
    pub collapsed: bool,
    /// A slide show: a group of images that crossfade one into the next. The
    /// group's children are timed from these settings (and each slide's own,
    /// if it has them); see `Show::relay_slides`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slides: Option<SlideShow>,
}

/// What every slide in a slide show gets unless it has its own.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct SlideShow {
    /// Seconds each slide takes to fade in over the one before.
    pub fade: f64,
    /// Seconds each slide is shown in full, between its fade and the next.
    pub duration: f64,
}

impl Default for SlideShow {
    fn default() -> Self {
        SlideShow {
            fade: 1.0,
            duration: 5.0,
        }
    }
}

impl Show {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn cue(&self, id: CueId) -> Option<&Cue> {
        self.cues.iter().find(|c| c.id == id)
    }

    #[cfg(test)]
    pub fn cue_mut(&mut self, id: CueId) -> Option<&mut Cue> {
        self.cues.iter_mut().find(|c| c.id == id)
    }

    pub fn cue_index(&self, id: CueId) -> Option<usize> {
        self.cues.iter().position(|c| c.id == id)
    }

    pub fn output(&self, id: OutputId) -> Option<&OutputConfig> {
        self.outputs.iter().find(|o| o.id == id)
    }

    /// Next unused integer cue number, following the highest numeric one.
    pub fn next_cue_number(&self) -> String {
        let max = self
            .cues
            .iter()
            .filter_map(|c| c.number.trim().parse::<f64>().ok())
            .fold(0.0_f64, f64::max);
        format!("{}", max.floor() as i64 + 1)
    }

    /// The display an output is on: itself, or for a sub display the display
    /// it is an area of.
    pub fn display_of<'a>(&'a self, o: &'a OutputConfig) -> Option<&'a OutputConfig> {
        match o.within {
            None => Some(o),
            Some(d) => self
                .outputs
                .iter()
                .find(|x| x.id == d && x.within.is_none()),
        }
    }

    /// Add a display, not assigned to a monitor yet.
    pub fn add_display(&mut self) -> OutputId {
        let id = OutputId(self.alloc_id());
        let n = self.outputs.iter().filter(|o| !o.is_sub_display()).count() + 1;
        self.outputs
            .push(OutputConfig::new(id, format!("Display {n}")));
        id
    }

    /// Set up two projectors that share one wide picture: a display for the
    /// picture (a `width` wide `height` tall one, `overlap` of a projector's
    /// width of it shown by both), and a display for each projector showing
    /// its half, fading out where they overlap. Returns the picture's display
    /// and the left and right projector's. They still need monitors.
    pub fn add_blended_pair(
        &mut self,
        width: u32,
        height: u32,
        overlap: f32,
    ) -> (OutputId, OutputId, OutputId) {
        let overlap_px = (width as f32 * overlap).round().max(0.0);
        let total = (2 * width) as f32 - overlap_px;
        let share = width as f32 / total;
        let picture = self.add_display();
        let left = self.add_display();
        let right = self.add_display();
        let set = |show: &mut Show, id: OutputId, name: &str, f: &dyn Fn(&mut OutputConfig)| {
            if let Some(o) = show.outputs.iter_mut().find(|o| o.id == id) {
                o.name = name.to_string();
                f(o);
            }
        };
        set(self, picture, "Wide picture", &|o| {
            o.picture = Some((total as u32, height));
        });
        set(self, left, "Left projector", &|o| {
            o.part_of = Some(PartOf {
                display: picture,
                area: [0.0, 0.0, share, 1.0],
            });
            o.blend.right = overlap;
        });
        set(self, right, "Right projector", &|o| {
            o.part_of = Some(PartOf {
                display: picture,
                area: [1.0 - share, 0.0, share, 1.0],
            });
            o.blend.left = overlap;
        });
        (picture, left, right)
    }

    /// The part of another display's picture that display `id` shows, if it
    /// shows one: only a display can be shown from, not a sub display, and not
    /// one that itself shows part of another (nor itself).
    pub fn part_of(&self, id: OutputId) -> Option<PartOf> {
        let part = self.output(id)?.part_of?;
        let source = self.output(part.display)?;
        (part.display != id && !source.is_sub_display() && source.part_of.is_none()).then_some(part)
    }

    /// Whether any display shows part of display `id`'s picture.
    pub fn is_shared(&self, id: OutputId) -> bool {
        self.outputs
            .iter()
            .any(|o| o.part_of.is_some_and(|p| p.display == id) && self.part_of(o.id).is_some())
    }

    /// Put right what a hand-edited show file may hold: shared pictures that
    /// don't make sense, widths and sizes out of range.
    fn sanitize_blending(&mut self) {
        let valid: Vec<(OutputId, bool)> = self
            .outputs
            .iter()
            .map(|o| (o.id, self.part_of(o.id).is_some()))
            .collect();
        for o in &mut self.outputs {
            if o.part_of.is_some() && !valid.iter().any(|(id, ok)| *id == o.id && *ok) {
                o.part_of = None;
            }
            if let Some(p) = &mut o.part_of {
                let [x, y, w, h] = p.area;
                if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
                    p.area = [0.0, 0.0, 1.0, 1.0];
                }
            }
            o.blend = o.blend.clamped();
            o.stream = o.stream.as_ref().map(StreamOut::clamped);
            if let Some((w, h)) = &mut o.picture {
                (*w, *h) = ((*w).clamp(1, 16_384), (*h).clamp(1, 16_384));
            }
        }
    }

    /// Add a sub display on `display`: the left half of it, then the right
    /// half, then a centred area.
    pub fn add_sub_display(&mut self, display: OutputId) -> OutputId {
        let id = OutputId(self.alloc_id());
        let siblings = self
            .outputs
            .iter()
            .filter(|o| o.within == Some(display))
            .count();
        let area = match siblings {
            0 => Corners::rect(0.0, 0.0, 0.5, 1.0),
            1 => Corners::rect(0.5, 0.0, 0.5, 1.0),
            _ => Corners::rect(0.25, 0.25, 0.5, 0.5),
        };
        let n = self.outputs.iter().filter(|o| o.is_sub_display()).count() + 1;
        let mut o = OutputConfig::new(id, format!("Sub display {n}"));
        o.within = Some(display);
        o.area = area;
        self.outputs.push(o);
        id
    }

    /// Remove a display or sub display (and a display's sub displays). Cues
    /// that were sent to what went are sent to the first display left.
    pub fn remove_output(&mut self, id: OutputId) {
        self.outputs.retain(|o| o.id != id && o.within != Some(id));
        // What showed part of a display that has gone shows its own picture.
        for o in &mut self.outputs {
            if o.part_of.is_some_and(|p| p.display == id) {
                o.part_of = None;
            }
        }
        let fallback = self.outputs.first().map(|o| o.id);
        let gone =
            |out: Option<OutputId>| out.is_some_and(|o| self.outputs.iter().all(|x| x.id != o));
        let retarget: Vec<CueId> = self
            .cues
            .iter()
            .filter(|c| {
                c.kind.cue_type() != CueType::Audio
                    && c.kind.media().is_some_and(|m| gone(m.output))
            })
            .map(|c| c.id)
            .collect();
        for c in &mut self.cues {
            if retarget.contains(&c.id)
                && let Some(m) = c.kind.media_mut()
            {
                m.output = fallback;
            }
        }
    }

    /// Whether some output is waiting for a monitor to be picked for it (see
    /// `assign_automatic_displays`).
    pub fn needs_automatic_assignment(&self) -> bool {
        self.outputs.iter().any(|o| o.needs_monitor)
    }

    /// Give each output of an older show that was "automatic" a monitor, once:
    /// the first one the main window isn't on, as it used to pick, which is
    /// then its monitor for good. Returns what it did, for the log.
    pub fn assign_automatic_displays(
        &mut self,
        monitors: &[MonitorKey],
        control: Option<usize>,
    ) -> Vec<String> {
        let mut notes = Vec::new();
        let mut claimed: std::collections::HashSet<usize> = self
            .outputs
            .iter()
            .filter(|o| !o.is_sub_display())
            .filter_map(
                |o| match display_state(o.monitor.as_ref(), o.twins, monitors, control) {
                    DisplayState::Connected(i) | DisplayState::OnMainWindowDisplay(i) => Some(i),
                    _ => None,
                },
            )
            .collect();
        for o in &mut self.outputs {
            if !std::mem::take(&mut o.needs_monitor) {
                continue;
            }
            let free = (0..monitors.len()).find(|m| Some(*m) != control && !claimed.contains(m));
            match free {
                Some(m) => {
                    claimed.insert(m);
                    o.twins = twins_of(monitors, m);
                    notes.push(format!("{} is on {}", o.name, monitors[m].identity));
                    o.monitor = Some(monitors[m].reference(m));
                }
                None => notes.push(format!(
                    "{} stays in the main window (no other display)",
                    o.name
                )),
            }
        }
        notes
    }

    /// A display assigned to a monitor before hardware ids were known learns its
    /// monitor's, once it is connected, so that it is found by them from now on.
    /// Returns what it learned, for the log.
    pub fn learn_hardware_ids(
        &mut self,
        monitors: &[MonitorKey],
        control: Option<usize>,
    ) -> Vec<String> {
        let mut notes = Vec::new();
        for o in self.outputs.iter_mut().filter(|o| o.within.is_none()) {
            let state = display_state(o.monitor.as_ref(), o.twins, monitors, control);
            let (DisplayState::Connected(i) | DisplayState::OnMainWindowDisplay(i)) = state else {
                continue;
            };
            if let Some(r) = &mut o.monitor
                && r.hardware.is_none()
                && let Some(h) = &monitors[i].hardware
            {
                r.hardware = Some(h.clone());
                r.name = monitors[i].identity.clone();
                o.twins = twins_of(monitors, i);
                notes.push(format!("{} is monitor {}", o.name, monitors[i].identity));
            }
        }
        notes
    }

    /// A display of the show for `monitor`, made if there isn't one.
    fn display_for(&mut self, name: String, monitor: Option<DisplayRef>, twins: u32) -> OutputId {
        let id = OutputId(self.alloc_id());
        let mut o = OutputConfig::new(id, name);
        o.monitor = monitor;
        o.twins = twins;
        self.outputs.push(o);
        id
    }

    /// Fold what shows from before displays and outputs were one thing
    /// recorded into displays and sub displays. Outputs become displays (or
    /// sub displays, if they were on one) with the same ids, so cues keep
    /// pointing at them; a second output that shared a display becomes a
    /// sub display covering all of it.
    pub(crate) fn migrate_legacy(&mut self) {
        if self.version >= 2 {
            return;
        }
        self.version = 2;
        let slots = std::mem::take(&mut self.legacy_displays);
        let subs = std::mem::take(&mut self.legacy_sub_displays);
        // The display that stands for each older display, and the output
        // that took over each older sub display.
        let mut slot_display: HashMap<u64, OutputId> = HashMap::new();
        let mut sub_output: HashMap<u64, OutputId> = HashMap::new();
        let mut on_subs: Vec<(usize, u64)> = Vec::new();
        for i in 0..self.outputs.len() {
            let o = &mut self.outputs[i];
            let placement = match (o.placement.take(), o.display.take()) {
                (Some(p), _) => p,
                (None, Some(d)) => LegacyPlacement::Display(d),
                (None, None) => LegacyPlacement::Auto,
            };
            let id = o.id;
            match placement {
                LegacyPlacement::Display(r) => o.monitor = Some(r),
                LegacyPlacement::MainWindow => {}
                LegacyPlacement::Auto => o.needs_monitor = true,
                LegacyPlacement::Slot(s) => match slot_display.get(&s) {
                    Some(&first) => o.within = Some(first),
                    None => {
                        if let Some(sl) = slots.iter().find(|x| x.id == s) {
                            o.monitor = sl.monitor.clone();
                            o.twins = sl.twins;
                        }
                        slot_display.insert(s, id);
                    }
                },
                LegacyPlacement::SubDisplay(v) => on_subs.push((i, v)),
            }
        }
        // The display an older sub display was part of.
        let mut parent_of = |show: &mut Show, sub: &LegacySubDisplay| -> OutputId {
            if let Some(s) = sub.display {
                if let Some(&d) = slot_display.get(&s) {
                    return d;
                }
                let sl = slots.iter().find(|x| x.id == s);
                let d = show.display_for(
                    sl.map_or("Display".to_string(), |x| x.name.clone()),
                    sl.and_then(|x| x.monitor.clone()),
                    sl.map_or(1, |x| x.twins),
                );
                slot_display.insert(s, d);
                return d;
            }
            if !is_unnamed(&sub.parent) {
                let found = show
                    .outputs
                    .iter()
                    .find(|o| !o.is_sub_display() && o.monitor.as_ref() == Some(&sub.parent));
                if let Some(o) = found {
                    return o.id;
                }
                return show.display_for(sub.parent.name.clone(), Some(sub.parent.clone()), 1);
            }
            show.display_for("Display".into(), None, 1)
        };
        for (i, v) in on_subs {
            let Some(sub) = subs.iter().find(|x| x.id == v) else {
                // Its sub display was removed: only the preview shows it.
                continue;
            };
            let parent = parent_of(self, sub);
            let id = self.outputs[i].id;
            self.outputs[i].within = Some(parent);
            self.outputs[i].area = sub.area;
            sub_output.entry(v).or_insert(id);
        }
        // Sub displays nothing was on still exist.
        for sub in &subs {
            if sub_output.contains_key(&sub.id) {
                continue;
            }
            let parent = parent_of(self, sub);
            let id = OutputId(self.alloc_id());
            let mut o = OutputConfig::new(id, sub.name.clone());
            o.within = Some(parent);
            o.area = sub.area;
            self.outputs.push(o);
        }
        // Displays nothing was on still exist, with their monitors.
        for sl in &slots {
            if let std::collections::hash_map::Entry::Vacant(e) = slot_display.entry(sl.id) {
                e.insert(self.display_for(sl.name.clone(), sl.monitor.clone(), sl.twins));
            }
        }
    }

    /// Milliseconds sound is held back on an audio output (None = the first,
    /// which is the default): the show's own plus the output's.
    pub fn audio_delay_ms(&self, output: Option<AudioOutputId>) -> u32 {
        let own = output
            .and_then(|id| self.audio_outputs.iter().find(|o| o.id == id))
            .or_else(|| self.audio_outputs.first())
            .map_or(0, |o| o.delay_ms);
        (self.sync.audio_ms + own).min(MAX_DELAY_MS)
    }

    /// Milliseconds pictures are held back on a video output.
    pub fn video_delay_ms(&self, output: Option<OutputId>) -> u32 {
        let own = output
            .and_then(|id| self.outputs.iter().find(|o| o.id == id))
            .map_or(0, |o| o.delay_ms);
        (self.sync.video_ms + own).min(MAX_DELAY_MS)
    }

    pub fn add_audio_output(&mut self) -> AudioOutputId {
        let id = AudioOutputId(self.alloc_id());
        let n = self.audio_outputs.len() + 1;
        self.audio_outputs.push(AudioOutputConfig {
            id,
            name: format!("Audio {n}"),
            device: None,
            delay_ms: 0,
        });
        id
    }

    /// Remove an audio output; cues that used it go back to the default.
    pub fn remove_audio_output(&mut self, id: AudioOutputId) {
        self.audio_outputs.retain(|o| o.id != id);
        for c in &mut self.cues {
            if let Some(m) = c.kind.media_mut()
                && m.audio_output == Some(id)
            {
                m.audio_output = None;
            }
        }
    }

    /// Build a new cue of the given type, inserting sensible defaults
    /// based on the cue that precedes `insert_at`.
    pub fn make_cue(&mut self, ty: CueType, file: Option<PathBuf>, insert_at: usize) -> Cue {
        let id = CueId(self.alloc_id());
        let number = self.next_cue_number();
        let default_output = self.outputs.first().map(|o| o.id);
        let previous_media = self.cues[..insert_at.min(self.cues.len())]
            .iter()
            .rev()
            .find(|c| c.kind.media().is_some())
            .map(|c| c.id);
        let file_name = file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map(|s| s.to_string_lossy().into_owned());
        let (name, kind) = match ty {
            CueType::Video => (
                file_name.unwrap_or_else(|| "Video".into()),
                CueKind::Video(MediaCue::new_visual(
                    file.unwrap_or_default(),
                    default_output,
                )),
            ),
            CueType::Image => (
                file_name.unwrap_or_else(|| "Image".into()),
                CueKind::Image(MediaCue::new_visual(
                    file.unwrap_or_default(),
                    default_output,
                )),
            ),
            CueType::Audio => (
                file_name.unwrap_or_else(|| "Audio".into()),
                CueKind::Audio(MediaCue::new(file.unwrap_or_default(), None)),
            ),
            CueType::Fade => (
                "Fade out".into(),
                CueKind::Fade(FadeCue {
                    target: previous_media,
                    duration: 3.0,
                    curve: Curve::SCurve,
                    opacity: Some(0.0),
                    volume_db: Some(MIN_DB),
                    stop_target: true,
                }),
            ),
            CueType::Stop => (
                "Stop all".into(),
                CueKind::Stop(StopCue {
                    target: None,
                    fade: 0.0,
                    use_own_fade: true,
                }),
            ),
            CueType::Wait => ("Wait".into(), CueKind::Wait(WaitCue { duration: 5.0 })),
            CueType::Group => ("Group".into(), CueKind::Group(GroupCue::default())),
            CueType::Text => ("Text".into(), CueKind::Text(TextCue::new(default_output))),
            CueType::Clock => (
                "Clock".into(),
                CueKind::Text(TextCue::clock(default_output)),
            ),
            CueType::Timer => (
                "Speaker timer".into(),
                CueKind::Text(TextCue::timer(default_output)),
            ),
            CueType::Camera => (
                "Camera".into(),
                CueKind::Camera(CameraCue::new(default_output)),
            ),
            CueType::Send => ("Send".into(), CueKind::Send(SendCue::default())),
            CueType::LiveAudio => (
                "Live audio".into(),
                CueKind::LiveAudio(LiveAudioCue::default()),
            ),
            CueType::Control => (
                ControlAction::default().label().into(),
                CueKind::Control(ControlCue::default()),
            ),
        };
        Cue {
            id,
            number,
            name,
            notes: String::new(),
            pre_wait: 0.0,
            continue_mode: ContinueMode::DoNotContinue,
            armed: true,
            parent: None,
            hotkey: None,
            midi_note: None,
            cart: false,
            timecode: None,
            clock: None,
            color: None,
            kind,
        }
    }

    /// The cue a key fires.
    pub fn cue_for_hotkey(&self, key: &str) -> Option<CueId> {
        self.cues
            .iter()
            .find(|c| c.hotkey.as_deref() == Some(key))
            .map(|c| c.id)
    }

    /// The cue a MIDI note fires.
    pub fn cue_for_note(&self, note: u8) -> Option<CueId> {
        self.cues
            .iter()
            .find(|c| c.midi_note == Some(note))
            .map(|c| c.id)
    }

    /// Give a cue a hotkey (or take it away). A key fires one cue, so any
    /// other cue that had it loses it.
    pub fn set_hotkey(&mut self, id: CueId, key: Option<String>) {
        if key.is_some() {
            for c in &mut self.cues {
                if c.hotkey == key {
                    c.hotkey = None;
                }
            }
        }
        if let Some(c) = self.cues.iter_mut().find(|c| c.id == id) {
            c.hotkey = key;
        }
    }

    /// Give a cue a MIDI note (or take it away), taking it from any other cue.
    pub fn set_midi_note(&mut self, id: CueId, note: Option<u8>) {
        if note.is_some() {
            for c in &mut self.cues {
                if c.midi_note == note {
                    c.midi_note = None;
                }
            }
        }
        if let Some(c) = self.cues.iter_mut().find(|c| c.id == id) {
            c.midi_note = note;
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Show> {
        let text = std::fs::read_to_string(path)?;
        let mut show: Show = serde_json::from_str(&text)?;
        let base = path.parent().unwrap_or(Path::new("."));
        for cue in &mut show.cues {
            if let Some(m) = cue.kind.media_mut()
                && std::mem::take(&mut m.crossfade)
            {
                m.transition = Transition::Crossfade;
            }
            if let Some(m) = cue.kind.media_mut()
                && m.file.is_relative()
                && !m.file.as_os_str().is_empty()
            {
                m.file = base.join(&m.file);
            }
            if let Some(m) = cue.kind.media_mut() {
                for e in &mut m.effects {
                    e.sanitize();
                }
            }
        }
        show.normalize_groups();
        show.relay_slides();
        // A mesh that doesn't make sense (a hand-edited file) is dropped, not
        // trusted with the picture.
        for o in &mut show.outputs {
            if o.mesh.as_ref().is_some_and(|m| !m.is_valid()) {
                o.mesh = None;
            }
        }
        show.sanitize_blending();
        // Guard against hand-edited files with stale counters.
        let max_id = show
            .cues
            .iter()
            .map(|c| c.id.0)
            .chain(show.outputs.iter().map(|o| o.id.0))
            .chain(show.audio_outputs.iter().map(|o| o.id.0))
            .max()
            .unwrap_or(0);
        show.next_id = show.next_id.max(max_id + 1);
        show.migrate_legacy();
        Ok(show)
    }

    /// Save with media paths made relative to the show file where possible,
    /// so a show folder can be moved between machines.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let base = path.parent().unwrap_or(Path::new("."));
        let mut copy = self.clone();
        for cue in &mut copy.cues {
            if let Some(m) = cue.kind.media_mut()
                && let Ok(rel) = m.file.strip_prefix(base)
            {
                m.file = rel.to_path_buf();
            }
        }
        let text = serde_json::to_string_pretty(&copy)?;
        // Write beside the file and rename over it, so a crash mid-write
        // can't leave a half-written show.
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = path.with_file_name(tmp_name);
        let written = (|| {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            // On the disk before it replaces the old show: a power cut just
            // after the rename can't leave an empty file where the show was.
            file.sync_all()
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}

// ----- Groups ---------------------------------------------------------------
//
// Cues are one flat list. A group is followed directly by its children (cues
// whose `parent` is the group), and groups don't nest; together they form a
// "block" that moves, duplicates and deletes as a unit.

/// Where a new cue goes, and which group it joins.
pub struct Slot {
    pub at: usize,
    pub parent: Option<CueId>,
}

impl Show {
    pub fn is_group(&self, idx: usize) -> bool {
        matches!(self.cues[idx].kind, CueKind::Group(_))
    }

    /// Index of the top-level cue that `idx` belongs to: its group, or itself.
    pub fn top_index(&self, idx: usize) -> usize {
        self.cues[idx]
            .parent
            .and_then(|p| self.cue_index(p))
            .unwrap_or(idx)
    }

    /// The top-level cue `id` belongs to: its group, or itself.
    pub fn top_id(&self, id: CueId) -> CueId {
        self.cue_index(id)
            .map_or(id, |i| self.cues[self.top_index(i)].id)
    }

    /// One past the last cue of the block starting at top-level cue `idx`.
    pub fn block_end(&self, idx: usize) -> usize {
        let id = self.cues[idx].id;
        let mut end = idx + 1;
        if self.is_group(idx) {
            while end < self.cues.len() && self.cues[end].parent == Some(id) {
                end += 1;
            }
        }
        end
    }

    /// The top-level cue after the block containing `idx`.
    pub fn next_top(&self, idx: usize) -> Option<usize> {
        let end = self.block_end(self.top_index(idx));
        (end < self.cues.len()).then_some(end)
    }

    /// Whether the cue at `idx` will run: it and its group are armed.
    pub fn is_armed(&self, idx: usize) -> bool {
        self.cues[idx].armed
            && self.cues[idx]
                .parent
                .and_then(|p| self.cue(p))
                .is_none_or(|g| g.armed)
    }

    /// The first top-level cue at or after `idx` (or after the block holding
    /// it) that is armed.
    pub fn armed_top_from(&self, idx: usize) -> Option<usize> {
        let mut i = self.top_index(idx);
        loop {
            if self.cues[i].armed {
                return Some(i);
            }
            i = self.next_top(i)?;
        }
    }

    /// The next armed top-level cue after the block containing `idx`.
    pub fn next_armed_top(&self, idx: usize) -> Option<usize> {
        self.next_top(idx).and_then(|n| self.armed_top_from(n))
    }

    /// A group's children that will run, in order.
    pub fn armed_children(&self, group: CueId) -> Vec<&Cue> {
        self.children(group).iter().filter(|c| c.armed).collect()
    }

    /// A group's children, in order.
    pub fn children(&self, group: CueId) -> &[Cue] {
        match self.cue_index(group) {
            Some(i) if self.is_group(i) => &self.cues[i + 1..self.block_end(i)],
            _ => &[],
        }
    }

    /// `id` and, for a group, its children.
    pub fn with_children(&self, id: CueId) -> Vec<CueId> {
        std::iter::once(id)
            .chain(self.children(id).iter().map(|c| c.id))
            .collect()
    }

    /// Shown in the cue list (not inside a collapsed group).
    pub fn is_visible(&self, idx: usize) -> bool {
        self.cues[idx]
            .parent
            .and_then(|p| self.cue(p))
            .is_none_or(|g| !matches!(&g.kind, CueKind::Group(g) if g.collapsed))
    }

    /// The indices of the cues shown in the cue list, in order, found in one
    /// pass (`is_visible` for each cue looks its group up by id, which for a
    /// long list is a search per cue).
    pub fn visible_indices(&self) -> Vec<usize> {
        let collapsed: std::collections::HashSet<CueId> = self
            .cues
            .iter()
            .filter(|c| matches!(&c.kind, CueKind::Group(g) if g.collapsed))
            .map(|c| c.id)
            .collect();
        self.cues
            .iter()
            .enumerate()
            .filter(|(_, c)| c.parent.is_none_or(|p| !collapsed.contains(&p)))
            .map(|(i, _)| i)
            .collect()
    }

    /// The visible cue `d` rows (+1/-1) from `idx`, if any.
    pub fn step_visible(&self, idx: usize, d: i32) -> Option<usize> {
        let mut i = idx as i32;
        loop {
            i += d.signum();
            if i < 0 || i as usize >= self.cues.len() {
                return None;
            }
            if self.is_visible(i as usize) {
                return Some(i as usize);
            }
        }
    }

    /// Repair the group structure of a loaded show: children must follow
    /// their group directly, and only groups have children.
    pub fn normalize_groups(&mut self) {
        for i in 0..self.cues.len() {
            let Some(p) = self.cues[i].parent else {
                continue;
            };
            let follows_parent = i > 0
                && (self.cues[i - 1].id == p || self.cues[i - 1].parent == Some(p))
                && self
                    .cue(p)
                    .is_some_and(|g| matches!(g.kind, CueKind::Group(_)));
            if !follows_parent || matches!(self.cues[i].kind, CueKind::Group(_)) {
                self.cues[i].parent = None;
            }
        }
    }

    /// Where a new cue goes when `selected` is selected: after a child, as a
    /// sibling; into a selected group, at the end of it; otherwise after the cue.
    pub fn insert_slot(&self, selected: Option<CueId>, new_is_group: bool) -> Slot {
        let Some(i) = selected.and_then(|id| self.cue_index(id)) else {
            return Slot {
                at: self.cues.len(),
                parent: None,
            };
        };
        if let Some(p) = self.cues[i].parent {
            return Slot {
                at: if new_is_group {
                    self.block_end(self.top_index(i))
                } else {
                    i + 1
                },
                parent: if new_is_group { None } else { Some(p) },
            };
        }
        if self.is_group(i) && !new_is_group {
            return Slot {
                at: self.block_end(i),
                parent: Some(self.cues[i].id),
            };
        }
        Slot {
            at: self.block_end(i),
            parent: None,
        }
    }

    /// Insert a cue made by `make_cue` into `slot`.
    pub fn insert_cue(&mut self, slot: Slot, mut cue: Cue) {
        cue.parent = slot.parent;
        self.cues.insert(slot.at.min(self.cues.len()), cue);
    }

    /// Give the cues numbers in order: 1, 2, 3 down the list, and a group's
    /// children 3.1, 3.2 inside group 3. Returns how many numbers changed.
    pub fn renumber(&mut self) -> usize {
        let mut changed = 0;
        let (mut top, mut child) = (0u32, 0u32);
        let mut group = String::new();
        for c in &mut self.cues {
            let number = if c.parent.is_some() {
                child += 1;
                format!("{group}.{child}")
            } else {
                top += 1;
                child = 0;
                group = top.to_string();
                group.clone()
            };
            if c.number != number {
                c.number = number;
                changed += 1;
            }
        }
        changed
    }

    /// The cues that answer a search for `query` (lower case), in order, wherever
    /// they are (folded away or not), and a group that has one of its children
    /// among them.
    pub fn matching_indices(&self, query: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.cues.len() {
            if self.is_group(i) {
                let end = self.block_end(i);
                let children: Vec<usize> = (i + 1..end)
                    .filter(|&k| self.cue_matches(k, query))
                    .collect();
                if self.cue_matches(i, query) || !children.is_empty() {
                    out.push(i);
                    out.extend(children);
                }
                i = end;
            } else {
                if self.cue_matches(i, query) {
                    out.push(i);
                }
                i += 1;
            }
        }
        out
    }

    /// Whether the cue at `idx` answers a search for `query` (lower case): its
    /// number, name, notes, what kind of cue it is or its file's name.
    pub fn cue_matches(&self, idx: usize, query: &str) -> bool {
        let c = &self.cues[idx];
        let has = |text: &str| text.to_lowercase().contains(query);
        has(&c.number)
            || has(&c.name)
            || has(&c.notes)
            || has(c.kind.cue_type().label())
            || c.color.is_some_and(|k| has(k.label()))
            || c.kind
                .file_media()
                .and_then(|m| m.file.file_name())
                .is_some_and(|f| has(&f.to_string_lossy()))
    }

    /// Copy the cue at `idx` (a group is copied with its children) just after
    /// the original. Returns the copy's id.
    pub fn duplicate(&mut self, idx: usize) -> CueId {
        let (range, at) = if self.is_group(idx) {
            (idx..self.block_end(idx), self.block_end(idx))
        } else {
            (idx..idx + 1, idx + 1)
        };
        let mut copies: Vec<Cue> = self.cues[range].to_vec();
        let old_group = copies[0].id;
        let mut new_group = None;
        let first_number: i64 = self.next_cue_number().parse().unwrap_or(1);
        for (n, c) in copies.iter_mut().enumerate() {
            c.id = CueId(self.alloc_id());
            c.number = (first_number + n as i64).to_string();
            // A key or note fires one cue: the copy doesn't take it over.
            c.hotkey = None;
            c.midi_note = None;
            if n == 0 && matches!(c.kind, CueKind::Group(_)) {
                new_group = Some(c.id);
            } else if c.parent == Some(old_group)
                && let Some(g) = new_group
            {
                c.parent = Some(g);
            }
        }
        let first = copies[0].id;
        for (n, c) in copies.into_iter().enumerate() {
            self.cues.insert(at + n, c);
        }
        first
    }

    /// Remove the cue at `idx`; a group goes with its children. Returns the
    /// ids removed.
    pub fn delete(&mut self, idx: usize) -> Vec<CueId> {
        let end = if self.is_group(idx) {
            self.block_end(idx)
        } else {
            idx + 1
        };
        self.cues.drain(idx..end).map(|c| c.id).collect()
    }

    /// Time the slides of every slide show: a slide starts when the one before
    /// has faded in and been shown for its duration, and crossfades in over it.
    /// The last one is shown for its duration and fades out. Run after any
    /// change to a slide show or its slides. Returns whether anything changed.
    pub fn relay_slides(&mut self) -> bool {
        let mut changed = false;
        let groups: Vec<(usize, SlideShow)> = self
            .cues
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match &c.kind {
                CueKind::Group(g) => g.slides.map(|s| (i, s)),
                _ => None,
            })
            .collect();
        for (g, defaults) in groups {
            let end = self.block_end(g);
            let slides: Vec<usize> = (g + 1..end)
                .filter(|&i| self.cues[i].armed && matches!(self.cues[i].kind, CueKind::Image(_)))
                .collect();
            let mut t = 0.0;
            for (n, &i) in slides.iter().enumerate() {
                let last = n + 1 == slides.len();
                let cue = &mut self.cues[i];
                let CueKind::Image(m) = &mut cue.kind else {
                    continue;
                };
                let fade = m.slide_fade.unwrap_or(defaults.fade).max(0.0);
                let hold = m.slide_duration.unwrap_or(defaults.duration).max(0.0);
                let finish = last.then_some(fade + hold + fade);
                if cue.pre_wait != t
                    || m.transition != Transition::Crossfade
                    || m.fade_in != fade
                    || m.fade_out != fade
                    || m.end != finish
                    || m.loop_media
                {
                    cue.pre_wait = t;
                    m.transition = Transition::Crossfade;
                    m.fade_in = fade;
                    m.fade_out = fade;
                    m.end = finish;
                    m.loop_media = false;
                    changed = true;
                }
                t += fade + hold;
            }
        }
        changed
    }

    /// Add the images `files` to the end of the slide show `group`, in order.
    pub fn add_slides(&mut self, group: CueId, files: Vec<PathBuf>) {
        let Some(g) = self.cue_index(group) else {
            return;
        };
        let output = self
            .children(group)
            .iter()
            .find_map(|c| c.kind.media().and_then(|m| m.output))
            .or_else(|| self.outputs.first().map(|o| o.id));
        let number = self.cues[g].number.clone();
        let mut n = self.children(group).len();
        for file in files {
            n += 1;
            let g = self.cue_index(group).unwrap_or(g);
            let at = self.block_end(g);
            let name = file
                .file_stem()
                .map_or_else(|| "Slide".into(), |s| s.to_string_lossy().into_owned());
            let id = CueId(self.alloc_id());
            self.cues.insert(
                at,
                Cue {
                    id,
                    number: format!("{number}.{n}"),
                    name,
                    notes: String::new(),
                    pre_wait: 0.0,
                    continue_mode: ContinueMode::default(),
                    armed: true,
                    parent: Some(group),
                    hotkey: None,
                    midi_note: None,
                    cart: false,
                    timecode: None,
                    clock: None,
                    color: None,
                    kind: CueKind::Image(MediaCue::new_visual(file, output)),
                },
            );
        }
        self.relay_slides();
    }

    /// Total seconds a slide show runs, with the slides as timed now.
    pub fn slides_length(&self, group: CueId) -> f64 {
        let Some(CueKind::Group(g)) = self.cue(group).map(|c| &c.kind) else {
            return 0.0;
        };
        let Some(d) = g.slides else { return 0.0 };
        let slides: Vec<&MediaCue> = self
            .children(group)
            .iter()
            .filter(|c| c.armed)
            .filter_map(|c| match &c.kind {
                CueKind::Image(m) => Some(m),
                _ => None,
            })
            .collect();
        let each = |m: &MediaCue| {
            (
                m.slide_fade.unwrap_or(d.fade).max(0.0),
                m.slide_duration.unwrap_or(d.duration).max(0.0),
            )
        };
        let mut t: f64 = slides.iter().map(|m| each(m).0 + each(m).1).sum();
        if let Some(l) = slides.last() {
            t += each(l).0;
        }
        t
    }

    /// Wrap the (top-level, non-group) cue at `idx` in a new group that takes
    /// over its pre-wait and continue mode, so timing is unchanged.
    pub fn group_cue(&mut self, idx: usize) -> Option<CueId> {
        if self.is_group(idx) || self.cues[idx].parent.is_some() {
            return None;
        }
        let mut group = self.make_cue(CueType::Group, None, idx);
        group.name = format!("Group of {}", self.cues[idx].name);
        group.pre_wait = std::mem::take(&mut self.cues[idx].pre_wait);
        group.continue_mode = std::mem::take(&mut self.cues[idx].continue_mode);
        group.armed = std::mem::replace(&mut self.cues[idx].armed, true);
        let gid = group.id;
        self.cues[idx].parent = Some(gid);
        self.cues.insert(idx, group);
        Some(gid)
    }

    /// Dissolve the group at `idx`; its children become top-level cues.
    pub fn ungroup(&mut self, idx: usize) {
        if !self.is_group(idx) {
            return;
        }
        let end = self.block_end(idx);
        for c in &mut self.cues[idx + 1..end] {
            c.parent = None;
        }
        self.cues.remove(idx);
    }

    /// Move the top-level, non-group cue at `idx` into the end of `group`.
    pub fn move_into(&mut self, idx: usize, group: CueId) -> bool {
        let Some(g) = self.cue_index(group) else {
            return false;
        };
        if !self.is_group(g) || self.is_group(idx) || self.cues[idx].parent.is_some() {
            return false;
        }
        let mut cue = self.cues.remove(idx);
        cue.parent = Some(group);
        let g = self.cue_index(group).unwrap_or(g);
        let at = self.block_end(g);
        self.cues.insert(at, cue);
        true
    }

    /// Move the child at `idx` out to just after its group.
    pub fn move_out(&mut self, idx: usize) -> bool {
        let Some(p) = self.cues[idx].parent else {
            return false;
        };
        let mut cue = self.cues.remove(idx);
        cue.parent = None;
        let at = self.cue_index(p).map_or(idx, |g| self.block_end(g));
        self.cues.insert(at, cue);
        true
    }

    /// Move the cue at `idx` one place up (-1) or down (+1) the list. A group
    /// moves with its children; a top-level cue steps over a whole group; a
    /// child swaps with its siblings and leaves the group from either end.
    /// Returns the cue's new index.
    pub fn move_cue(&mut self, idx: usize, d: i32) -> Option<usize> {
        let up = d < 0;
        if let Some(p) = self.cues[idx].parent {
            let first = idx > 0 && self.cues[idx - 1].id == p;
            let last = self.cues.get(idx + 1).is_none_or(|c| c.parent != Some(p));
            return match (up, first, last) {
                (true, true, _) => {
                    let g = self.cue_index(p)?;
                    let mut cue = self.cues.remove(idx);
                    cue.parent = None;
                    self.cues.insert(g, cue);
                    Some(g)
                }
                (false, _, true) => {
                    self.cues[idx].parent = None;
                    Some(idx)
                }
                (true, false, _) => {
                    self.cues.swap(idx, idx - 1);
                    Some(idx - 1)
                }
                (false, _, false) => {
                    self.cues.swap(idx, idx + 1);
                    Some(idx + 1)
                }
            };
        }
        let end = self.block_end(idx);
        if up {
            if idx == 0 {
                return None;
            }
            let prev = self.top_index(idx - 1);
            let block: Vec<Cue> = self.cues.drain(idx..end).collect();
            for (n, c) in block.into_iter().enumerate() {
                self.cues.insert(prev + n, c);
            }
            Some(prev)
        } else {
            if end >= self.cues.len() {
                return None;
            }
            let next_end = self.block_end(end);
            let len = end - idx;
            let block: Vec<Cue> = self.cues.drain(idx..end).collect();
            let at = next_end - len;
            for (n, c) in block.into_iter().enumerate() {
                self.cues.insert(at + n, c);
            }
            Some(at)
        }
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

pub fn guess_cue_type(path: &Path) -> Option<CueType> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "mpg" | "mpeg" | "mxf" | "wmv" | "flv"
        | "ts" | "m2ts" | "gif" => Some(CueType::Video),
        "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp" | "tga" => Some(CueType::Image),
        "wav" | "mp3" | "aif" | "aiff" | "flac" | "ogg" | "m4a" | "aac" | "opus" | "wma" => {
            Some(CueType::Audio)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_relative_paths() {
        let dir = std::env::temp_dir().join("newq_test_roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let mut show = Show::default();
        let cue = show.make_cue(CueType::Video, Some(dir.join("media").join("a.mp4")), 0);
        show.cues.push(cue);
        let path = dir.join("show.newq");
        show.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let raw: Show = serde_json::from_str(&text).unwrap();
        assert!(raw.cues[0].kind.media().unwrap().file.is_relative());
        let loaded = Show::load(&path).unwrap();
        assert_eq!(
            loaded.cues[0].kind.media().unwrap().file,
            dir.join("media").join("a.mp4")
        );
    }

    #[test]
    fn legacy_crossfade_flag_migrates() {
        let dir = std::env::temp_dir().join("newq_test_legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.newq");
        std::fs::write(
            &path,
            r#"{"version":1,"name":"x","outputs":[],"next_id":5,"cues":[
              {"id":1,"number":"1","name":"a","kind":{"Video":{"file":"a.mp4","output":null,
               "fade_in":2.0,"crossfade":true}}},
              {"id":2,"number":"2","name":"b","kind":{"Image":{"file":"b.png","output":null}}}]}"#,
        )
        .unwrap();
        let show = Show::load(&path).unwrap();
        assert_eq!(
            show.cues[0].kind.media().unwrap().transition,
            Transition::Crossfade
        );
        assert_eq!(
            show.cues[1].kind.media().unwrap().transition,
            Transition::FadeIn
        );
        let saved = serde_json::to_string(&show).unwrap();
        assert!(!saved.contains("crossfade"));
    }

    #[test]
    fn a_blended_pair_is_set_up_and_round_trips_and_plain_shows_are_unchanged() {
        let mut show = Show::default();
        let plain = serde_json::to_string(&show).unwrap();
        assert!(
            !plain.contains("part_of") && !plain.contains("blend") && !plain.contains("picture"),
            "{plain}"
        );
        let (wide, left, right) = show.add_blended_pair(1920, 1080, 0.15);
        let o = |id| show.output(id).unwrap().clone();
        assert_eq!(o(wide).picture, Some((3552, 1080)));
        assert_eq!(o(wide).name, "Wide picture");
        let (l, r) = (o(left), o(right));
        let (lp, rp) = (show.part_of(left).unwrap(), show.part_of(right).unwrap());
        assert_eq!((lp.display, rp.display), (wide, wide));
        // Each shows 1920 of the 3552 pixels, and they share 288 of them.
        let share = 1920.0 / 3552.0;
        assert!((lp.area[2] - share).abs() < 1e-6 && (rp.area[2] - share).abs() < 1e-6);
        assert!((lp.area[0] + lp.area[2] - (rp.area[0] + 288.0 / 3552.0)).abs() < 1e-5);
        assert_eq!((l.blend.right, l.blend.left), (0.15, 0.0));
        assert_eq!((r.blend.left, r.blend.right), (0.15, 0.0));
        assert!(show.is_shared(wide) && !show.is_shared(left));

        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(back.part_of(left), Some(lp));
        assert_eq!(back.output(right).unwrap().blend, r.blend);
        assert_eq!(back.output(wide).unwrap().picture, Some((3552, 1080)));

        // Taking the picture's display away leaves the projectors on their own pictures.
        show.remove_output(wide);
        assert!(show.output(left).unwrap().part_of.is_none());
        assert!(!show.is_shared(wide));
    }

    #[test]
    fn nonsense_sharing_and_blends_in_a_file_are_put_right() {
        let dir = std::env::temp_dir().join("newq_test_blend");
        std::fs::create_dir_all(&dir).unwrap();
        let mut show = Show::default();
        let (wide, left, right) = show.add_blended_pair(1920, 1080, 0.1);
        let sub = show.add_sub_display(left);
        // A display showing part of itself, of a display that shows part of
        // another, and of a sub display; and a zero-sized part.
        show.outputs
            .iter_mut()
            .find(|o| o.id == wide)
            .unwrap()
            .part_of = Some(PartOf {
            display: wide,
            area: [0.0, 0.0, 1.0, 1.0],
        });
        let sub_part = PartOf {
            display: sub,
            area: [0.0, 0.0, 0.5, 1.0],
        };
        show.outputs
            .iter_mut()
            .find(|o| o.id == left)
            .unwrap()
            .part_of = Some(sub_part);
        let right_o = show.outputs.iter_mut().find(|o| o.id == right).unwrap();
        right_o.part_of = Some(PartOf {
            display: wide,
            area: [0.2, 0.0, 0.0, 1.0],
        });
        right_o.blend = EdgeBlend {
            left: 9.0,
            right: 7.0,
            top: -1.0,
            bottom: 0.2,
            gamma: 99.0,
        };
        right_o.picture = Some((0, 100_000));
        let path = dir.join("bad.newq");
        show.save(&path).unwrap();
        let back = Show::load(&path).unwrap();
        let o = |id| back.output(id).unwrap();
        // The wide display now shows part of itself (nonsense): its own picture.
        assert!(o(wide).part_of.is_none());
        assert!(o(left).part_of.is_none());
        // Right showed part of wide, which was part of itself at the time of
        // the check, so that's gone too; the numbers are in range regardless.
        let b = o(right).blend;
        assert_eq!(
            (b.left, b.right, b.top, b.bottom, b.gamma),
            (0.5, 0.5, 0.0, 0.2, 4.0)
        );
        assert_eq!(o(right).picture, Some((1, 16_384)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_test_pattern_choice_round_trips_and_the_default_is_not_written() {
        let mut show = Show::default();
        let plain = serde_json::to_string(&show).unwrap();
        assert!(!plain.contains("\"pattern\""), "{plain}");
        // A show from before there was a choice loads with the bars.
        let back: Show = serde_json::from_str(&plain).unwrap();
        assert_eq!(back.outputs[0].pattern, PatternKind::Bars);
        // Every pattern survives a save and load, and is written under its name.
        for kind in PatternKind::ALL {
            show.outputs[0].pattern = kind;
            show.outputs[0].test_pattern = true;
            let json = serde_json::to_string(&show).unwrap();
            let back: Show = serde_json::from_str(&json).unwrap();
            assert_eq!(back.outputs[0].pattern, kind);
            assert!(back.outputs[0].test_pattern);
            assert_eq!(json.contains("\"pattern\""), kind != PatternKind::Bars);
        }
    }

    #[test]
    fn a_stream_round_trips_plain_shows_have_none_and_numbers_are_kept_in_range() {
        let mut show = Show::default();
        let plain = serde_json::to_string(&show).unwrap();
        assert!(!plain.contains("stream"), "{plain}");
        show.outputs[0].stream = Some(StreamOut {
            enabled: true,
            kind: StreamKind::Udp {
                host: "10.0.0.2".into(),
                port: 5004,
            },
            fps: 25,
            height: 540,
        });
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(back.outputs[0].stream, show.outputs[0].stream);

        let dir = std::env::temp_dir().join("newq_test_stream_show");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wild.newq");
        show.outputs[0].stream = Some(StreamOut {
            enabled: true,
            kind: StreamKind::Custom {
                args: "-f null -".into(),
            },
            fps: 9000,
            height: 1,
        });
        show.save(&path).unwrap();
        let s = Show::load(&path).unwrap().outputs[0]
            .stream
            .clone()
            .unwrap();
        assert_eq!((s.fps, s.height), (60, 16));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_mesh_round_trips_old_shows_have_none_and_a_bad_one_is_dropped() {
        let mut show = Show::default();
        let plain = serde_json::to_string(&show).unwrap();
        assert!(!plain.contains("mesh"), "{plain}");
        let mut m = crate::mesh::Mesh::grid(4, 3);
        *m.point_mut(1, 1) = [0.3, 0.4];
        show.outputs[0].mesh = Some(m.clone());
        let json = serde_json::to_string(&show).unwrap();
        let back: Show = serde_json::from_str(&json).unwrap();
        assert_eq!(back.outputs[0].mesh, Some(m));

        let dir = std::env::temp_dir().join("newq_test_mesh");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.newq");
        show.outputs[0].mesh.as_mut().unwrap().points.pop();
        show.save(&path).unwrap();
        assert!(Show::load(&path).unwrap().outputs[0].mesh.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keyframes_out_of_order_or_off_the_scale_are_put_right_on_load() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, None, 0);
        let mut blur = Effect::new(EffectKind::Blur);
        blur.p[0].keys = vec![
            Keyframe {
                t: 2.0,
                v: 40.0,
                curve: Curve::Linear,
            },
            Keyframe {
                t: 0.0,
                v: 0.0,
                curve: Curve::Linear,
            },
        ];
        // Out of order, they still read without a panic...
        assert!(blur.values(1.0)[0].is_finite());
        c.kind.media_mut().unwrap().effects.push(blur);
        show.cues.push(c);
        let dir = std::env::temp_dir().join("newq_test_keyframes");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.newq");
        show.save(&path).unwrap();
        // ...and come back from the file in order. (JSON has no NaN or
        // infinity, and a number too big for a float fails to load, so the
        // numbers themselves can be trusted; the order can't.)
        let back = Show::load(&path).unwrap();
        let e = &back.cues[0].kind.media().unwrap().effects[0];
        let times: Vec<f64> = e.p[0].keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![0.0, 2.0]);
        assert_eq!(e.values(1.0)[0], 20.0);
        // A NaN time asks for the first key, never a panic.
        assert_eq!(e.p[0].at(f64::NAN), 0.0);
        // Nonsense from elsewhere is put right too.
        let mut a = Animated::new(f32::NAN);
        a.keys.push(Keyframe {
            t: f64::INFINITY,
            v: 1.0,
            curve: Curve::Linear,
        });
        a.sanitize(3.0);
        assert_eq!((a.value, a.keys.len()), (3.0, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corners_default_and_convexity() {
        let c = Corners::default();
        assert!(c.is_identity() && c.is_convex());
        let keystone = Corners([[0.1, 0.0], [0.9, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        assert!(keystone.is_convex() && !keystone.is_identity());
        // Swapping two corners twists the quad.
        let twisted = Corners([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]);
        assert!(!twisted.is_convex());
        // A dent.
        let concave = Corners([[0.0, 0.0], [1.0, 0.0], [0.4, 0.4], [0.0, 1.0]]);
        assert!(!concave.is_convex());
        // Old shows without corners load with the full screen.
        let o: OutputConfig =
            serde_json::from_str(r#"{"id":1,"name":"A","enabled":true}"#).unwrap();
        assert!(o.corners.is_identity());
    }

    #[test]
    fn displays_and_sub_displays_round_trip_and_stay_lean() {
        let mut show = Show::default();
        let json = serde_json::to_string(&show).unwrap();
        for absent in [
            "monitor",
            "within",
            "area",
            "twins",
            "placement",
            "sub_displays",
        ] {
            assert!(!json.contains(absent), "{absent} in {json}");
        }
        assert_eq!(
            (show.version, show.outputs[0].name.as_str()),
            (2, "Display 1")
        );
        let d = show.outputs[0].id;
        show.outputs[0].monitor = Some(DisplayRef {
            name: "Samsung".into(),
            index: 1,
            hardware: None,
        });
        let sub = show.add_sub_display(d);
        let sub2 = show.add_sub_display(d);
        assert_eq!(
            show.output(sub).unwrap().area.as_rect(),
            Some([0.0, 0.0, 0.5, 1.0])
        );
        assert_eq!(
            show.output(sub2).unwrap().area.as_rect(),
            Some([0.5, 0.0, 0.5, 1.0])
        );
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(back.outputs.len(), 3);
        assert_eq!(back.outputs[0].monitor, show.outputs[0].monitor);
        assert_eq!(back.outputs[1].within, Some(d));
        assert_eq!(back.outputs[2].area, show.outputs[2].area);
        // A saved version 2 show is never treated as an older one.
        let mut back = back;
        back.migrate_legacy();
        assert!(!back.needs_automatic_assignment());
        let text = serde_json::to_string(&show).unwrap();
        assert!(
            !text.contains("twins"),
            "one monitor is the default: {text}"
        );
    }

    #[test]
    fn removing_a_display_removes_its_sub_displays_and_retargets_cues() {
        let mut show = Show::default();
        let first = show.outputs[0].id;
        let second = show.add_display();
        let sub = show.add_sub_display(second);
        let mut cue = show.make_cue(CueType::Image, Some("a.png".into()), 0);
        cue.kind.media_mut().unwrap().output = Some(sub);
        let image = cue.id;
        show.cues.push(cue);
        let mut audio = show.make_cue(CueType::Audio, Some("a.wav".into()), 1);
        audio.kind.media_mut().unwrap().output = Some(second);
        let sound = audio.id;
        show.cues.push(audio);
        show.remove_output(second);
        assert_eq!(show.outputs.len(), 1);
        assert!(show.output(sub).is_none());
        // What was sent to it goes to the first display; sound has no display.
        assert_eq!(
            show.cue(image).unwrap().kind.media().unwrap().output,
            Some(first)
        );
        assert_eq!(
            show.cue(sound).unwrap().kind.media().unwrap().output,
            Some(second),
            "sound never had a display"
        );
    }

    fn legacy(json: &str) -> Show {
        let mut show: Show = serde_json::from_str(json).unwrap();
        show.migrate_legacy();
        show
    }

    #[test]
    fn shows_from_before_placements_keep_their_monitor_or_wait_for_one() {
        let show = legacy(
            r#"{"version":1,"name":"x","next_id":5,"cues":[],"outputs":[
              {"id":1,"name":"A","display":{"name":"DELL","index":1},"fullscreen":true,"enabled":true},
              {"id":2,"name":"B","display":null,"fullscreen":false,"enabled":true}]}"#,
        );
        assert_eq!(
            show.outputs[0].monitor,
            Some(DisplayRef {
                name: "DELL".into(),
                index: 1,
                hardware: None
            })
        );
        assert_eq!(show.outputs[1].monitor, None);
        assert!(show.needs_automatic_assignment());
        let saved = serde_json::to_string(&show).unwrap();
        assert!(!saved.contains("fullscreen") && !saved.contains("\"display\""));

        // The automatic one gets the first monitor the main window isn't on, once.
        let mut show = show;
        let keys = [key("Laptop"), key("DELL"), key("TV")];
        let notes = show.assign_automatic_displays(&keys, Some(0));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(show.outputs[1].monitor.as_ref().unwrap().name, "TV");
        assert!(!show.needs_automatic_assignment());
        assert!(
            show.assign_automatic_displays(&[key("Laptop")], Some(0))
                .is_empty()
        );
        assert_eq!(show.outputs[1].monitor.as_ref().unwrap().name, "TV");

        // With no monitor to spare it stays in the main window, and stays unassigned.
        let mut show = legacy(
            r#"{"version":1,"name":"x","next_id":5,"cues":[],"outputs":[
              {"id":1,"name":"A","enabled":true}]}"#,
        );
        let notes = show.assign_automatic_displays(&[key("Laptop")], Some(0));
        assert!(notes[0].contains("main window"), "{notes:?}");
        assert_eq!(show.outputs[0].monitor, None);
        assert!(!show.needs_automatic_assignment());
    }

    #[test]
    fn shows_with_numbered_displays_fold_into_displays_and_sub_displays() {
        // 0.6.0: outputs placed on displays and sub displays kept in lists.
        let show = legacy(
            r#"{"version":1,"name":"x","next_id":1000,"cues":[],
              "displays":[
                {"id":900,"name":"Stage TV","monitor":{"name":"SAMSUNG","index":1},"twins":2},
                {"id":901,"name":"Lobby"},
                {"id":902,"name":"Spare","monitor":{"name":"Dell","index":2}}],
              "sub_displays":[
                {"id":50,"name":"Left","display":900,"area":[[0,0],[0.5,0],[0.5,1],[0,1]]},
                {"id":51,"name":"Right","display":901,"area":[[0.5,0],[1,0],[1,1],[0.5,1]]}],
              "outputs":[
                {"id":1,"name":"Main","enabled":true,"placement":{"Slot":900}},
                {"id":2,"name":"Second","enabled":true,"placement":{"Slot":900}},
                {"id":3,"name":"Titles","enabled":true,"placement":{"SubDisplay":50}},
                {"id":4,"name":"Preview","enabled":true,"placement":"MainWindow"}]}"#,
        );
        let o = |id: u64| show.output(OutputId(id)).unwrap();
        // The first output on a display is the display, with its monitor.
        assert_eq!(o(1).monitor.as_ref().unwrap().name, "SAMSUNG");
        assert_eq!((o(1).twins, o(1).within), (2, None));
        // A second one on the same display is a sub display covering all of it.
        assert_eq!(o(2).within, Some(OutputId(1)));
        assert_eq!(o(2).area, Corners::default());
        // An output on a sub display took its place.
        assert_eq!(o(3).within, Some(OutputId(1)));
        assert_eq!(o(3).area.as_rect(), Some([0.0, 0.0, 0.5, 1.0]));
        assert_eq!((o(4).monitor.clone(), o(4).within), (None, None));
        // A sub display nothing was on, and the display it was on, are kept.
        let right = show.outputs.iter().find(|x| x.name == "Right").unwrap();
        assert_eq!(right.area.as_rect(), Some([0.5, 0.0, 0.5, 1.0]));
        let lobby = show.output(right.within.unwrap()).unwrap();
        assert_eq!(
            (lobby.name.as_str(), lobby.monitor.is_none()),
            ("Lobby", true)
        );
        // A display nothing was on keeps its monitor.
        let spare = show.outputs.iter().find(|x| x.name == "Spare").unwrap();
        assert_eq!(spare.monitor.as_ref().unwrap().name, "Dell");
        // Ids stay unique, nothing is waiting for a monitor, and it is saved as new.
        let mut ids: Vec<u64> = show.outputs.iter().map(|x| x.id.0).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), show.outputs.len());
        assert!(!show.needs_automatic_assignment() && show.version == 2);
        let text = serde_json::to_string(&show).unwrap();
        assert!(
            !text.contains("placement") && !text.contains("\"displays\""),
            "{text}"
        );
    }

    #[test]
    fn a_display_assigned_before_hardware_ids_learns_its_monitors_once() {
        let mut show = Show::default();
        show.outputs[0].monitor = Some(DisplayRef {
            name: "SAMSUNG".into(),
            index: 1,
            hardware: None,
        });
        let monitors = [
            key("Laptop"),
            MonitorKey {
                identity: "SAMSUNG".into(),
                raw: "Monitor #2".into(),
                hardware: Some("SAM0F6B:00012345".into()),
            },
        ];
        // Not there: nothing to learn from.
        assert!(show.learn_hardware_ids(&monitors[..1], Some(0)).is_empty());
        assert_eq!(show.outputs[0].monitor.as_ref().unwrap().hardware, None);
        // There: it learns, and says so once.
        let notes = show.learn_hardware_ids(&monitors, Some(0));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(
            show.outputs[0]
                .monitor
                .as_ref()
                .unwrap()
                .hardware
                .as_deref(),
            Some("SAM0F6B:00012345")
        );
        assert!(show.learn_hardware_ids(&monitors, Some(0)).is_empty());
        // The id is saved with the show, and left out when there is none.
        let text = serde_json::to_string(&show).unwrap();
        assert!(text.contains("SAM0F6B:00012345"));
        let plain = serde_json::to_string(&Show::default()).unwrap();
        assert!(!plain.contains("hardware"));
    }

    #[test]
    fn sub_displays_that_named_a_monitor_get_a_display_for_it() {
        let show = legacy(
            r#"{"version":1,"name":"x","next_id":100,"cues":[],
              "virtual_displays":[
                {"id":5,"name":"L","parent":{"name":"Projector","index":1},
                 "area":[[0,0],[0.5,0],[0.5,1],[0,1]]}],
              "outputs":[
                {"id":1,"name":"A","enabled":true,"placement":{"Display":{"name":"Projector","index":1}}},
                {"id":2,"name":"B","enabled":true,"placement":{"Virtual":5}}]}"#,
        );
        // The output on the monitor is the display; the sub display is on it.
        let b = show.output(OutputId(2)).unwrap();
        assert_eq!(b.within, Some(OutputId(1)));
        assert_eq!(b.area.as_rect(), Some([0.0, 0.0, 0.5, 1.0]));
        assert_eq!(show.outputs.len(), 2, "no second display was made");
    }

    fn key(name: &str) -> MonitorKey {
        MonitorKey {
            identity: name.into(),
            raw: name.into(),
            hardware: None,
        }
    }

    #[test]
    fn corner_shapes_measure_and_square_up() {
        let r = Corners::rect(0.25, 0.0, 0.5, 0.5);
        assert_eq!(r.as_rect(), Some([0.25, 0.0, 0.5, 0.5]));
        assert_eq!(r.size_on((1920, 1080)), (960, 540));
        assert_eq!(Corners::default().size_on((1920, 1080)), (1920, 1080));
        let quad = Corners([[0.1, 0.0], [0.9, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        assert_eq!(quad.as_rect(), None);
        assert_eq!(quad.bounds(), [0.0, 0.0, 1.0, 1.0]);
        // Top edge 0.8, bottom edge 1.0 of 1000 px: averaged.
        assert_eq!(quad.size_on((1000, 1000)).0, 900);
    }

    #[test]
    fn new_visual_cues_crossfade_by_default() {
        let mut show = Show::default();
        let v = show.make_cue(CueType::Video, Some("a.mp4".into()), 0);
        assert_eq!(v.kind.media().unwrap().transition, Transition::Crossfade);
        let a = show.make_cue(CueType::Audio, Some("a.wav".into()), 0);
        assert_eq!(a.kind.media().unwrap().transition, Transition::FadeIn);
    }

    #[test]
    fn cue_numbers_increment() {
        let mut show = Show::default();
        for _ in 0..3 {
            let c = show.make_cue(CueType::Wait, None, show.cues.len());
            show.cues.push(c);
        }
        let nums: Vec<_> = show.cues.iter().map(|c| c.number.clone()).collect();
        assert_eq!(nums, vec!["1", "2", "3"]);
    }

    #[test]
    fn fade_targets_previous_media() {
        let mut show = Show::default();
        let v = show.make_cue(CueType::Video, Some("a.mp4".into()), 0);
        let vid = v.id;
        show.cues.push(v);
        let f = show.make_cue(CueType::Fade, None, 1);
        match f.kind {
            CueKind::Fade(f) => assert_eq!(f.target, Some(vid)),
            _ => unreachable!(),
        }
    }

    /// A show of wait cues named by `names`.
    fn named(names: &str) -> Show {
        let mut show = Show::default();
        for n in names.chars() {
            let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
            c.name = n.to_string();
            show.cues.push(c);
        }
        show
    }

    /// The list as text: names, with children in brackets after their group.
    fn layout(show: &Show) -> String {
        show.cues
            .iter()
            .map(|c| {
                let n = c.name.as_str();
                match (&c.kind, c.parent) {
                    (CueKind::Group(_), _) => format!("G{n}:"),
                    (_, Some(_)) => format!("[{n}]"),
                    _ => n.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `abcd` with b and c grouped into one group.
    fn grouped() -> Show {
        let mut show = named("abcd");
        let g = show.group_cue(1).unwrap();
        let gi = show.cue_index(g).unwrap();
        show.cues[gi].name = "1".into();
        assert!(show.move_into(3, g));
        show
    }

    #[test]
    fn grouping_wraps_a_cue_and_keeps_its_timing() {
        let mut show = named("abc");
        show.cues[1].pre_wait = 2.0;
        show.cues[1].continue_mode = ContinueMode::AutoFollow;
        let g = show.group_cue(1).unwrap();
        let gi = show.cue_index(g).unwrap();
        assert_eq!(layout(&show), "a GGroup of b: [b] c");
        assert_eq!(show.cues[gi].pre_wait, 2.0);
        assert_eq!(show.cues[gi].continue_mode, ContinueMode::AutoFollow);
        assert_eq!(show.cues[gi + 1].pre_wait, 0.0);
        assert_eq!(show.cues[gi + 1].continue_mode, ContinueMode::DoNotContinue);
        // Groups don't nest, and a child can't be wrapped again.
        assert!(show.group_cue(gi).is_none());
        assert!(show.group_cue(gi + 1).is_none());
    }

    #[test]
    fn blocks_and_navigation() {
        let show = grouped();
        assert_eq!(layout(&show), "a G1: [b] [c] d");
        assert_eq!(show.block_end(1), 4);
        assert_eq!(show.block_end(0), 1);
        assert_eq!(show.top_index(3), 1);
        assert_eq!(show.top_index(4), 4);
        assert_eq!(show.top_id(show.cues[3].id), show.cues[1].id);
        assert_eq!(show.next_top(2), Some(4));
        assert_eq!(show.next_top(4), None);
        assert_eq!(show.children(show.cues[1].id).len(), 2);
        assert_eq!(show.with_children(show.cues[1].id).len(), 3);
        assert!(show.children(show.cues[0].id).is_empty());
    }

    #[test]
    fn collapsed_children_are_skipped_when_stepping() {
        let mut show = grouped();
        assert_eq!(show.step_visible(1, 1), Some(2));
        if let CueKind::Group(g) = &mut show.cues[1].kind {
            g.collapsed = true;
        }
        assert!(!show.is_visible(2));
        assert_eq!(show.step_visible(1, 1), Some(4));
        assert_eq!(show.step_visible(4, -1), Some(1));
        assert_eq!(show.step_visible(4, 1), None);
    }

    #[test]
    fn the_visible_indices_are_what_is_visible_found_in_one_pass() {
        let mut show = grouped();
        let by_asking = |show: &Show| -> Vec<usize> {
            (0..show.cues.len())
                .filter(|&i| show.is_visible(i))
                .collect()
        };
        assert_eq!(show.visible_indices(), by_asking(&show));
        assert_eq!(show.visible_indices(), [0, 1, 2, 3, 4]);
        if let CueKind::Group(g) = &mut show.cues[1].kind {
            g.collapsed = true;
        }
        assert_eq!(show.visible_indices(), by_asking(&show));
        assert_eq!(
            show.visible_indices(),
            [0, 1, 4],
            "the group's children are hidden"
        );
        // A child whose group has gone is shown, as `is_visible` says.
        show.cues[2].parent = Some(CueId(9999));
        assert_eq!(show.visible_indices(), by_asking(&show));
        assert!(Show::default().visible_indices().is_empty());
    }

    #[test]
    fn new_cues_join_the_selected_group() {
        let show = grouped();
        let ids: Vec<CueId> = show.cues.iter().map(|c| c.id).collect();
        let (a, g, b, c, d) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
        let slot = |sel, group| {
            let s = show.insert_slot(sel, group);
            (s.at, s.parent)
        };
        assert_eq!(slot(None, false), (5, None));
        assert_eq!(slot(Some(a), false), (1, None));
        assert_eq!(slot(Some(d), false), (5, None));
        // Selecting a group adds to the end of it; a new group goes after it.
        assert_eq!(slot(Some(g), false), (4, Some(g)));
        assert_eq!(slot(Some(g), true), (4, None));
        // After a child: next to it, in its group; a new group goes after the group.
        assert_eq!(slot(Some(b), false), (3, Some(g)));
        assert_eq!(slot(Some(c), true), (4, None));
    }

    #[test]
    fn moving_top_level_cues_and_groups() {
        let mut show = grouped();
        // A top-level cue steps over a whole group.
        assert_eq!(show.move_cue(4, -1), Some(1));
        assert_eq!(layout(&show), "a d G1: [b] [c]");
        assert_eq!(show.move_cue(1, 1), Some(4));
        assert_eq!(layout(&show), "a G1: [b] [c] d");
        // A group moves with its children.
        assert_eq!(show.move_cue(1, -1), Some(0));
        assert_eq!(layout(&show), "G1: [b] [c] a d");
        assert_eq!(show.move_cue(0, 1), Some(1));
        assert_eq!(layout(&show), "a G1: [b] [c] d");
        // The ends of the list stop.
        assert_eq!(show.move_cue(0, -1), None);
        assert_eq!(show.move_cue(4, 1), None);
        assert_eq!(layout(&show), "a G1: [b] [c] d");
    }

    #[test]
    fn moving_children_within_and_out_of_a_group() {
        let mut show = grouped();
        assert_eq!(show.move_cue(2, 1), Some(3));
        assert_eq!(layout(&show), "a G1: [c] [b] d");
        assert_eq!(show.move_cue(3, -1), Some(2));
        assert_eq!(layout(&show), "a G1: [b] [c] d");
        // Up from the first child leaves above the group; down from the last, below it.
        assert_eq!(show.move_cue(2, -1), Some(1));
        assert_eq!(layout(&show), "a b G1: [c] d");
        let mut show = grouped();
        assert_eq!(show.move_cue(3, 1), Some(3));
        assert_eq!(layout(&show), "a G1: [b] c d");
    }

    #[test]
    fn moving_cues_into_and_out_of_groups() {
        let mut show = grouped();
        let g = show.cues[1].id;
        assert!(show.move_out(2));
        assert_eq!(layout(&show), "a G1: [c] b d");
        assert!(!show.move_out(3), "already top-level");
        assert!(show.move_into(0, g));
        assert_eq!(layout(&show), "G1: [c] [a] b d");
        assert!(!show.move_into(0, g), "a group can't go into a group");
        let c = show.cues[1].id;
        assert!(!show.move_into(1, c), "target must be a group");
        assert!(!show.move_into(1, g), "already a child");
    }

    #[test]
    fn ungrouping_frees_the_children() {
        let mut show = grouped();
        show.ungroup(1);
        assert_eq!(layout(&show), "a b c d");
        assert!(show.cues.iter().all(|c| c.parent.is_none()));
    }

    #[test]
    fn duplicating_and_deleting_blocks() {
        let mut show = grouped();
        let first = show.duplicate(1);
        assert_eq!(layout(&show), "a G1: [b] [c] G1: [b] [c] d");
        let copy = show.cue_index(first).unwrap();
        assert_eq!(show.children(first).len(), 2);
        assert!(show.children(first).iter().all(|c| c.parent == Some(first)));
        let mut ids: Vec<_> = show.cues.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), show.cues.len(), "ids must stay unique");
        let mut nums: Vec<_> = show.cues.iter().map(|c| c.number.clone()).collect();
        nums.sort();
        nums.dedup();
        assert_eq!(nums.len(), show.cues.len(), "cue numbers must stay unique");
        assert_eq!(show.delete(copy).len(), 3);
        assert_eq!(layout(&show), "a G1: [b] [c] d");

        // A child copies next to itself, in the same group.
        show.duplicate(2);
        assert_eq!(layout(&show), "a G1: [b] [b] [c] d");
        assert_eq!(show.delete(2).len(), 1);
        assert_eq!(layout(&show), "a G1: [b] [c] d");
    }

    #[test]
    fn load_repairs_broken_groups() {
        let mut show = grouped();
        let g = show.cues[1].id;
        show.cues[0].parent = Some(g); // before its group
        show.cues[4].parent = Some(show.cues[0].id); // parent isn't a group
        show.cues[3].parent = Some(CueId(9999)); // parent doesn't exist
        show.normalize_groups();
        assert_eq!(layout(&show), "a G1: [b] c d");
        // A group inside a group is flattened.
        let mut show = grouped();
        let g = show.cues[1].id;
        let mut inner = show.make_cue(CueType::Group, None, 3);
        inner.parent = Some(g);
        show.cues.insert(3, inner);
        show.normalize_groups();
        assert!(show.cues[3].parent.is_none());
    }

    #[test]
    fn groups_roundtrip_through_the_show_file() {
        let dir = std::env::temp_dir().join("newq_test_groups");
        std::fs::create_dir_all(&dir).unwrap();
        let mut show = grouped();
        if let CueKind::Group(g) = &mut show.cues[1].kind {
            g.mode = GroupMode::Playlist;
            g.collapsed = true;
        }
        let path = dir.join("groups.newq");
        show.save(&path).unwrap();
        let loaded = Show::load(&path).unwrap();
        assert_eq!(layout(&loaded), "a G1: [b] [c] d");
        match &loaded.cues[1].kind {
            CueKind::Group(g) => assert_eq!((g.mode, g.collapsed), (GroupMode::Playlist, true)),
            _ => unreachable!(),
        }
        // Top-level cues don't carry a parent field, so old shows are unchanged.
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("\"parent\"").count(), 2);
    }

    #[test]
    fn transforms_default_to_nothing_and_stay_out_of_old_files() {
        let mut show = Show::default();
        let cue = show.make_cue(CueType::Video, Some("a.mp4".into()), 0);
        let json = serde_json::to_string(&cue).unwrap();
        assert!(
            !json.contains("transform"),
            "identity should not be written: {json}"
        );
        // A file from before transforms loads with the identity.
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert!(back.kind.media().unwrap().transform.is_identity());
        // A partial transform fills in the rest.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["kind"]["Video"]["transform"] = serde_json::json!({"rotation": 15.0});
        let t = serde_json::from_value::<Cue>(v).unwrap();
        let t = t.kind.media().unwrap().transform;
        assert_eq!((t.rotation, t.scale_x, t.scale_y), (15.0, 1.0, 1.0));
    }

    #[test]
    fn remote_settings_default_off_and_stay_out_of_old_files() {
        let show = Show::default();
        assert!(!show.remote.osc && !show.remote.midi);
        let json = serde_json::to_string(&show).unwrap();
        assert!(!json.contains("remote"), "{json}");
        // An old file has no such field and loads with everything off.
        let back: Show = serde_json::from_str(&json).unwrap();
        assert!(back.remote.is_default());
        // Settings survive a round trip, and a partial object fills in the rest.
        let mut on = Show::default();
        on.remote.osc = true;
        on.remote.midi_channel = Some(3);
        let back: Show = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(
            back.remote.osc && back.remote.midi_channel == Some(3) && back.remote.osc_port == 53000
        );
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["remote"] = serde_json::json!({"osc": true});
        let part: Show = serde_json::from_value(v).unwrap();
        assert!(part.remote.osc && !part.remote.midi && part.remote.osc_port == 53000);
    }

    #[test]
    fn triggers_default_off_and_stay_out_of_old_files() {
        let mut show = Show::default();
        let a = show.make_cue(CueType::Wait, None, 0);
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            !json.contains("hotkey") && !json.contains("midi_note") && !json.contains("cart"),
            "{json}"
        );
        // An old file (no such fields) loads with none set.
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert!(back.hotkey.is_none() && back.midi_note.is_none() && !back.cart);
        // Set ones round-trip.
        let mut b = a.clone();
        b.hotkey = Some("F4".into());
        b.midi_note = Some(36);
        b.cart = true;
        let back: Cue = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back.hotkey.as_deref(), Some("F4"));
        assert_eq!((back.midi_note, back.cart), (Some(36), true));
    }

    #[test]
    fn timecode_triggers_default_off_and_stay_out_of_old_files() {
        let mut show = Show::default();
        let mut cue = show.make_cue(CueType::Wait, None, 0);
        let json = serde_json::to_string(&cue).unwrap();
        assert!(!json.contains("timecode"), "{json}");
        assert!(
            serde_json::from_str::<Cue>(&json)
                .unwrap()
                .timecode
                .is_none()
        );
        cue.timecode = Some(Timecode::new(1, 0, 5, 12));
        let json = serde_json::to_string(&cue).unwrap();
        assert!(json.contains("\"timecode\":\"01:00:05:12\""), "{json}");
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(back.timecode, Some(Timecode::new(1, 0, 5, 12)));
        // Following MIDI Time Code is off, and absent from files, by default.
        assert!(!RemoteSettings::default().mtc);
        let part: RemoteSettings = serde_json::from_str("{\"midi\":true}").unwrap();
        assert!(part.midi && !part.mtc);
        let on = RemoteSettings {
            mtc: true,
            ..RemoteSettings::default()
        };
        assert!(!on.is_default());
        let back: RemoteSettings =
            serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(back.mtc);
    }

    #[test]
    fn send_cues_round_trip_and_old_files_are_unaffected() {
        let mut show = Show::default();
        let wait = show.make_cue(CueType::Wait, None, 0);
        show.cues.push(wait);
        let before = serde_json::to_string(&show).unwrap();
        assert!(!before.contains("Send"), "{before}");

        let mut osc = show.make_cue(CueType::Send, None, 1);
        assert_eq!(osc.kind.cue_type(), CueType::Send);
        if let CueKind::Send(s) = &mut osc.kind {
            s.target = SendTarget::Osc(OscOut {
                host: "10.0.0.9".into(),
                port: 8000,
                address: "/cue/1/start".into(),
                args: "1 hello".into(),
            });
        }
        let mut midi = show.make_cue(CueType::Send, None, 2);
        if let CueKind::Send(s) = &mut midi.kind {
            s.target = SendTarget::Midi(MidiOut {
                port: Some("Desk".into()),
                kind: MidiKind::ShowControl(MscCommand::Go),
                cue: "12".into(),
                ..MidiOut::default()
            });
        }
        show.cues.push(osc.clone());
        show.cues.push(midi.clone());
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        let (CueKind::Send(a), CueKind::Send(b)) = (&back.cues[1].kind, &back.cues[2].kind) else {
            panic!("not send cues");
        };
        assert_eq!(
            a.target,
            match &osc.kind {
                CueKind::Send(s) => s.target.clone(),
                _ => unreachable!(),
            }
        );
        assert_eq!(
            b.target,
            match &midi.kind {
                CueKind::Send(s) => s.target.clone(),
                _ => unreachable!(),
            }
        );
        // A hand-written OSC cue may leave out the arguments.
        let part: OscOut = serde_json::from_str(r#"{"host":"h","port":1,"address":"/a"}"#).unwrap();
        assert_eq!(part.args, "");
        // They have no media, so nothing is preloaded or probed for them.
        assert!(osc.kind.media().is_none() && !osc.kind.preloads());
    }

    #[test]
    fn web_remote_settings_default_off_and_stay_out_of_old_files() {
        let d = RemoteSettings::default();
        assert!(!d.web && d.web_port == 8080 && d.web_pin.is_empty());
        assert!(d.is_default());
        let part: RemoteSettings = serde_json::from_str("{\"osc\":true}").unwrap();
        assert!(part.osc && !part.web && part.web_port == 8080);
        let on = RemoteSettings {
            web: true,
            web_port: 9000,
            web_pin: "4821".into(),
            ..RemoteSettings::default()
        };
        assert!(!on.is_default());
        let back: RemoteSettings =
            serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(back.web && back.web_port == 9000 && back.web_pin == "4821");
        // A show that doesn't use it says nothing about it.
        let json = serde_json::to_string(&Show::default()).unwrap();
        assert!(!json.contains("web"), "{json}");
    }

    #[test]
    fn a_key_or_note_fires_one_cue() {
        let mut show = Show::default();
        let mut ids = Vec::new();
        for i in 0..3 {
            let c = show.make_cue(CueType::Wait, None, i);
            ids.push(c.id);
            show.cues.push(c);
        }
        show.set_hotkey(ids[0], Some("Q".into()));
        show.set_midi_note(ids[0], Some(60));
        assert_eq!(show.cue_for_hotkey("Q"), Some(ids[0]));
        assert_eq!(show.cue_for_note(60), Some(ids[0]));
        // Giving the same key or note to another cue takes it over.
        show.set_hotkey(ids[1], Some("Q".into()));
        show.set_midi_note(ids[1], Some(60));
        assert_eq!(show.cue_for_hotkey("Q"), Some(ids[1]));
        assert_eq!(show.cue_for_note(60), Some(ids[1]));
        assert!(show.cues[0].hotkey.is_none() && show.cues[0].midi_note.is_none());
        // Clearing leaves the others alone.
        show.set_hotkey(ids[1], None);
        assert_eq!(show.cue_for_hotkey("Q"), None);
        assert_eq!(show.cue_for_note(60), Some(ids[1]));
        // A copy of a cue doesn't inherit its triggers (it would fight for them).
        show.set_hotkey(ids[2], Some("W".into()));
        show.cues[2].cart = true;
        let copy = show.duplicate(2);
        let c = show.cue(copy).unwrap();
        assert!(c.hotkey.is_none() && c.midi_note.is_none() && c.cart);
        assert_eq!(show.cue_for_hotkey("W"), Some(ids[2]));
    }

    #[test]
    fn clock_and_timer_cues_round_trip_and_plain_text_is_written_as_before() {
        let mut show = Show::default();
        let text = show.make_cue(CueType::Text, None, 0);
        show.cues.push(text);
        let plain = serde_json::to_string(&show).unwrap();
        assert!(!plain.contains("source"), "{plain}");
        // A file from before live text loads as typed text.
        let back: Show = serde_json::from_str(&plain).unwrap();
        let CueKind::Text(t) = &back.cues[0].kind else {
            panic!("not text");
        };
        assert_eq!(t.source, TextSource::Static);

        let clock = show.make_cue(CueType::Clock, None, 1);
        let timer = show.make_cue(CueType::Timer, None, 2);
        assert_eq!(clock.kind.cue_type(), CueType::Text);
        show.cues.push(clock);
        show.cues.push(timer);
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        let sources: Vec<TextSource> = back
            .cues
            .iter()
            .map(|c| match &c.kind {
                CueKind::Text(t) => t.source,
                _ => panic!("not text"),
            })
            .collect();
        assert_eq!(sources[0], TextSource::Static);
        assert!(matches!(sources[1], TextSource::Clock { hour24: true, .. }));
        assert_eq!(
            sources[2],
            TextSource::Timer {
                total: 600.0,
                up: false,
                warn: 60.0
            }
        );
    }

    #[test]
    fn projector_and_network_cues_round_trip_and_old_send_cues_are_unaffected() {
        let mut show = Show::default();
        let mut osc = show.make_cue(CueType::Send, None, 0);
        osc.name = "osc".into();
        show.cues.push(osc);
        let before = serde_json::to_string(&show).unwrap();
        assert!(
            !before.contains("Projector") && !before.contains("Network"),
            "{before}"
        );

        for target in [
            SendTarget::Projector(ProjectorOut {
                host: "10.1.1.5".into(),
                password: "secret".into(),
                action: ProjectorAction::Input(32),
                ..ProjectorOut::default()
            }),
            SendTarget::Network(NetworkOut {
                host: "10.1.1.6".into(),
                port: 1024,
                tcp: false,
                payload: "\\x02PON\\x03".into(),
            }),
        ] {
            let mut cue = show.make_cue(CueType::Send, None, 1);
            cue.kind = CueKind::Send(SendCue {
                target: target.clone(),
            });
            let mut one = Show::default();
            one.cues.push(cue);
            let back: Show = serde_json::from_str(&serde_json::to_string(&one).unwrap()).unwrap();
            let CueKind::Send(s) = &back.cues[0].kind else {
                panic!("not a send cue");
            };
            assert_eq!(s.target, target);
        }
        // A projector saved with no password field loads without one.
        let json = r#"{"host":"h","port":4352,"action":"PowerOff"}"#;
        let p: ProjectorOut = serde_json::from_str(json).unwrap();
        assert_eq!(
            (p.password.as_str(), p.action),
            ("", ProjectorAction::PowerOff)
        );
        assert_eq!(input_name(31), "Digital 1");
        assert_eq!(ProjectorAction::Input(52).label(), "Input Network 2");
    }

    #[test]
    fn camera_cues_round_trip_and_old_files_are_unaffected() {
        let mut show = Show::default();
        // A show with no camera cue is written exactly as before.
        let wait = show.make_cue(CueType::Wait, None, 0);
        show.cues.push(wait);
        let before = serde_json::to_string(&show).unwrap();
        assert!(!before.contains("Camera"), "{before}");

        let mut cue = show.make_cue(CueType::Camera, None, 1);
        assert_eq!(cue.kind.cue_type(), CueType::Camera);
        if let CueKind::Camera(c) = &mut cue.kind {
            c.device = "/dev/video2".into();
            c.fps = 25;
            c.media.end = Some(10.0);
        }
        show.cues.push(cue);
        let json = serde_json::to_string(&show).unwrap();
        let back: Show = serde_json::from_str(&json).unwrap();
        let CueKind::Camera(c) = &back.cues[1].kind else {
            panic!("not a camera cue");
        };
        assert_eq!(
            (c.device.as_str(), c.width, c.height, c.fps),
            ("/dev/video2", 1280, 720, 25)
        );
        assert_eq!(c.media.end, Some(10.0));
        // It has the media settings other visual cues have, but reads no file.
        assert!(back.cues[1].kind.media().is_some() && back.cues[1].kind.file_media().is_none());
        assert!(back.cues[1].kind.preloads());
    }

    #[test]
    fn delays_default_to_none_stay_out_of_old_files_and_add_up() {
        let mut show = Show::default();
        let json = serde_json::to_string(&show).unwrap();
        assert!(
            !json.contains("sync") && !json.contains("delay_ms"),
            "{json}"
        );
        assert_eq!(
            (show.audio_delay_ms(None), show.video_delay_ms(None)),
            (0, 0)
        );
        // An old file has none of these and loads as no delay.
        let old: Show = serde_json::from_str(&json).unwrap();
        assert_eq!(old.sync, SyncSettings::default());

        let out = show.outputs[0].id;
        let first = show.add_audio_output();
        let second = show.add_audio_output();
        show.sync = SyncSettings {
            audio_ms: 40,
            video_ms: 100,
        };
        show.audio_outputs[1].delay_ms = 25;
        show.outputs[0].delay_ms = 20;
        // No output chosen means the first, the default.
        assert_eq!(show.audio_delay_ms(None), 40);
        assert_eq!(show.audio_delay_ms(Some(first)), 40);
        assert_eq!(show.audio_delay_ms(Some(second)), 65);
        assert_eq!(show.video_delay_ms(Some(out)), 120);
        assert_eq!(show.video_delay_ms(None), 100);
        show.sync.audio_ms = MAX_DELAY_MS;
        assert_eq!(show.audio_delay_ms(Some(second)), MAX_DELAY_MS);

        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(back.sync, show.sync);
        assert_eq!(back.audio_outputs, show.audio_outputs);
        assert_eq!(back.outputs[0].delay_ms, 20);
    }

    #[test]
    fn audio_outputs_default_empty_and_stay_out_of_old_files() {
        let mut show = Show::default();
        let cue = show.make_cue(CueType::Audio, Some("a.wav".into()), 0);
        let json = serde_json::to_string(&show).unwrap();
        assert!(!json.contains("audio_outputs"), "{json}");
        let cue_json = serde_json::to_string(&cue).unwrap();
        assert!(!cue_json.contains("audio_output") && !cue_json.contains("audio_channel"));
        let back: Cue = serde_json::from_str(&cue_json).unwrap();
        let m = back.kind.media().unwrap();
        assert_eq!((m.audio_output, m.audio_channel), (None, 0));
    }

    #[test]
    fn audio_outputs_round_trip_and_removal_resets_cues() {
        let dir = std::env::temp_dir().join("newq_test_audio_outputs");
        std::fs::create_dir_all(&dir).unwrap();
        let mut show = Show::default();
        let pa = show.add_audio_output();
        let monitor = show.add_audio_output();
        show.audio_outputs[1].device = Some("USB Interface".into());
        let mut cue = show.make_cue(CueType::Audio, Some("a.wav".into()), 0);
        {
            let m = cue.kind.media_mut().unwrap();
            m.audio_output = Some(monitor);
            m.audio_channel = 2;
        }
        show.cues.push(cue);
        let path = dir.join("audio.newq");
        show.save(&path).unwrap();
        let mut loaded = Show::load(&path).unwrap();
        assert_eq!(loaded.audio_outputs, show.audio_outputs);
        assert_eq!(loaded.audio_outputs[0].name, "Audio 1");
        assert_eq!(
            loaded.audio_outputs[1].device.as_deref(),
            Some("USB Interface")
        );
        let m = loaded.cues[0].kind.media().unwrap();
        assert_eq!((m.audio_output, m.audio_channel), (Some(monitor), 2));
        // Ids stay unique across everything the show numbers.
        let fresh = loaded.alloc_id();
        assert!(fresh > pa.0 && fresh > monitor.0);
        // Removing an output sends its cues back to the default.
        loaded.remove_audio_output(monitor);
        assert_eq!(loaded.audio_outputs.len(), 1);
        assert_eq!(loaded.cues[0].kind.media().unwrap().audio_output, None);
        assert!(loaded.audio_outputs.iter().any(|o| o.id == pa));
        assert!(!loaded.audio_outputs.iter().any(|o| o.id == monitor));
    }

    #[test]
    fn armed_defaults_on_and_only_a_disarmed_cue_is_written() {
        let mut show = Show::default();
        let mut cue = show.make_cue(CueType::Wait, None, 0);
        assert!(cue.armed);
        let json = serde_json::to_string(&cue).unwrap();
        assert!(!json.contains("armed"), "{json}");
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert!(back.armed, "old files have no field and load armed");
        cue.armed = false;
        let json = serde_json::to_string(&cue).unwrap();
        assert!(json.contains("\"armed\":false"), "{json}");
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert!(!back.armed);
    }

    #[test]
    fn armed_helpers_skip_disarmed_cues_and_groups() {
        // a G1:[b c] d e, with c, G1 and e disarmed in turn.
        let mut show = grouped();
        let e = {
            let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
            c.name = "e".into();
            show.cues.push(c);
            show.cues.len() - 1
        };
        assert!((0..show.cues.len()).all(|i| show.is_armed(i)));
        assert_eq!(show.next_armed_top(0), Some(1));
        // A disarmed child: the group still runs, with one child fewer.
        show.cues[3].armed = false;
        assert!(show.is_armed(1) && show.is_armed(2) && !show.is_armed(3));
        let g = show.cues[1].id;
        assert_eq!(show.armed_children(g).len(), 1);
        // A disarmed group disarms its children and is passed over.
        show.cues[1].armed = false;
        assert!(!show.is_armed(1) && !show.is_armed(2));
        assert_eq!(show.next_armed_top(0), Some(4));
        assert_eq!(show.armed_top_from(1), Some(4));
        assert_eq!(
            show.armed_top_from(2),
            Some(4),
            "a child counts as its group"
        );
        // From the end: nothing armed after.
        show.cues[e].armed = false;
        assert_eq!(show.armed_top_from(1), Some(4));
        show.cues[4].armed = false;
        assert_eq!(show.armed_top_from(1), None);
        assert_eq!(show.next_armed_top(0), None);
    }

    #[test]
    fn wrapping_a_disarmed_cue_keeps_it_out_of_the_show() {
        let mut show = named("abc");
        show.cues[1].armed = false;
        let g = show.group_cue(1).unwrap();
        let gi = show.cue_index(g).unwrap();
        assert!(!show.cues[gi].armed, "the group takes over the arm state");
        assert!(show.cues[gi + 1].armed);
        assert!(!show.is_armed(gi + 1), "so the child still doesn't run");
    }

    #[test]
    fn colour_settings_default_off_and_stay_out_of_old_files() {
        let mut show = Show::default();
        let cue = show.make_cue(CueType::Video, Some("a.mp4".into()), 0);
        let json = serde_json::to_string(&cue).unwrap();
        assert!(!json.contains("color"), "{json}");
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert!(back.kind.media().unwrap().color.is_default());

        let mut c = cue.clone();
        let m = c.kind.media_mut().unwrap();
        m.color.hue = 30.0;
        m.color.key = Some(ChromaKey {
            color: [10, 200, 30],
            tolerance: 0.3,
            softness: 0.2,
            spill: 0.9,
        });
        let json = serde_json::to_string(&c).unwrap();
        let back: Cue = serde_json::from_str(&json).unwrap();
        let got = back.kind.media().unwrap().color;
        assert_eq!(got.hue, 30.0);
        assert_eq!(got.contrast, 1.0, "the rest keep their defaults");
        assert_eq!(got.key.unwrap().color, [10, 200, 30]);
        // A hand-written file may name only what it changes.
        let part: ColorAdjust = serde_json::from_str(r#"{"saturation":0.0}"#).unwrap();
        assert_eq!((part.saturation, part.contrast, part.key), (0.0, 1.0, None));
        let key: ChromaKey = serde_json::from_str(r#"{"color":[0,0,255]}"#).unwrap();
        assert_eq!((key.color, key.tolerance), ([0, 0, 255], 0.2));
    }

    #[test]
    fn clock_triggers_default_off_and_stay_out_of_old_files() {
        let mut show = Show::default();
        let mut cue = show.make_cue(CueType::Wait, None, 0);
        let json = serde_json::to_string(&cue).unwrap();
        assert!(!json.contains("clock"), "{json}");
        assert!(serde_json::from_str::<Cue>(&json).unwrap().clock.is_none());
        cue.clock = TimeOfDay::new(18, 30, 0);
        let json = serde_json::to_string(&cue).unwrap();
        assert!(json.contains("\"clock\":\"18:30:00\""), "{json}");
        let back: Cue = serde_json::from_str(&json).unwrap();
        assert_eq!(back.clock, TimeOfDay::new(18, 30, 0));
        // Written by hand with no seconds, it still reads.
        let hand = json.replace("18:30:00", "7:05");
        assert_eq!(
            serde_json::from_str::<Cue>(&hand).unwrap().clock,
            TimeOfDay::new(7, 5, 0)
        );
    }

    #[test]
    fn saving_replaces_the_show_whole_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join("newq_save_whole");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("show.newq");
        let mut show = Show {
            name: "First".into(),
            ..Show::default()
        };
        show.save(&path).unwrap();
        show.name = "Second".into();
        show.save(&path).unwrap();
        assert_eq!(Show::load(&path).unwrap().name, "Second");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["show.newq"], "no .tmp left behind");
        // A save that can't be written fails and touches nothing.
        let nowhere = dir.join("no/such/folder/show.newq");
        assert!(show.save(&nowhere).is_err());
        assert_eq!(Show::load(&path).unwrap().name, "Second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn slide_show(show: &mut Show, n: usize) -> CueId {
        let slot = show.insert_slot(None, true);
        let mut g = show.make_cue(CueType::Group, None, slot.at);
        if let CueKind::Group(gc) = &mut g.kind {
            gc.slides = Some(SlideShow {
                fade: 2.0,
                duration: 4.0,
            });
        }
        let id = g.id;
        show.insert_cue(slot, g);
        show.add_slides(
            id,
            (0..n).map(|i| PathBuf::from(format!("s{i}.png"))).collect(),
        );
        id
    }

    fn slide_times(show: &Show, g: CueId) -> Vec<(f64, f64, Option<f64>)> {
        show.children(g)
            .iter()
            .map(|c| {
                let m = c.kind.media().unwrap();
                (c.pre_wait, m.fade_in, m.end)
            })
            .collect()
    }

    #[test]
    fn slides_start_when_the_one_before_has_faded_in_and_been_shown() {
        let mut show = Show::default();
        let g = slide_show(&mut show, 3);
        // Each takes 2 s to fade in and is shown for 4 s: 6 s apart. The last
        // is shown for its 4 s and fades out over 2 s.
        assert_eq!(
            slide_times(&show, g),
            vec![(0.0, 2.0, None), (6.0, 2.0, None), (12.0, 2.0, Some(8.0))]
        );
        assert_eq!(show.slides_length(g), 20.0);
        assert!(
            show.children(g)
                .iter()
                .all(|c| c.kind.media().unwrap().transition == Transition::Crossfade)
        );
        assert_eq!(
            show.children(g)[1].number,
            format!("{}.2", show.cue(g).unwrap().number)
        );
        // Nothing to do the second time.
        assert!(!show.relay_slides());
    }

    #[test]
    fn a_slide_can_have_its_own_fade_and_duration() {
        let mut show = Show::default();
        let g = slide_show(&mut show, 3);
        let first = show.children(g)[0].id;
        let last = show.children(g)[2].id;
        for (id, fade, dur) in [(first, Some(0.5), Some(1.0)), (last, Some(0.0), None)] {
            let i = show.cue_index(id).unwrap();
            let CueKind::Image(m) = &mut show.cues[i].kind else {
                panic!()
            };
            m.slide_fade = fade;
            m.slide_duration = dur;
        }
        assert!(show.relay_slides());
        // 0.5 + 1.0 = 1.5 for the first, then the default 2 + 4 for the second.
        assert_eq!(
            slide_times(&show, g),
            vec![(0.0, 0.5, None), (1.5, 2.0, None), (7.5, 0.0, Some(4.0))]
        );
        // The show's settings reach only the slides without their own.
        let gi = show.cue_index(g).unwrap();
        if let CueKind::Group(gc) = &mut show.cues[gi].kind {
            gc.slides = Some(SlideShow {
                fade: 1.0,
                duration: 3.0,
            });
        }
        show.relay_slides();
        assert_eq!(
            slide_times(&show, g),
            vec![(0.0, 0.5, None), (1.5, 1.0, None), (5.5, 0.0, Some(3.0))]
        );
    }

    #[test]
    fn disarmed_slides_and_other_cues_in_a_slide_show_are_not_timed() {
        let mut show = Show::default();
        let g = slide_show(&mut show, 3);
        let second = show.children(g)[1].id;
        let i = show.cue_index(second).unwrap();
        show.cues[i].armed = false;
        show.relay_slides();
        let t = slide_times(&show, g);
        assert_eq!((t[0].0, t[2].0), (0.0, 6.0));
        assert_eq!(t[2].2, Some(8.0));
    }

    #[test]
    fn slide_shows_are_backwards_compatible() {
        let mut show = Show::default();
        let plain = show.make_cue(CueType::Group, None, 0);
        let gid = plain.id;
        show.cues.push(plain);
        let json = serde_json::to_string(&show).unwrap();
        assert!(!json.contains("slides") && !json.contains("slide_fade"));
        // A show saved before there were slide shows loads as plain groups.
        let back: Show = serde_json::from_str(&json).unwrap();
        assert!(matches!(&back.cue(gid).unwrap().kind, CueKind::Group(g) if g.slides.is_none()));
        // A slide show and its slides' own times round-trip.
        let g = slide_show(&mut show, 2);
        let first = show.children(g)[0].id;
        let i = show.cue_index(first).unwrap();
        if let CueKind::Image(m) = &mut show.cues[i].kind {
            m.slide_fade = Some(0.25);
        }
        let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
        assert_eq!(
            back.children(g)[0].kind.media().unwrap().slide_fade,
            Some(0.25)
        );
        assert!(matches!(&back.cue(g).unwrap().kind, CueKind::Group(g) if g.slides.is_some()));
    }

    #[test]
    fn live_audio_cues_round_trip_and_keep_the_file_format_small() {
        let mut show = Show::default();
        let mut cue = show.make_cue(CueType::LiveAudio, None, 0);
        assert_eq!(cue.kind.cue_type(), CueType::LiveAudio);
        // It has playback settings (volume, fades, output) but reads no file.
        assert!(cue.kind.media().is_some() && cue.kind.file_media().is_none());
        assert!(!cue.kind.preloads());
        let before = serde_json::to_string(&cue).unwrap();
        assert!(
            !before.contains("input_channel") && !before.contains("device"),
            "{before}"
        );
        if let CueKind::LiveAudio(c) = &mut cue.kind {
            c.device = Some("Dante Virtual Soundcard".into());
            c.input_channel = 6;
            c.media.volume_db = -3.0;
        }
        let back: Cue = serde_json::from_str(&serde_json::to_string(&cue).unwrap()).unwrap();
        let CueKind::LiveAudio(c) = &back.kind else {
            panic!()
        };
        assert_eq!(c.device.as_deref(), Some("Dante Virtual Soundcard"));
        assert_eq!((c.input_channel, c.media.volume_db), (6, -3.0));
    }

    #[test]
    fn control_cues_round_trip_and_old_files_still_load() {
        let mut show = Show::default();
        let target = show.make_cue(CueType::Wait, None, 0);
        let tid = target.id;
        show.cues.push(target);
        let mut c = show.make_cue(CueType::Control, None, 1);
        assert_eq!(c.kind.cue_type(), CueType::Control);
        assert_eq!(c.name, "Start");
        assert!(c.kind.media().is_none() && !c.kind.preloads());
        c.kind = CueKind::Control(ControlCue {
            action: ControlAction::Devamp,
            target: Some(tid),
        });
        let back: Cue = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        let CueKind::Control(b) = &back.kind else {
            panic!()
        };
        assert_eq!((b.action, b.target), (ControlAction::Devamp, Some(tid)));
        // An action left out of a file is the default one.
        let old: ControlCue = serde_json::from_str(r#"{"target":null}"#).unwrap();
        assert_eq!(old.action, ControlAction::Start);
        for a in ControlAction::ALL {
            assert!(!a.label().is_empty() && !a.description().is_empty());
        }
        assert!(!ControlAction::Memo.needs_target());
    }

    #[test]
    fn cues_are_renumbered_in_order_with_children_under_their_group() {
        let mut show = Show::default();
        for n in ["7", "3.5", "x"] {
            let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
            c.number = n.into();
            show.cues.push(c);
        }
        let g = show.make_cue(CueType::Group, None, show.cues.len());
        show.cues.push(g);
        for _ in 0..2 {
            let slot = show.insert_slot(Some(show.cues[3].id), false);
            let c = show.make_cue(CueType::Wait, None, slot.at);
            show.insert_cue(slot, c);
        }
        let last = show.make_cue(CueType::Wait, None, show.cues.len());
        show.cues.push(last);
        let changed = show.renumber();
        let numbers: Vec<&str> = show.cues.iter().map(|c| c.number.as_str()).collect();
        assert_eq!(numbers, ["1", "2", "3", "4", "4.1", "4.2", "5"]);
        assert!(changed >= 3);
        // Nothing to change the second time.
        assert_eq!(show.renumber(), 0);
    }

    #[test]
    fn a_search_finds_cues_by_number_name_notes_kind_file_and_colour() {
        let mut show = Show::default();
        let mut a = show.make_cue(CueType::Video, Some("/m/Opening Titles.mp4".into()), 0);
        a.name = "Curtain up".into();
        a.number = "12".into();
        let mut b = show.make_cue(CueType::Wait, None, 1);
        b.notes = "Check the fog machine".into();
        b.color = Some(CueColor::Red);
        let g = show.make_cue(CueType::Group, None, 2);
        let gid = g.id;
        show.cues.extend([a, b, g]);
        let slot = show.insert_slot(Some(gid), false);
        let mut child = show.make_cue(CueType::Audio, Some("storm.wav".into()), slot.at);
        child.name = "Thunder".into();
        show.insert_cue(slot, child);
        let find = |q: &str| show.matching_indices(q);
        assert_eq!(find("curtain"), vec![0]);
        assert_eq!(find("TITLES".to_lowercase().as_str()), vec![0], "by file");
        assert_eq!(find("fog"), vec![1], "by notes");
        assert_eq!(find("red"), vec![1], "by colour");
        assert_eq!(find("12"), vec![0], "by number");
        assert_eq!(find("wait"), vec![1], "by type");
        // A group shows with the child that matched, even if it is folded away.
        assert_eq!(find("thunder"), vec![2, 3]);
        assert!(find("nothing like this").is_empty());
    }

    #[test]
    fn speed_is_kept_in_range_and_not_written_when_normal() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some("a.mp4".into()), 0);
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("rate"), "the default isn't written: {json}");
        let m = c.kind.media_mut().unwrap();
        assert_eq!(m.speed(), 1.0);
        m.rate = 100.0;
        assert_eq!(m.speed(), MAX_RATE);
        m.rate = 0.0;
        assert_eq!(m.speed(), MIN_RATE);
        m.rate = f64::NAN;
        assert_eq!(m.speed(), 1.0);
        m.rate = 1.5;
        let back: Cue = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.kind.media().unwrap().rate, 1.5);
        let mut other = back.kind.media().unwrap().clone();
        assert!(other.decodes_like(back.kind.media().unwrap()));
        other.rate = 1.0;
        assert!(!other.decodes_like(back.kind.media().unwrap()));
        // Cues saved before speed existed load at normal speed.
        let old =
            serde_json::to_string(&show.make_cue(CueType::Video, Some("a.mp4".into()), 0)).unwrap();
        let old: Cue = serde_json::from_str(&old).unwrap();
        assert_eq!(old.kind.media().unwrap().rate, 1.0);
    }

    #[test]
    fn a_colour_label_round_trips_and_is_left_out_when_there_is_none() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Wait, None, 0);
        assert!(!serde_json::to_string(&c).unwrap().contains("color"));
        c.color = Some(CueColor::Purple);
        let back: Cue = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.color, Some(CueColor::Purple));
        for k in CueColor::ALL {
            assert!(!k.label().is_empty());
        }
    }
}
