//! Show file data model: cues, outputs, and (de)serialization.

use crate::clock::TimeOfDay;
use crate::media::playback::Playback;
use crate::timecode::Timecode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SHOW_EXTENSION: &str = "newq";
/// Volumes at or below this are treated as silence.
pub const MIN_DB: f32 = -60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct CueId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubDisplayId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioOutputId(pub u64);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Show {
    pub version: u32,
    pub name: String,
    pub outputs: Vec<OutputConfig>,
    /// Areas of physical displays that outputs can be placed on like displays.
    #[serde(default, alias = "virtual_displays")]
    pub sub_displays: Vec<SubDisplay>,
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
            version: 1,
            name: "Untitled Show".into(),
            outputs: Vec::new(),
            sub_displays: Vec::new(),
            audio_outputs: Vec::new(),
            cues: Vec::new(),
            next_id: 1,
            panic_fade: default_panic_fade(),
            remote: RemoteSettings::default(),
        };
        let id = OutputId(show.alloc_id());
        show.outputs.push(OutputConfig::new(id, "Output 1".into()));
        show
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DisplayRef {
    /// Monitor name as reported by the OS.
    pub name: String,
    /// Index in the OS monitor list, used as a fallback when names collide or change.
    pub index: usize,
}

/// Where an output appears.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub enum Placement {
    /// Fullscreen on a display other than the one NewQ's main window is on;
    /// inside the main window when there is no such display.
    #[default]
    Auto,
    /// Always inside the main window.
    MainWindow,
    /// Fullscreen on this display; inside the main window while it isn't connected.
    Display(DisplayRef),
    /// Filling this sub display; inside the main window while its display
    /// isn't connected.
    #[serde(alias = "Virtual")]
    SubDisplay(SubDisplayId),
}

/// Part of a physical display that outputs can be placed on as if it were a
/// display of its own (e.g. one projector covering two set pieces, or one
/// LED processor input feeding several screens).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SubDisplay {
    pub id: SubDisplayId,
    pub name: String,
    /// The physical display it is part of.
    pub parent: DisplayRef,
    /// Its four corners on that display. Any convex shape; a rectangle by default.
    pub area: Corners,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputConfig {
    pub id: OutputId,
    pub name: String,
    #[serde(default)]
    pub placement: Placement,
    /// Whether the output is shown at all.
    pub enabled: bool,
    #[serde(default)]
    pub test_pattern: bool,
    /// Keystone / corner pin applied to everything on this output.
    #[serde(default)]
    pub corners: Corners,
    /// Legacy (pre-placement) display choice, migrated on load.
    #[serde(default, skip_serializing)]
    display: Option<DisplayRef>,
}

impl OutputConfig {
    pub fn new(id: OutputId, name: String) -> Self {
        OutputConfig {
            id,
            name,
            placement: Placement::Auto,
            enabled: true,
            test_pattern: false,
            corners: Corners::default(),
            display: None,
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
    pub kind: CueKind,
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
        }
    }
    /// The playback settings of a cue that plays something: transitions,
    /// fades, output, layer and so on. A text cue has them too.
    pub fn media(&self) -> Option<&MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            CueKind::Text(t) => Some(&t.media),
            CueKind::Camera(c) => Some(&c.media),
            _ => None,
        }
    }
    pub fn media_mut(&mut self) -> Option<&mut MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            CueKind::Text(t) => Some(&mut t.media),
            CueKind::Camera(c) => Some(&mut c.media),
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

    /// How many passes a looping cue makes; None = forever.
    pub fn loop_passes(&self) -> Option<u32> {
        (self.loop_count > 0).then_some(self.loop_count)
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
    pub media: MediaCue,
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
            media,
        }
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
}

