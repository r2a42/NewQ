//! `newq --bench-video <file>`: how hard a video file is to play on this
//! computer. It plays the file on a number of layers as a show would, for a
//! while, and reports what the pipeline delivered and what it cost: frames
//! per second per layer, ticks of a 60 Hz output that found no new frame, the
//! processor used by NewQ and the FFmpeg processes it started, memory, and
//! with `--gpu` the time to upload and draw every layer.
//!
//! ```text
//! newq --bench-video clip.mp4 [--layers 4] [--seconds 10] [--gpu] [--no-hap] [--size 1920x1080]
//! ```

use crate::gpu::{DrawGroup, DrawLayer, Geometry, Gpu, LayerTexture, Slot, corner_pin};
use crate::media::ffmpeg::Ffmpeg;
use crate::media::playback::Playback;
use crate::media::video::{self, VideoPlayer};
use crate::show::{ColorAdjust, Fit, LayerTransform};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Debug, PartialEq)]
struct Options {
    file: std::path::PathBuf,
    layers: usize,
    seconds: f64,
    gpu: bool,
    no_hap: bool,
    size: (u32, u32),
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        file: Default::default(),
        layers: 1,
        seconds: 10.0,
        gpu: false,
        no_hap: false,
        size: (1920, 1080),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--layers" => {
                o.layers = value("--layers")?
                    .parse::<usize>()
                    .map_err(|e| format!("--layers: {e}"))?
                    .clamp(1, 64)
            }
            "--seconds" => {
                o.seconds = value("--seconds")?
                    .parse::<f64>()
                    .map_err(|e| format!("--seconds: {e}"))?
                    .clamp(1.0, 86_400.0)
            }
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size is like 1920x1080")?;
                o.size = (
                    w.parse().map_err(|e| format!("--size: {e}"))?,
                    h.parse().map_err(|e| format!("--size: {e}"))?,
                );
            }
            "--gpu" => o.gpu = true,
            "--no-hap" => o.no_hap = true,
            other if !other.starts_with("--") && o.file.as_os_str().is_empty() => {
                o.file = other.into()
            }
            other => return Err(format!("don't know {other}")),
        }
    }
    if o.file.as_os_str().is_empty() {
        return Err("which file?".into());
    }
    Ok(o)
}

/// Run the benchmark if the program was asked to. The exit code, if so.
pub fn run_if_asked(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some("--bench-video") {
        return None;
    }
    Some(match parse(&args[1..]).and_then(run) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("bench-video: {e}");
            2
        }
    })
}

