//! Lens maps on the stage-render thread (`MapJob`): baked in their order
//! and landed where the app looks for them.

use super::*;
use crate::stage3d::lensmap::tests::{flat_show, still_of};
use crate::stage3d::lensmap::{feeds_for, map_key, still_key};

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless();
    if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
        panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
    }
    gpu
}

fn map_job(output: bool, size: (u32, u32)) -> MapJob {
    let (show, _, _, p) = flat_show();
    let scene = Arc::new(still_of(&show));
    let pr = show.projector(p).unwrap().clone();
    let aspect = pr.aspect(size);
    let feeds = feeds_for(&show, p);
    MapJob {
        key: map_key(still_key(&scene), &pr, aspect, size, false, &feeds),
        feeds,
        projector: pr,
        aspect,
        size,
        scene,
        output,
        depth: false,
    }
}

#[test]
fn merging_keeps_the_newer_maps_and_the_older_others() {
    let a = map_job(true, (64, 36));
    let mut a2 = a.clone();
    a2.key ^= 1;
    let b = map_job(false, (32, 18));
    let old = StageJob {
        maps: vec![a, b],
        serial: 1,
        ..StageJob::default()
    };
    let new = StageJob {
        maps: vec![a2.clone()],
        serial: 2,
        ..StageJob::default()
    };
    let both = old.merged(new);
    assert_eq!(both.maps.len(), 2);
    assert_eq!(both.maps[0].key, a2.key, "the newer one of the same map");
    assert_eq!(both.maps[1].size, (32, 18), "the other kept");
}

#[test]
fn the_drawer_bakes_a_windows_map_first_and_a_views_last_and_lands_them() {
    let Some(gpu) = gpu() else { return };
    let mut drawer = StageDrawer::counting(&gpu, Counters::for_test());
    let slot = BakedSlot::default();
    drawer.baked_into(slot.clone());
    let (window, view) = (map_job(true, (64, 36)), map_job(false, (32, 18)));
    let job = StageJob {
        // The view's first in the list: the window's is still baked first.
        maps: vec![view.clone(), window.clone()],
        serial: 1,
        ..StageJob::default()
    };
    // What had landed when the displays were done: the window's alone.
    let mut at_displays = Vec::new();
    drawer.draw(&gpu, &job, &mut || {
        at_displays = slot.lock().iter().map(|b| b.size).collect();
    });
    assert_eq!(at_displays, vec![window.size]);
    let baked = drawer.take_baked();
    assert_eq!(baked.len(), 2);
    assert_eq!((baked[0].size, baked[0].key), (window.size, window.key));
    assert_eq!(baked[1].size, view.size);
    assert!(baked.iter().all(|b| b.map.is_some()));
    assert_eq!(baked[0].map.as_ref().unwrap().key, window.key);
}

#[test]
fn a_refused_bake_lands_as_none_and_is_counted() {
    let Some(gpu) = gpu() else { return };
    let count = Counters::for_test();
    let mut drawer = StageDrawer::counting(&gpu, count);
    drawer.refuse_bake();
    let job = StageJob {
        maps: vec![map_job(true, (16, 9))],
        serial: 1,
        ..StageJob::default()
    };
    drawer.draw(&gpu, &job, &mut || {});
    let baked = drawer.take_baked();
    assert_eq!(baked.len(), 1);
    assert!(baked[0].map.is_none());
    let t = count.0.totals();
    assert_eq!((t.maps_baked, t.maps_refused), (0, 1));
}
