//! What the 3D stage did, counted as it goes (`stage_render::totals()`), for
//! Test Mode's reports (`testmode::previews`, and the statistics' `stage_ms`).
//! Relaxed atomics only: counting never waits, is never on the main thread's
//! way, and never changes what is drawn.
//!
//! `TOTALS` is the whole program's. A part of the stage counts through a
//! `CountTo`, which is `TOTALS` unless a test gave it counters of its own;
//! those count into `TOTALS` as well, so a test can check its own exactly
//! while others draw beside it.

use crate::render3d::FrameStats;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The upper bounds (ms) of the processor-time bands a 3D view is counted
/// in (`StageTotals::frames_by_ms`); one over the last is in a band of its
/// own after them.
pub const FRAME_MS_BANDS: [u64; 8] = [1, 2, 4, 8, 16, 33, 66, 133];
const BANDS: usize = FRAME_MS_BANDS.len() + 1;

pub struct Counters {
    jobs: AtomicU64,
    feeds: AtomicU64,
    throws: AtomicU64,
    views: AtomicU64,
    displays: AtomicU64,
    cpu_us_views: AtomicU64,
    cpu_us_displays: AtomicU64,
    cpu_us_worst: AtomicU64,
    frames_by_ms: [AtomicU64; BANDS],
    missing_pictures: AtomicU64,
    display_missing_pictures: AtomicU64,
    light_left_out: AtomicU64,
    shadows_drawn: AtomicU64,
    thread_starts: AtomicU64,
    thread_stops: AtomicU64,
    restarts: AtomicU64,
    idle_releases: AtomicU64,
    throws_left_out: AtomicU64,
    venue_loads: AtomicU64,
    venue_failures: AtomicU64,
    venue_load_ms: AtomicU64,
    venue_parts: AtomicU64,
    venue_triangles: AtomicU64,
    maps_baked: AtomicU64,
    maps_refused: AtomicU64,
}

/// The whole program's counts (`totals()`).
pub static TOTALS: Counters = Counters::new();

/// What the whole program's 3D stage has done since it started.
pub fn totals() -> StageTotals {
    TOTALS.totals()
}

