//! Which graphics adapter to draw with, and what to do while the only one is
//! a software renderer (pure; `gpu.rs` and `app.rs::watch_gpu` use it).
//!
//! A Windows laptop whose displays were rearranged had its driver reset; at
//! that instant the only adapter offered was "Microsoft Basic Render Driver"
//! (the processor), NewQ's recovery took it and stayed on it: a picture every
//! 10 to 15 seconds for the rest of the show. So: a hardware adapter always
//! wins over a software one, a software one is only a stopgap while the card
//! is looked for again every few seconds, and it is said loudly meanwhile.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What kind of graphics adapter, in the order NewQ prefers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Discrete,
    Integrated,
    Virtual,
    Other,
    /// Drawn by the processor (Microsoft Basic Render Driver, llvmpipe):
    /// a picture every few seconds at a show's sizes.
    Software,
}

/// Names of software renderers that may not say they are one.
const SOFTWARE_NAMES: [&str; 6] = [
    "basic render driver",
    "llvmpipe",
    "lavapipe",
    "softpipe",
    "swiftshader",
    "warp",
];

impl Kind {
    pub fn of(t: wgpu::DeviceType, name: &str) -> Kind {
        let lower = name.to_ascii_lowercase();
        let named = |s: &&str| {
            // A phrase anywhere; a single word only as a word of its own.
            if s.contains(' ') {
                lower.contains(*s)
            } else {
                lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == *s)
            }
        };
        if t == wgpu::DeviceType::Cpu || SOFTWARE_NAMES.iter().any(named) {
            return Kind::Software;
        }
        match t {
            wgpu::DeviceType::DiscreteGpu => Kind::Discrete,
            wgpu::DeviceType::IntegratedGpu => Kind::Integrated,
            wgpu::DeviceType::VirtualGpu => Kind::Virtual,
            _ => Kind::Other,
        }
    }

    pub fn is_software(self) -> bool {
        self == Kind::Software
    }

    /// Its rank: lower is better. `low_power` (`WGPU_POWER_PREF=low`) puts
    /// the integrated card before the discrete one, as wgpu's own pick does.
    fn rank(self, low_power: bool) -> u8 {
        match (self, low_power) {
            (Kind::Discrete, false) | (Kind::Integrated, true) => 0,
            (Kind::Integrated, false) | (Kind::Discrete, true) => 1,
            (Kind::Virtual, _) => 2,
            (Kind::Other, _) => 3,
            (Kind::Software, _) => 4,
        }
    }
}

/// An adapter the system offers, as far as the choice goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub kind: Kind,
    /// It can draw to the main window (always true with no window).
    pub draws: bool,
}

impl Offer {
    pub fn of(info: &wgpu::AdapterInfo, draws: bool) -> Offer {
        Offer {
            name: info.name.clone(),
            kind: Kind::of(info.device_type, &info.name),
            draws,
        }
    }
}

/// Which of `offers` to use instead of wgpu's own pick (`first`; None when
/// it found none), if any: wgpu's pick stands unless it is a software
/// renderer, so a machine that works today chooses as it did. Otherwise the
/// best hardware adapter that can draw to the window (discrete, integrated,
/// virtual, other; the system's order among equals); with no pick and no
/// hardware, the first software one that can draw.
pub fn better(first: Option<&Offer>, offers: &[Offer], low_power: bool) -> Option<usize> {
    if first.is_some_and(|f| !f.kind.is_software()) {
        return None;
    }
    let best = offers
        .iter()
        .enumerate()
        .filter(|(_, o)| o.draws && !o.kind.is_software())
        .min_by_key(|(i, o)| (o.kind.rank(low_power), *i))
        .map(|(i, _)| i);
    if best.is_some() || first.is_some() {
        return best;
    }
    offers.iter().position(|o| o.draws)
}

/// The hardware adapters among `offers` (for the look while on software).
pub fn hardware(offers: &[Offer]) -> Vec<&Offer> {
    offers.iter().filter(|o| !o.kind.is_software()).collect()
}

/// How long after a lost device NewQ waits for a hardware adapter before it
/// takes a software one (a driver reset offers only the software one for a
/// moment; taking it then means a second rebuild later).
pub const SOFTWARE_GRACE: Duration = Duration::from_secs(3);

/// Whether a rebuild after a loss may settle for a software adapter, having
/// waited `waited` for hardware.
pub fn settles_for_software(waited: Duration) -> bool {
    waited >= SOFTWARE_GRACE
}

/// How often to look for a hardware adapter while drawing in software: every
/// few seconds at first (a driver reset is over in seconds), then twice a
/// minute.
pub fn look_every(on_software_for: Duration) -> Duration {
    if on_software_for < Duration::from_secs(120) {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(30)
    }
}