impl Default for SendTarget {
    fn default() -> Self {
        SendTarget::Osc(OscOut::default())
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
}

impl MscCommand {
    pub fn label(self) -> &'static str {
        match self {
            MscCommand::Go => "GO",
            MscCommand::Stop => "STOP",
            MscCommand::Resume => "RESUME",
            MscCommand::Load => "LOAD",
            MscCommand::AllOff => "ALL OFF",
        }
    }
    /// Whether the command takes a cue number.
    pub fn takes_cue(self) -> bool {
        !matches!(self, MscCommand::Resume | MscCommand::AllOff)
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
    pub const ALL: [MidiKind; 9] = [
        MidiKind::NoteOn,
        MidiKind::NoteOff,
        MidiKind::ControlChange,
        MidiKind::ProgramChange,
        MidiKind::ShowControl(MscCommand::Go),
        MidiKind::ShowControl(MscCommand::Stop),
        MidiKind::ShowControl(MscCommand::Resume),
        MidiKind::ShowControl(MscCommand::Load),
        MidiKind::ShowControl(MscCommand::AllOff),
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

    pub fn sub_display(&self, id: SubDisplayId) -> Option<&SubDisplay> {
        self.sub_displays.iter().find(|v| v.id == id)
    }

    /// Add a sub display on `parent`: the left half of it, then the right
    /// half, then a centred area.
    pub fn add_sub_display(&mut self, parent: DisplayRef) -> SubDisplayId {
        let id = SubDisplayId(self.alloc_id());
        let siblings = self
            .sub_displays
            .iter()
            .filter(|v| v.parent == parent)
            .count();
        let area = match siblings {
            0 => Corners::rect(0.0, 0.0, 0.5, 1.0),
            1 => Corners::rect(0.5, 0.0, 0.5, 1.0),
            _ => Corners::rect(0.25, 0.25, 0.5, 0.5),
        };
        let n = self.sub_displays.len() + 1;
        self.sub_displays.push(SubDisplay {
            id,
            name: format!("Sub display {n}"),
            parent,
            area,
        });
        id
    }

    /// Remove a sub display; outputs on it move to the main window.
    pub fn remove_sub_display(&mut self, id: SubDisplayId) {
        self.sub_displays.retain(|v| v.id != id);
        for o in &mut self.outputs {
            if o.placement == Placement::SubDisplay(id) {
                o.placement = Placement::MainWindow;
            }
        }
    }

    pub fn add_audio_output(&mut self) -> AudioOutputId {
        let id = AudioOutputId(self.alloc_id());
        let n = self.audio_outputs.len() + 1;
        self.audio_outputs.push(AudioOutputConfig {
            id,
            name: format!("Audio {n}"),
            device: None,
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

    pub fn add_output(&mut self) -> OutputId {
        let id = OutputId(self.alloc_id());
        let n = self.outputs.len() + 1;
        self.outputs
            .push(OutputConfig::new(id, format!("Output {n}")));
        id
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
            CueType::Camera => (
                "Camera".into(),
                CueKind::Camera(CameraCue::new(default_output)),
            ),
            CueType::Send => ("Send".into(), CueKind::Send(SendCue::default())),
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
        }
        for out in &mut show.outputs {
            if let Some(d) = out.display.take() {
                out.placement = Placement::Display(d);
            }
        }
        show.normalize_groups();
        // Guard against hand-edited files with stale counters.
        let max_id = show
            .cues
            .iter()
            .map(|c| c.id.0)
            .chain(show.outputs.iter().map(|o| o.id.0))
            .chain(show.sub_displays.iter().map(|v| v.id.0))
            .chain(show.audio_outputs.iter().map(|o| o.id.0))
            .max()
            .unwrap_or(0);
        show.next_id = show.next_id.max(max_id + 1);
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
        std::fs::write(&tmp, text)?;
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
    fn legacy_output_display_migrates_to_placement() {
        let dir = std::env::temp_dir().join("newq_test_legacy_outputs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.newq");
        std::fs::write(
            &path,
            r#"{"version":1,"name":"x","next_id":5,"cues":[],"outputs":[
              {"id":1,"name":"A","display":{"name":"DELL","index":1},"fullscreen":true,"enabled":true},
              {"id":2,"name":"B","display":null,"fullscreen":false,"enabled":true}]}"#,
        )
        .unwrap();
        let show = Show::load(&path).unwrap();
        assert_eq!(
            show.outputs[0].placement,
            Placement::Display(DisplayRef {
                name: "DELL".into(),
                index: 1
            })
        );
        assert_eq!(show.outputs[1].placement, Placement::Auto);
        let saved = serde_json::to_string(&show).unwrap();
        assert!(!saved.contains("fullscreen") && !saved.contains("\"display\""));
    }

    #[test]
    fn sub_displays_add_remove_and_load() {
        let mut show = Show::default();
        let parent = DisplayRef {
            name: "Projector".into(),
            index: 1,
        };
        let a = show.add_sub_display(parent.clone());
        let b = show.add_sub_display(parent.clone());
        assert_eq!(
            show.sub_display(a).unwrap().area.as_rect(),
            Some([0.0, 0.0, 0.5, 1.0])
        );
        assert_eq!(
            show.sub_display(b).unwrap().area.as_rect(),
            Some([0.5, 0.0, 0.5, 1.0])
        );
        let out = show.outputs[0].id;
        show.outputs[0].placement = Placement::SubDisplay(a);
        show.remove_sub_display(a);
        assert!(show.sub_display(a).is_none());
        assert_eq!(show.output(out).unwrap().placement, Placement::MainWindow);

        // Round trip, and shows from before sub displays still load.
        let text = serde_json::to_string(&show).unwrap();
        let back: Show = serde_json::from_str(&text).unwrap();
        assert_eq!(back.sub_displays, show.sub_displays);
        let old: Show = serde_json::from_str(
            r#"{"version":1,"name":"x","next_id":2,"cues":[],"outputs":[{"id":1,"name":"A","enabled":true}]}"#,
        )
        .unwrap();
        assert!(old.sub_displays.is_empty());
        // ...as do shows saved while they were called virtual displays.
        let renamed: Show = serde_json::from_str(
            r#"{"version":1,"name":"x","next_id":9,"cues":[],
                "outputs":[{"id":1,"name":"A","enabled":true,"placement":{"Virtual":5}}],
                "virtual_displays":[{"id":5,"name":"L","parent":{"name":"P","index":0},
                    "area":[[0,0],[0.5,0],[0.5,1],[0,1]]}]}"#,
        )
        .unwrap();
        assert_eq!(renamed.sub_displays[0].id, SubDisplayId(5));
        assert_eq!(
            renamed.outputs[0].placement,
            Placement::SubDisplay(SubDisplayId(5))
        );
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
}