/// Where a part of the stage counts what it did: `TOTALS` unless a test
/// gave it counters of its own.
#[derive(Clone, Copy)]
pub struct CountTo(pub &'static Counters);

impl Default for CountTo {
    fn default() -> Self {
        CountTo(&TOTALS)
    }
}

impl CountTo {
    /// `f` on these counters, and on `TOTALS` too when they aren't it.
    fn each(self, f: impl Fn(&Counters)) {
        f(self.0);
        if !std::ptr::eq(self.0, &TOTALS) {
            f(&TOTALS);
        }
    }

    fn add(self, field: fn(&Counters) -> &AtomicU64, n: u64) {
        if n > 0 {
            self.each(|c| {
                field(c).fetch_add(n, Ordering::Relaxed);
            });
        }
    }

    fn set(self, field: fn(&Counters) -> &AtomicU64, v: u64) {
        self.each(|c| field(c).store(v, Ordering::Relaxed));
    }

    /// A job taken up: `feeds` displays' pictures and `throws` projectors'
    /// pictures composited for it.
    pub fn job(self, feeds: usize, throws: usize) {
        self.add(|c| &c.jobs, 1);
        self.add(|c| &c.feeds, feeds as u64);
        self.add(|c| &c.throws, throws as u64);
    }

    /// One view drawn: `display` for a display a window or stream shows
    /// (`ViewJob::display`), else a preview's (or that of a display only the
    /// previews show).
    pub fn view(self, display: bool, s: &FrameStats) {
        let us = u64::try_from(s.cpu_time.as_micros()).unwrap_or(u64::MAX);
        let band = FRAME_MS_BANDS
            .iter()
            .position(|&ms| us <= ms * 1000)
            .unwrap_or(FRAME_MS_BANDS.len());
        let missing = u64::from(s.missing_pictures);
        self.each(|c| {
            let (n, cpu) = if display {
                c.display_missing_pictures
                    .fetch_add(missing, Ordering::Relaxed);
                (&c.displays, &c.cpu_us_displays)
            } else {
                (&c.views, &c.cpu_us_views)
            };
            n.fetch_add(1, Ordering::Relaxed);
            cpu.fetch_add(us, Ordering::Relaxed);
            c.cpu_us_worst.fetch_max(us, Ordering::Relaxed);
            c.frames_by_ms[band].fetch_add(1, Ordering::Relaxed);
            c.missing_pictures.fetch_add(missing, Ordering::Relaxed);
            c.light_left_out
                .fetch_add(u64::from(s.light_left_out), Ordering::Relaxed);
            c.shadows_drawn
                .fetch_add(u64::from(s.shadows_drawn), Ordering::Relaxed);
        });
    }

    /// The drawing thread started; `after_a_stop` when one was found
    /// stopped before it (a restart).
    pub fn thread_started(self, after_a_stop: bool) {
        self.add(|c| &c.thread_starts, 1);
        self.add(|c| &c.restarts, u64::from(after_a_stop));
    }

    /// The drawing thread was found stopped (a panic in it).
    pub fn thread_stopped(self) {
        self.add(|c| &c.thread_stops, 1);
    }

    /// The drawing thread let go with nothing to draw.
    pub fn idle_release(self) {
        self.add(|c| &c.idle_releases, 1);
    }

    /// A lens map baked (`ok`), or refused by the card.
    pub fn map_baked(self, ok: bool) {
        if ok {
            self.add(|c| &c.maps_baked, 1);
        } else {
            self.add(|c| &c.maps_refused, 1);
        }
    }

    /// The visible projectors past `MAX_THROWS` at this lit pass.
    pub fn throws_left_out(self, n: usize) {
        self.set(|c| &c.throws_left_out, n as u64);
    }

    /// A venue read in `took`: its parts and triangles if it loaded, None
    /// if it failed.
    pub fn venue_read(self, took: Duration, loaded: Option<(usize, usize)>) {
        match loaded {
            Some((parts, triangles)) => {
                self.add(|c| &c.venue_loads, 1);
                self.set(
                    |c| &c.venue_load_ms,
                    u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
                );
                self.set(|c| &c.venue_parts, parts as u64);
                self.set(|c| &c.venue_triangles, triangles as u64);
            }
            None => self.add(|c| &c.venue_failures, 1),
        }
    }
}

impl Counters {
    pub const fn new() -> Counters {
        #[allow(clippy::declare_interior_mutable_const)]
        const Z: AtomicU64 = AtomicU64::new(0);
        Counters {
            jobs: Z,
            feeds: Z,
            throws: Z,
            views: Z,
            displays: Z,
            cpu_us_views: Z,
            cpu_us_displays: Z,
            cpu_us_worst: Z,
            frames_by_ms: [Z; BANDS],
            missing_pictures: Z,
            display_missing_pictures: Z,
            light_left_out: Z,
            shadows_drawn: Z,
            thread_starts: Z,
            thread_stops: Z,
            restarts: Z,
            idle_releases: Z,
            throws_left_out: Z,
            venue_loads: Z,
            venue_failures: Z,
            venue_load_ms: Z,
            venue_parts: Z,
            venue_triangles: Z,
            maps_baked: Z,
            maps_refused: Z,
        }
    }

    /// Counters of a test's own (never freed: a few bytes a test).
    #[cfg(test)]
    pub fn for_test() -> CountTo {
        CountTo(Box::leak(Box::new(Counters::new())))
    }

    /// What these counters say now.
    pub fn totals(&self) -> StageTotals {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        StageTotals {
            jobs: g(&self.jobs),
            feeds: g(&self.feeds),
            throws: g(&self.throws),
            views: g(&self.views),
            displays: g(&self.displays),
            cpu_us_views: g(&self.cpu_us_views),
            cpu_us_displays: g(&self.cpu_us_displays),
            cpu_us_worst: g(&self.cpu_us_worst),
            frames_by_ms: std::array::from_fn(|i| g(&self.frames_by_ms[i])),
            missing_pictures: g(&self.missing_pictures),
            display_missing_pictures: g(&self.display_missing_pictures),
            light_left_out: g(&self.light_left_out),
            shadows_drawn: g(&self.shadows_drawn),
            thread_starts: g(&self.thread_starts),
            thread_stops: g(&self.thread_stops),
            restarts: g(&self.restarts),
            idle_releases: g(&self.idle_releases),
            throws_left_out: g(&self.throws_left_out),
            venue_loads: g(&self.venue_loads),
            venue_failures: g(&self.venue_failures),
            venue_load_ms: g(&self.venue_load_ms),
            venue_parts: g(&self.venue_parts),
            venue_triangles: g(&self.venue_triangles),
            maps_baked: g(&self.maps_baked),
            maps_refused: g(&self.maps_refused),
        }
    }
}

impl Default for Counters {
    fn default() -> Self {
        Counters::new()
    }
}

/// A look at the counters (`totals()`). Running sums unless said: the
/// difference of two looks (`since`, as Test Mode does with
/// `crumbs::stall_count`) is what happened between them. The levels
/// (`cpu_us_worst`, `throws_left_out`, the last venue read) are as they
/// stand at the later look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageTotals {
    /// Jobs drawn (a pass's views and the pictures they need).
    pub jobs: u64,
    /// Displays' pictures composited for the screens on the stage.
    pub feeds: u64,
    /// Projectors' pictures composited for the light.
    pub throws: u64,
    /// Previews' views drawn (and those of displays only the previews show,
    /// drawn at their pace).
    pub views: u64,
    /// Views drawn for displays that show the stage in a window or stream.
    pub displays: u64,
    /// The processor time `render` took (µs), summed over `views`...
    pub cpu_us_views: u64,
    /// ...and over `displays` (the graphics card's own time isn't known).
    pub cpu_us_displays: u64,
    /// A level: the longest of any one view (µs) since counting began.
    pub cpu_us_worst: u64,
    /// Views by their processor time: band `i` up to `FRAME_MS_BANDS[i]`
    /// ms, the last over all of them. A `since` gives the worst between two
    /// looks to a band (`worst_band_ms`).
    pub frames_by_ms: [u64; BANDS],
    /// Screens drawn without a picture from their display, in every view...
    pub missing_pictures: u64,
    /// ...and in the displays' views alone.
    pub display_missing_pictures: u64,
    /// Views drawn without the light they asked for (the card refused it;
    /// `render3d::Renderer::light_refused`).
    pub light_left_out: u64,
    /// Projectors' shadow maps drawn again.
    pub shadows_drawn: u64,
    /// The drawing thread started (the first time, after idling, after a
    /// device was lost, after a stop)...
    pub thread_starts: u64,
    /// ...found stopped (a panic in it)...
    pub thread_stops: u64,
    /// ...started again after such a stop...
    pub restarts: u64,
    /// ...and let go with nothing to draw (`stage_live::IDLE_RELEASE`).
    pub idle_releases: u64,
    /// A level: the visible projectors past `light::MAX_THROWS` at the last
    /// pass that worked out the light (they throw none).
    pub throws_left_out: u64,
    /// Venue models read...
    pub venue_loads: u64,
    /// ...and those that failed to read.
    pub venue_failures: u64,
    /// Levels: the last venue read, how long it took (ms), its parts and its
    /// triangles.
    pub venue_load_ms: u64,
    pub venue_parts: u64,
    pub venue_triangles: u64,
    /// Mapped projectors' lens maps baked...
    pub maps_baked: u64,
    /// ...and those the card refused (the projector stays black).
    pub maps_refused: u64,
}