/// After `failed` rebuilds that hoped for hardware and still ended in
/// software (it went again, or can't draw to the window), wait this long
/// before the next; None: give up hoping (each rebuild blanks the outputs
/// for a moment).
pub fn after_failed_switches(failed: u32) -> Option<Duration> {
    match failed {
        0 => Some(Duration::ZERO),
        1 => Some(Duration::from_secs(30)),
        2 => Some(Duration::from_secs(120)),
        n if n < GIVE_UP_AFTER => Some(Duration::from_secs(480)),
        _ => None,
    }
}

/// The count of failed switches after one more. With no device lost this
/// run (NewQ started in software beside a card that can't draw to its
/// windows: a VM, a remote session, Xvfb), once is enough: give up.
pub fn failed_switch(failed: u32, lost_this_run: bool) -> u32 {
    if lost_this_run {
        failed + 1
    } else {
        GIVE_UP_AFTER
    }
}

/// Failed switches after which the card is no longer hoped for.
const GIVE_UP_AFTER: u32 = 4;

/// Whether to rebuild the graphics onto hardware now: drawing in software,
/// a look found a hardware adapter, and the wait after earlier failed
/// switches (`since_last_switch`) is over.
pub fn switch_back(
    on_software: bool,
    hardware_seen: bool,
    failed: u32,
    since_last_switch: Option<Duration>,
) -> bool {
    on_software
        && hardware_seen
        && after_failed_switches(failed)
            .is_some_and(|wait| since_last_switch.is_none_or(|s| s >= wait))
}

/// A computer drawing in software, as it tells the others (in its
/// `StatusReport`) and as the reports say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoftwareDrawing {
    /// The adapter it draws with ("Microsoft Basic Render Driver").
    pub adapter: String,
    /// It had a graphics card and lost it (else it never had one NewQ could use).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub lost: bool,
}

impl SoftwareDrawing {
    /// The warning, naming the computer (`who`: "WNO-2", "This computer").
    pub fn sentence(&self, who: &str) -> String {
        if self.lost {
            format!(
                "{who} lost its graphics card and is drawing in software ({}): pictures will be very slow. NewQ keeps looking for the card and goes back to it when it returns; restarting NewQ (or the computer) may bring it back sooner",
                self.adapter
            )
        } else {
            format!(
                "{who} has no graphics card NewQ can use and is drawing in software ({}): pictures will be very slow",
                self.adapter
            )
        }
    }

    /// The same in the past tense, for a test's report.
    pub fn report_sentence(&self, who: &str) -> String {
        if self.lost {
            format!(
                "{who} lost its graphics card and drew in software ({}): its pictures were very slow from then on, whatever the timing says",
                self.adapter
            )
        } else {
            format!(
                "{who} had no graphics card NewQ could use and drew in software ({}): its pictures were very slow, whatever the timing says",
                self.adapter
            )
        }
    }

    /// Short, for the status bar.
    pub fn short(&self) -> &'static str {
        if self.lost {
            "⚠ graphics card lost: drawing in software, pictures very slow"
        } else {
            "⚠ no graphics card: drawing in software, pictures very slow"
        }
    }
}

