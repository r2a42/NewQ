//! Show file data model: cues, outputs, and (de)serialization.

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Show {
    pub version: u32,
    pub name: String,
    pub outputs: Vec<OutputConfig>,
    /// Areas of physical displays that outputs can be placed on like displays.
    #[serde(default, alias = "virtual_displays")]
    pub sub_displays: Vec<SubDisplay>,
    pub cues: Vec<Cue>,
    pub next_id: u64,
    /// Fade time used by the Panic / Stop All button.
    #[serde(default = "default_panic_fade")]
    pub panic_fade: f64,
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
            cues: Vec::new(),
            next_id: 1,
            panic_fade: default_panic_fade(),
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueType {
    Video,
    Image,
    Audio,
    Fade,
    Stop,
    Wait,
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
        }
    }
    pub fn media(&self) -> Option<&MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            _ => None,
        }
    }
    pub fn media_mut(&mut self) -> Option<&mut MediaCue> {
        match self {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => Some(m),
            _ => None,
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
    #[serde(default)]
    pub volume_db: f32,
    /// In point, seconds.
    #[serde(default)]
    pub start: f64,
    /// Out point, seconds. For images: how long to hold (None = until stopped).
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub loop_media: bool,
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
            volume_db: 0.0,
            start: 0.0,
            end: None,
            loop_media: false,
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

impl Show {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn cue(&self, id: CueId) -> Option<&Cue> {
        self.cues.iter().find(|c| c.id == id)
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
        };
        Cue {
            id,
            number,
            name,
            notes: String::new(),
            pre_wait: 0.0,
            continue_mode: ContinueMode::DoNotContinue,
            kind,
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
        // Guard against hand-edited files with stale counters.
        let max_id = show
            .cues
            .iter()
            .map(|c| c.id.0)
            .chain(show.outputs.iter().map(|o| o.id.0))
            .chain(show.sub_displays.iter().map(|v| v.id.0))
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
        std::fs::write(path, text)?;
        Ok(())
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
}