impl StageTotals {
    /// What happened between `earlier` and this look: the sums subtracted,
    /// the levels this look's.
    pub fn since(&self, earlier: &StageTotals) -> StageTotals {
        let d = |a: u64, b: u64| a.saturating_sub(b);
        StageTotals {
            jobs: d(self.jobs, earlier.jobs),
            feeds: d(self.feeds, earlier.feeds),
            throws: d(self.throws, earlier.throws),
            views: d(self.views, earlier.views),
            displays: d(self.displays, earlier.displays),
            cpu_us_views: d(self.cpu_us_views, earlier.cpu_us_views),
            cpu_us_displays: d(self.cpu_us_displays, earlier.cpu_us_displays),
            frames_by_ms: std::array::from_fn(|i| d(self.frames_by_ms[i], earlier.frames_by_ms[i])),
            missing_pictures: d(self.missing_pictures, earlier.missing_pictures),
            display_missing_pictures: d(
                self.display_missing_pictures,
                earlier.display_missing_pictures,
            ),
            light_left_out: d(self.light_left_out, earlier.light_left_out),
            shadows_drawn: d(self.shadows_drawn, earlier.shadows_drawn),
            thread_starts: d(self.thread_starts, earlier.thread_starts),
            thread_stops: d(self.thread_stops, earlier.thread_stops),
            restarts: d(self.restarts, earlier.restarts),
            idle_releases: d(self.idle_releases, earlier.idle_releases),
            venue_loads: d(self.venue_loads, earlier.venue_loads),
            venue_failures: d(self.venue_failures, earlier.venue_failures),
            maps_baked: d(self.maps_baked, earlier.maps_baked),
            maps_refused: d(self.maps_refused, earlier.maps_refused),
            ..*self
        }
    }