/// The status bar's warning while this computer draws in software.
pub fn status(ui: &mut egui::Ui) {
    let Some(s) = crate::gpu::drawing_in_software() else {
        return;
    };
    ui.label(egui::RichText::new(s.short()).color(egui::Color32::from_rgb(230, 70, 60)))
        .on_hover_text(s.sentence("This computer"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(name: &str, kind: Kind, draws: bool) -> Offer {
        Offer {
            name: name.into(),
            kind,
            draws,
        }
    }

    #[test]
    fn software_renderers_are_known_by_type_or_name() {
        use wgpu::DeviceType as T;
        assert_eq!(
            Kind::of(T::Cpu, "Microsoft Basic Render Driver"),
            Kind::Software
        );
        assert_eq!(
            Kind::of(T::Other, "Microsoft Basic Render Driver"),
            Kind::Software
        );
        assert_eq!(
            Kind::of(T::Other, "llvmpipe (LLVM 15.0.7, 256 bits)"),
            Kind::Software
        );
        assert_eq!(
            Kind::of(T::VirtualGpu, "SwiftShader Device"),
            Kind::Software
        );
        assert_eq!(
            Kind::of(T::DiscreteGpu, "NVIDIA GeForce RTX 5060 Laptop GPU"),
            Kind::Discrete
        );
        assert_eq!(Kind::of(T::IntegratedGpu, "Apple M2"), Kind::Integrated);
        assert_eq!(
            Kind::of(T::IntegratedGpu, "Intel(R) UHD Graphics"),
            Kind::Integrated
        );
        // "warp" only as a word of its own.
        assert_eq!(Kind::of(T::DiscreteGpu, "Warpspeed 9000"), Kind::Discrete);
    }

    #[test]
    fn wgpus_hardware_pick_stands() {
        let first = offer("NVIDIA", Kind::Integrated, true);
        let offers = [
            offer("NVIDIA", Kind::Discrete, true),
            offer("Microsoft Basic Render Driver", Kind::Software, true),
        ];
        assert_eq!(better(Some(&first), &offers, false), None);
    }

    #[test]
    fn a_software_pick_gives_way_to_hardware_discrete_first() {
        let first = offer("Microsoft Basic Render Driver", Kind::Software, true);
        let offers = [
            offer("Microsoft Basic Render Driver", Kind::Software, true),
            offer("Intel(R) UHD Graphics", Kind::Integrated, true),
            offer("NVIDIA GeForce RTX 5060", Kind::Discrete, true),
        ];
        assert_eq!(better(Some(&first), &offers, false), Some(2));
        // Low power asked for: the integrated one.
        assert_eq!(better(Some(&first), &offers, true), Some(1));
    }

    #[test]
    fn hardware_that_cannot_draw_to_the_window_is_not_taken() {
        let first = offer("llvmpipe", Kind::Software, true);
        let offers = [
            offer("NVIDIA", Kind::Discrete, false),
            offer("llvmpipe", Kind::Software, true),
        ];
        assert_eq!(better(Some(&first), &offers, false), None);
    }

    #[test]
    fn software_only_when_it_is_all_there_is() {
        let offers = [
            offer("Microsoft Basic Render Driver", Kind::Software, false),
            offer("llvmpipe", Kind::Software, true),
        ];
        // wgpu's software pick stands.
        let first = offer("llvmpipe", Kind::Software, true);
        assert_eq!(better(Some(&first), &offers, false), None);
        // No pick at all: the first software one that draws.
        assert_eq!(better(None, &offers, false), Some(1));
        // No pick, hardware there: the hardware.
        let mixed = [
            offer("llvmpipe", Kind::Software, true),
            offer("Radeon", Kind::Other, true),
        ];
        assert_eq!(better(None, &mixed, false), Some(1));
        assert_eq!(better(None, &[], false), None);
        assert_eq!(hardware(&mixed).len(), 1);
    }

    #[test]
    fn equal_kinds_keep_the_systems_order() {
        let offers = [
            offer("A", Kind::Integrated, true),
            offer("B", Kind::Integrated, true),
        ];
        assert_eq!(better(None, &offers, false), Some(0));
    }

    #[test]
    fn a_rebuild_waits_a_moment_before_settling_for_software() {
        assert!(!settles_for_software(Duration::ZERO));
        assert!(!settles_for_software(Duration::from_secs(2)));
        assert!(settles_for_software(SOFTWARE_GRACE));
    }

    #[test]
    fn the_card_is_looked_for_often_at_first_then_less() {
        assert_eq!(look_every(Duration::from_secs(10)), Duration::from_secs(5));
        assert_eq!(
            look_every(Duration::from_secs(600)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn hardware_seen_while_in_software_means_a_rebuild() {
        assert!(switch_back(true, true, 0, None));
        assert!(!switch_back(true, false, 0, None), "nothing to go to");
        assert!(!switch_back(false, true, 0, None), "already on hardware");
        // After a switch that ended in software again, wait longer each time.
        assert!(!switch_back(true, true, 1, Some(Duration::from_secs(10))));
        assert!(switch_back(true, true, 1, Some(Duration::from_secs(30))));
        assert!(!switch_back(true, true, 2, Some(Duration::from_secs(60))));
        assert!(switch_back(true, true, 3, Some(Duration::from_secs(480))));
        // A switch that failed with no loss this run (started in software
        // beside a card that can't draw): give up at once.
        assert_eq!(failed_switch(0, true), 1);
        let gave_up = failed_switch(0, false);
        assert!(!switch_back(
            true,
            true,
            gave_up,
            Some(Duration::from_secs(100_000))
        ));
        // And give up after four.
        assert!(!switch_back(
            true,
            true,
            4,
            Some(Duration::from_secs(100_000))
        ));
    }

    #[test]
    fn the_warning_says_what_happened() {
        let lost = SoftwareDrawing {
            adapter: "Microsoft Basic Render Driver".into(),
            lost: true,
        };
        let s = lost.sentence("WNO-2");
        assert!(
            s.starts_with(
                "WNO-2 lost its graphics card and is drawing in software (Microsoft Basic Render Driver): pictures will be very slow"
            ),
            "{s}"
        );
        let never = SoftwareDrawing {
            adapter: "llvmpipe".into(),
            lost: false,
        };
        assert_eq!(
            never.sentence("This computer"),
            "This computer has no graphics card NewQ can use and is drawing in software (llvmpipe): pictures will be very slow"
        );
        assert!(lost.short().contains("lost"));
    }

    #[test]
    fn the_status_field_is_small_and_defaults() {
        let never = SoftwareDrawing {
            adapter: "llvmpipe".into(),
            lost: false,
        };
        assert_eq!(
            serde_json::to_string(&never).unwrap(),
            r#"{"adapter":"llvmpipe"}"#
        );
        let back: SoftwareDrawing = serde_json::from_str("{}").unwrap();
        assert_eq!(back, SoftwareDrawing::default());
    }
}