/// A percentile of `v` (sorted in place), 0 to 1.
fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn run(o: Options) -> Result<(), String> {
    let ff = Ffmpeg::locate().ok_or("ffmpeg not found")?;
    let info = ff.probe(&o.file).map_err(|e| format!("{e:#}"))?;
    let vi = info.video.clone().ok_or("the file has no picture")?;
    let gpu = if o.gpu {
        Some(Gpu::headless().ok_or("no graphics adapter for --gpu")?)
    } else {
        None
    };
    video::set_bc_supported(gpu.as_ref().is_some_and(Gpu::reads_bc) && !o.no_hap);
    let hap = video::plays_as_hap(&vi);
    let playback = Playback::plan(0.0, None, info.duration, true, None);
    let size = video::decode_size((vi.width, vi.height), o.size, false);
    println!(
        "{}: {}x{} {:.2} fps, {} codec, {} layer(s), {}, decoding at {}x{}",
        o.file.display(),
        vi.width,
        vi.height,
        vi.fps,
        vi.codec,
        o.layers,
        if hap {
            "Hap straight to the card"
        } else {
            "FFmpeg decode and scale"
        },
        size.0,
        size.1
    );
    let mut players = Vec::new();
    for _ in 0..o.layers {
        let p = if hap {
            VideoPlayer::spawn_hap(&ff, &o.file, &vi, 0.0, playback, 1.0, None)
        } else {
            VideoPlayer::spawn_at_rate(&ff, &o.file, &vi, 0.0, playback, size, 1.0, None)
        }
        .map_err(|e| format!("{e:#}"))?;
        players.push(p);
    }
    // The picture of each layer on the card, and somewhere to draw them.
    let mut textures: Vec<Option<LayerTexture>> = (0..o.layers).map(|_| None).collect();
    let target = gpu.as_ref().map(|g| {
        g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench"),
            size: wgpu::Extent3d {
                width: o.size.0,
                height: o.size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    });
    let pipeline = gpu.as_ref().map(|g| {
        g.compositor
            .pipeline(&g.device, wgpu::TextureFormat::Rgba8Unorm)
    });

    let mut sys = System::new();
    let me = Pid::from_u32(std::process::id());
    let (mut cpu_samples, mut rss_max) = (Vec::new(), 0u64);
    let mut last_sample = Instant::now();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let period = Duration::from_secs_f64(1.0 / 60.0);
    let start = Instant::now();
    let mut next = start;
    let (mut ticks, mut stale_ticks, mut dropped_ticks) = (0u64, 0u64, 0u64);
    let mut frames = vec![0u64; o.layers];
    let mut last_serial = vec![0u64; o.layers];
    let mut gpu_ms = Vec::new();
    let mut tick_late_ms = Vec::new();
    while start.elapsed().as_secs_f64() < o.seconds {
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        let lateness = Instant::now().saturating_duration_since(next);
        tick_late_ms.push(lateness.as_secs_f64() * 1000.0);
        // A tick later than a whole frame is one a real output would have dropped:
        // count it, and carry on from now rather than racing to catch up.
        if lateness > period {
            dropped_ticks += 1;
            next = Instant::now() + period;
        } else {
            next += period;
        }
        let t = start.elapsed().as_secs_f64();
        ticks += 1;
        let mut any_new = false;
        for (i, p) in players.iter_mut().enumerate() {
            p.update(t);
            if p.serial != last_serial[i] {
                last_serial[i] = p.serial;
                frames[i] += 1;
                any_new = true;
                if let (Some(g), Some(f)) = (&gpu, p.current()) {
                    let tex = textures[i].get_or_insert_with(|| match f.hap {
                        Some(h) => LayerTexture::new_bc(g, p.width, p.height, h),
                        None if p.rgba => LayerTexture::new_rgba(g, p.width, p.height),
                        None => LayerTexture::new_yuv(g, p.width, p.height, p.matrix),
                    });
                    match f.hap {
                        Some(_) => tex.upload_bc(&g.queue, &f.data),
                        None if p.rgba => tex.upload_rgba(&g.queue, &f.data),
                        None => tex.upload_yuv(&g.queue, &f.data),
                    }
                }
            }
        }
        if !any_new {
            stale_ticks += 1;
        }
        // Draw every layer that has a picture and wait for the card.
        if let (Some(g), Some(target), Some(pipeline)) = (&gpu, &target, &pipeline) {
            let layers: Vec<DrawLayer> = textures
                .iter()
                .flatten()
                .map(|t| DrawLayer {
                    texture: t,
                    opacity: 1.0,
                    fit: Fit::Stretch,
                    transform: LayerTransform::default(),
                    color: ColorAdjust::default(),
                    fx: Default::default(),
                })
                .collect();
            if !layers.is_empty() {
                let pin = corner_pin(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
                let groups = [DrawGroup {
                    layers,
                    geometry: Geometry {
                        canvas: o.size,
                        warp: pin,
                        pin,
                        mesh: None,
                        slice: Geometry::WHOLE,
                    },
                }];
                let began = Instant::now();
                let view = target.create_view(&Default::default());
                let cmd = g.composite(pipeline, Slot::Window(0), &view, &groups, None);
                g.queue.submit([cmd]);
                let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
                gpu_ms.push(began.elapsed().as_secs_f64() * 1000.0);
            }
        }
        if last_sample.elapsed() >= Duration::from_secs(1) {
            last_sample = Instant::now();
            sys.refresh_processes(ProcessesToUpdate::All, true);
            let (mut cpu, mut rss) = (0.0f32, 0u64);
            for (pid, p) in sys.processes() {
                if *pid == me || p.parent() == Some(me) {
                    cpu += p.cpu_usage();
                    rss += p.memory();
                }
            }
            cpu_samples.push(f64::from(cpu));
            rss_max = rss_max.max(rss);
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let per_layer: Vec<f64> = frames.iter().map(|f| *f as f64 / secs).collect();
    let mean_fps = per_layer.iter().sum::<f64>() / per_layer.len() as f64;
    let worst_fps = per_layer.iter().copied().fold(f64::MAX, f64::min);
    println!("  ran {secs:.1} s, {ticks} ticks of a 60 Hz output");
    println!(
        "  frames per layer: {mean_fps:.1}/s on average, {worst_fps:.1}/s at worst (the file is {:.1})",
        vi.fps
    );
    println!(
        "  ticks with no new frame on any layer: {:.1}%",
        100.0 * stale_ticks as f64 / ticks.max(1) as f64
    );
    println!(
        "  ticks dropped (later than a whole frame): {dropped_ticks} ({:.1}%)",
        100.0 * dropped_ticks as f64 / ticks.max(1) as f64
    );
    println!(
        "  tick lateness: p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        percentile(&mut tick_late_ms.clone(), 0.5),
        percentile(&mut tick_late_ms.clone(), 0.99),
        percentile(&mut tick_late_ms, 1.0)
    );
    if !gpu_ms.is_empty() {
        println!(
            "  upload and draw of all layers: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms (a frame is 16.7 ms)",
            percentile(&mut gpu_ms.clone(), 0.5),
            percentile(&mut gpu_ms.clone(), 0.95),
            percentile(&mut gpu_ms, 1.0)
        );
    }
    if !cpu_samples.is_empty() {
        let mean = cpu_samples.iter().sum::<f64>() / cpu_samples.len() as f64;
        println!(
            "  processor, NewQ and FFmpeg together: {mean:.0}% of one core on average, {:.0}% at most; memory up to {} MB",
            percentile(&mut cpu_samples, 1.0),
            rss_max >> 20
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_options_are_read_and_bad_ones_are_said() {
        let o = parse(&args(&[
            "clip.mp4",
            "--layers",
            "4",
            "--seconds",
            "5",
            "--gpu",
            "--size",
            "3840x2160",
        ]))
        .unwrap();
        assert_eq!(
            o,
            Options {
                file: "clip.mp4".into(),
                layers: 4,
                seconds: 5.0,
                gpu: true,
                no_hap: false,
                size: (3840, 2160),
            }
        );
        // Limits keep it sane.
        assert_eq!(parse(&args(&["a", "--layers", "9999"])).unwrap().layers, 64);
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["a", "--layers"])).is_err());
        assert!(parse(&args(&["a", "--size", "big"])).is_err());
        assert!(parse(&args(&["a", "--nonsense"])).is_err());
        assert_eq!(run_if_asked(&args(&["show.newq"])), None);
    }

    #[test]
    fn percentiles_pick_from_the_sorted_values() {
        let mut v = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&mut v, 0.0), 1.0);
        assert_eq!(percentile(&mut v, 0.5), 3.0);
        assert_eq!(percentile(&mut v, 1.0), 5.0);
        assert_eq!(percentile(&mut [], 0.5), 0.0);
    }

    #[test]
    fn a_short_run_over_real_media_reports_and_succeeds() {
        let Some((_, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let o = parse(&args(&[
            clip.to_str().unwrap(),
            "--layers",
            "2",
            "--seconds",
            "2",
        ]))
        .unwrap();
        run(o).unwrap();
        // With the graphics card too, where there is one.
        if Gpu::headless().is_some() {
            let o = parse(&args(&[clip.to_str().unwrap(), "--seconds", "2", "--gpu"])).unwrap();
            run(o).unwrap();
        }
    }
}