    /// The upper bound (ms) of the slowest band with a view in it (None: no
    /// view; `u64::MAX`: over the last band). On a `since`, the worst view
    /// between the two looks, to a band.
    pub fn worst_band_ms(&self) -> Option<u64> {
        let i = self.frames_by_ms.iter().rposition(|&n| n > 0)?;
        Some(FRAME_MS_BANDS.get(i).copied().unwrap_or(u64::MAX))
    }

    /// The mean processor time of a view, displays' and previews' (µs).
    pub fn mean_cpu_us(&self) -> Option<u64> {
        let n = self.views + self.displays;
        (n > 0).then(|| (self.cpu_us_views + self.cpu_us_displays) / n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_are_banded_and_a_since_gives_what_happened_between() {
        let to = Counters::for_test();
        let stats = |ms: u64| FrameStats {
            cpu_time: Duration::from_millis(ms),
            missing_pictures: 1,
            ..FrameStats::default()
        };
        let before_all = totals();
        to.view(true, &stats(3));
        let first = to.0.totals();
        to.view(false, &stats(20));
        to.view(false, &stats(500));
        to.thread_started(false);
        to.venue_read(Duration::from_millis(41), Some((3, 1200)));
        let t = to.0.totals();
        assert_eq!((t.views, t.displays), (2, 1));
        assert_eq!(t.cpu_us_worst, 500_000);
        assert_eq!(t.frames_by_ms[2], 1, "3 ms: up to 4");
        assert_eq!(t.frames_by_ms[5], 1, "20 ms: up to 33");
        assert_eq!(t.frames_by_ms[8], 1, "500 ms: over them all");
        assert_eq!((t.missing_pictures, t.display_missing_pictures), (3, 1));
        assert_eq!(t.mean_cpu_us(), Some(523_000 / 3));
        let between = t.since(&first);
        assert_eq!((between.views, between.displays), (2, 0));
        assert_eq!(between.worst_band_ms(), Some(u64::MAX));
        assert_eq!(first.worst_band_ms(), Some(4));
        assert_eq!(StageTotals::default().worst_band_ms(), None);
        // Levels are the later look's.
        assert_eq!((between.venue_load_ms, between.venue_triangles), (41, 1200));
        assert_eq!(between.cpu_us_worst, 500_000);
        // A test's own counters count into the program's too.
        let all = totals().since(&before_all);
        assert!(all.views >= 2 && all.displays >= 1 && all.thread_starts >= 1);
    }
}
