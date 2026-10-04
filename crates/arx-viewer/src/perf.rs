//! Where a frame's time goes. `ARX_LOG_FPS=1` prints, every two seconds, the frames per second and the milliseconds
//! each timed system took per frame; what is left over is Bevy's own work (drawing, interface layout, uploads).
//! It also turns VSync off, so that the number is the real cost and not the screen's refresh rate.

use bevy::prelude::*;
use std::time::{Duration, Instant};

#[derive(Resource)]
pub struct Perf {
    pub enabled: bool,
    mark: Instant,
    frame_start: Instant,
    since_report: Instant,
    frames: u32,
    totals: Vec<(&'static str, Duration)>,
}

impl Default for Perf {
    fn default() -> Self {
        let now = Instant::now();
        Perf { enabled: std::env::var_os("ARX_LOG_FPS").is_some(), mark: now, frame_start: now, since_report: now, frames: 0, totals: Vec::new() }
    }
}

/// First system of the frame.
pub fn begin(mut perf: ResMut<Perf>, drawn: Query<&ViewVisibility, With<Mesh3d>>, materials: Res<Assets<StandardMaterial>>, meshes: Res<Assets<Mesh>>, nodes: Query<(), With<Node>>) {
    if !perf.enabled {
        return;
    }
    let now = Instant::now();
    perf.frames += 1;
    let elapsed = now - perf.since_report;
    if elapsed >= Duration::from_secs(2) {
        let frames = perf.frames as f32;
        let frame_ms = elapsed.as_secs_f32() * 1000.0 / frames;
        let mut timed = 0.0;
        let mut parts = String::new();
        perf.totals.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, total) in &perf.totals {
            let ms = total.as_secs_f32() * 1000.0 / frames;
            timed += ms;
            if ms >= 0.05 {
                parts.push_str(&format!("  {name} {ms:.2}"));
            }
        }
        eprintln!("fps {:.0} ({frame_ms:.2} ms): engine {:.2}{parts}", 1000.0 / frame_ms, frame_ms - timed);
        eprintln!("    {:.1} meshes rewritten per frame", TOUCHED.swap(0, std::sync::atomic::Ordering::Relaxed) as f32 / frames);
        eprintln!("    {} mesh entities ({} visible), {} meshes, {} materials, {} interface nodes", drawn.iter().count(), drawn.iter().filter(|v| v.get()).count(), meshes.len(), materials.len(), nodes.iter().count());
        perf.totals.clear();
        perf.frames = 0;
        perf.since_report = now;
    }
    perf.frame_start = now;
    perf.mark = now;
}

/// A system that charges the time since the last mark to `name`; put it right after the system it measures.
pub fn lap(name: &'static str) -> impl FnMut(ResMut<Perf>) {
    move |mut perf: ResMut<Perf>| {
        if !perf.enabled {
            return;
        }
        let now = Instant::now();
        let spent = now - perf.mark;
        perf.mark = now;
        match perf.totals.iter_mut().find(|(n, _)| *n == name) {
            Some((_, total)) => *total += spent,
            None => perf.totals.push((name, spent)),
        }
    }
}

/// Meshes rewritten (and so uploaded again) since the last report: the thing that costs most.
pub static TOUCHED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `ARX_SKIP=name,name`: leave systems out, to see what they cost (development only).
pub fn skip(name: &str) -> bool {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| std::env::var("ARX_SKIP").map(|v| v.split(',').map(str::to_owned).collect()).unwrap_or_default()).iter().any(|n| n == name)
}
