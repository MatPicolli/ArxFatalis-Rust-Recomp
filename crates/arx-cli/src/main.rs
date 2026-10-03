use anyhow::{Context, Result};
use arx_formats::PakSet;
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;

const DEFAULT_GAME_DIR: &str = r"D:\Steam\steamapps\common\Arx Fatalis";

#[derive(Parser)]
#[command(about = "Arx Fatalis asset tool")]
struct Cli {
    /// Game installation directory (containing data.pak etc.)
    #[arg(long, env = "ARX_DIR", default_value = DEFAULT_GAME_DIR, global = true)]
    game_dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// File counts and sizes per top-level directory and per file extension
    Stats,
    /// List files below a virtual path prefix
    Ls { prefix: Option<String> },
    /// Extract one file to `--out` (default: current dir, flat)
    Extract { path: String, #[arg(short, long)] out: Option<PathBuf> },
    /// Extract every file below a prefix, keeping the directory structure
    ExtractAll { prefix: Option<String>, #[arg(short, long)] out: PathBuf },
    /// Decompress every file and report failures
    Verify,
    /// Parse one .ftl model and describe it, or (no path) parse all of them and report failures
    Ftl { path: Option<String> },
    /// Parse level geometry (`game/graph/levels/levelN/fast.fts`); no argument = all levels
    Fts { level: Option<u32> },
    /// Load a language's text and check that every entity name scripts set (setname) resolves; with a key,
    /// print its variants
    Locale { #[arg(default_value = "english")] language: String, key: Option<String> },
    /// Entity orientation check: sample points along each door/portcullis should be in free space with an end touching a wall; compares the engine's rotation with raw-yaw alternatives
    OrientPanel { #[arg(default_value = "light_door")] class_filter: String },
    /// Orientation check for wall-mounted objects: counts entities whose back is against a wall versus facing into one
    OrientWall { #[arg(default_value = "")] class_filter: String },
    /// List every level polygon (any flags) covering the Arx-coordinate point (x, z), to debug collision
    PolysAt { level: u32, x: f32, z: f32 },
    /// Play a level headlessly through its scripts: each step is `list <text>`, `pickup <id>`, `use <id>`,
    /// `combine <id> <target id>`, `send <id> <event>`, `drop <id>`, `open <container>`, `close`, `contents <id>`, `take <container> <item|all>` or `status`
    Game { level: u32, steps: Vec<String> },
    /// Send `action` to every fixture (doors, levers, chests, switches, ...) of a level (no argument = all levels)
    /// and report, per class, how many reacted visibly (animation, sound, message, speech, state change) and
    /// which unimplemented commands they hit
    Fixtures { level: Option<u32> },
    /// Original player movement speeds, derived from the hero animations the way the engine does (root motion
    /// per millisecond, scaled by 0.0125; the engine's velocity damping then settles at that / 0.009)
    PlayerSpeeds,
    /// Decode every .wav in the archives (sfx, speech) and report failures and totals; with a path,
    /// decode that file to a 16-bit PCM wav at `--out`
    Audio { path: Option<String>, #[arg(short, long)] out: Option<PathBuf> },
    /// Load every entity script of a level, run INIT/INITEND for all, and report what the interpreter
    /// could not handle (no argument = all levels, one world per level)
    Script { level: Option<u32>, /// how many unknown commands / warnings to list
        #[arg(short, long, default_value_t = 25)] top: usize,
        /// with a level: list what the scripts did to each entity (hidden, destroyed, mesh, scale)
        #[arg(short, long)] details: bool },
    /// Simulate the player in a level: drop from the start, then walk in 8 directions and report how
    /// far each walk got before hitting a wall (a headless check of the collision system)
    Walk { level: u32, /// ignore doors and other entities (level geometry only)
        #[arg(long)] no_entities: bool,
        /// never jump during the fuzz (separates collision problems from long jumps over edges)
        #[arg(long)] no_jump: bool },
    /// Parse a .tea animation (and with --model, pose that .ftl at several times); no path = parse all
    Tea { path: Option<String>, #[arg(long)] model: Option<String> },
    /// Parse level scene definitions (`graph/levels/levelN/levelN.dlf`); no argument = all levels
    Dlf { level: Option<u32>, /// print every entity
        #[arg(short, long)] entities: bool },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let pak = std::sync::Arc::new(PakSet::open_game_dir(&cli.game_dir)
        .with_context(|| format!("opening {}", cli.game_dir.display()))?);

    match cli.cmd {
        Cmd::Stats => {
            let mut by_dir: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
            let mut by_ext: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
            for (name, e) in pak.iter() {
                let top = name.split('/').next().unwrap_or("");
                let ext = name.rsplit_once('.').map_or("", |(_, x)| x);
                for (map, k) in [(&mut by_dir, top), (&mut by_ext, ext)] {
                    let s = map.entry(k).or_default();
                    s.0 += 1;
                    s.1 += e.size;
                }
            }
            println!("{} files total", pak.len());
            println!("\nby top-level directory:");
            for (k, (n, sz)) in &by_dir { println!("  {k:<16} {n:>7} files {:>10.1} MiB", *sz as f64 / 1048576.0); }
            println!("\nby extension:");
            for (k, (n, sz)) in &by_ext { println!("  .{k:<15} {n:>7} files {:>10.1} MiB", *sz as f64 / 1048576.0); }
        }
        Cmd::Ls { prefix } => {
            for name in pak.list(prefix.as_deref().unwrap_or("")) {
                let e = pak.entry(name).unwrap();
                println!("{:>10} {} {} [{}]", e.size, if e.is_compressed() { "z" } else { " " }, name, e.origin(&pak));
            }
        }
        Cmd::Extract { path, out } => {
            let data = pak.read(&path)?;
            let out = out.unwrap_or_else(|| PathBuf::from(path.rsplit(['/', '\\']).next().unwrap()));
            std::fs::write(&out, &*data)?;
            println!("wrote {} bytes to {}", data.len(), out.display());
        }
        Cmd::ExtractAll { prefix, out } => {
            let names = pak.list(prefix.as_deref().unwrap_or(""));
            for name in &names {
                let dest = out.join(name);
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::write(&dest, &*pak.read(name)?)?;
            }
            println!("extracted {} files to {}", names.len(), out.display());
        }
        Cmd::Verify => {
            let (mut ok, mut bad, mut compressed) = (0usize, 0usize, 0usize);
            for name in pak.list("") {
                let e = pak.entry(name).unwrap();
                compressed += e.is_compressed() as usize;
                match pak.read(name) {
                    Ok(_) => ok += 1,
                    Err(err) => { bad += 1; eprintln!("FAIL {name}: {err}"); }
                }
            }
            println!("{ok} ok ({compressed} compressed), {bad} failed");
            if bad > 0 { std::process::exit(1); }
        }
        Cmd::Ftl { path: Some(path) } => {
            let m = arx_formats::ftl::Ftl::parse(&pak.read(&path)?)?;
            println!("name: {}", m.name);
            println!("origin vertex: {}", m.origin);
            println!("{} vertices, {} faces", m.vertices.len(), m.faces.len());
            for (i, t) in m.textures.iter().enumerate() {
                println!("  texture {i}: {t:?} -> {:?}", pak.find_texture(t));
            }
            println!("{} groups, {} actions, {} selections", m.groups.len(), m.actions.len(), m.selections.len());
            let (mn, mx) = m.vertices.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(mut a, mut b), v| {
                for i in 0..3 { a[i] = a[i].min(v.pos[i]); b[i] = b[i].max(v.pos[i]); }
                (a, b)
            });
            println!("bounds: {mn:?} .. {mx:?}");
        }
        Cmd::Fts { level } => {
            let levels: Vec<String> = pak.list("game/graph/levels").into_iter()
                .filter(|n| n.ends_with("/fast.fts"))
                .filter(|n| level.is_none_or(|l| n.contains(&format!("/level{l}/"))))
                .map(str::to_owned).collect();
            for name in levels {
                let t = std::time::Instant::now();
                match arx_formats::fts::Fts::parse(&pak.read(&name)?) {
                    Ok(f) => {
                        let tex_missing = f.textures.values().filter(|t| pak.find_texture(t).is_none()).count();
                        let drawn = f.polys.iter().filter(|p| p.flags & (arx_formats::poly::NODRAW | arx_formats::poly::HIDE) == 0).count();
                        let lvl: u32 = name.rsplit("/level").nth(0).and_then(|r| r.split('/').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
                        let verts: usize = f.polys.iter().map(|p| p.vertex_count()).sum();
                        match pak.load_llf(lvl) {
                            Some(l) => println!("  llf: {} lights, {} colours (expected {verts}){}", l.lights.len(), l.colors.len(), if l.colors.len() == verts { "" } else { "  MISMATCH" }),
                            None => println!("  llf: FAILED to load"),
                        }
                        println!("{name}: {}x{} tiles, {} polys ({drawn} drawable), {} textures ({tex_missing} missing), {} anchors, {} portals, {} rooms, trailing {} bytes, player {:?} [{:.0?}]",
                            f.size.0, f.size.1, f.polys.len(), f.textures.len(), f.anchors.len(), f.portals.len(), f.rooms.len(), f.trailing, f.player_pos, t.elapsed());
                    }
                    Err(e) => println!("{name}: FAILED {e}"),
                }
            }
        }
        Cmd::PolysAt { level, x, z } => {
            let fts = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{level}/fast.fts"))?)?;
            let inside = |p: &arx_formats::fts::Poly, idx: [usize; 3]| {
                let v = |i: usize| (p.verts[i].pos[0], p.verts[i].pos[2]);
                let (a, b, c) = (v(idx[0]), v(idx[1]), v(idx[2]));
                let s = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1);
                let (d1, d2, d3) = (s((x, z), a, b), s((x, z), b, c), s((x, z), c, a));
                !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
            };
            let mut rows = Vec::new();
            for p in &fts.polys {
                let hit = inside(p, [0, 1, 2]) || (p.is_quad() && inside(p, [3, 2, 1]));
                if !hit { continue; }
                let ys: Vec<f32> = p.verts[..p.vertex_count()].iter().map(|v| v.pos[1]).collect();
                let (y0, y1) = ys.iter().fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
                rows.push((y0, y1, p.flags, p.norm[1], p.tex, fts.textures.get(&p.tex).cloned().unwrap_or_default()));
            }
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            let cw = arx_physics::CollisionWorld::from_fts(&fts);
            println!("collision world floor at that point (y-up, searching up to y=-1000): {:?}", cw.floor_height(x, -z, -1000.0));
            println!("{} polygons cover ({x}, {z}); Arx y is down, so the first rows are the highest:", rows.len());
            for (y0, y1, f, ny, tex, name) in rows { println!("  y {y0:>8.1}..{y1:>8.1}  flags {f:#09x}  norm.y {ny:>5.2}  tex {tex} {name}"); }
        }
        Cmd::OrientWall { class_filter } => {
            use glam::{Quat, Vec3};
            // Moller-Trumbore, returns hit distance along a unit direction.
            fn ray_tri(o: Vec3, d: Vec3, t: &[Vec3; 3]) -> Option<f32> {
                let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
                let p = d.cross(e2);
                let det = e1.dot(p);
                if det.abs() < 1e-6 { return None; }
                let inv = 1.0 / det;
                let s = o - t[0];
                let u = s.dot(p) * inv;
                if !(0.0..=1.0).contains(&u) { return None; }
                let q = s.cross(e1);
                let v = d.dot(q) * inv;
                if v < 0.0 || u + v > 1.0 { return None; }
                let dist = e2.dot(q) * inv;
                (dist > 0.0).then_some(dist)
            }
            let (mut toward_wall, mut away_from_wall, mut n, mut flipped_check) = (0, 0, 0, 0);
            // [hypothesis][diagonal?] -> (back against wall, facing into wall)
            let mut split = [[(0i32, 0i32); 2]; 2];
            let mut by_class: BTreeMap<String, (i32, i32)> = BTreeMap::new();
            for l in (0..=30u32).filter(|l| pak.contains(&format!("graph/levels/level{l}/level{l}.dlf"))) {
                let Ok(dlf) = pak.load_dlf(l) else { continue };
                let Ok(fts) = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{l}/fast.fts"))?) else { continue };
                let sp = Vec3::from(fts.scene_pos);
                // Level walls only: steep triangles.
                let mut walls: Vec<[Vec3; 3]> = Vec::new();
                for p in fts.polys.iter().filter(|p| p.flags & (arx_formats::poly::HIDE | arx_formats::poly::NODRAW | arx_formats::poly::TRANS | arx_formats::poly::WATER) == 0) {
                    let v: Vec<Vec3> = p.verts[..p.vertex_count()].iter().map(|v| arx_level::to_yup(v.pos)).collect();
                    let mut push = |a: Vec3, b: Vec3, c: Vec3| { if (b - a).cross(c - a).normalize_or_zero().y.abs() < 0.5 { walls.push([a, b, c]); } };
                    push(v[0], v[1], v[2]);
                    if v.len() == 4 { push(v[3], v[2], v[1]); }
                }
                for e in dlf.entities.iter().filter(|e| (e.class.contains("/fix_inter/") || e.class.contains("/items/")) && !e.class.contains("door") && !e.class.contains("teleport") && e.class.contains(&class_filter)) {
                    let Some(m) = pak.read(&format!("game/{}.ftl", e.class)).ok().and_then(|b| arx_formats::ftl::Ftl::parse(&b).ok()) else { continue };
                    let origin = arx_level::to_yup([e.pos[0] + sp.x, e.pos[1] + sp.y, e.pos[2] + sp.z]);
                    let rot: Quat = arx_level::entity_rotation(e.angle, e.class.contains("/npc/"));
                    let alt: Quat = Quat::from_rotation_z(e.angle[2].to_radians()) * Quat::from_rotation_x(e.angle[0].to_radians()) * Quat::from_rotation_y(e.angle[1].to_radians());
                    let yaw_mod = e.angle[1].rem_euclid(90.0);
                    let diagonal = (yaw_mod > 6.0 && yaw_mod < 84.0) as usize;
                    // Where the mesh body lies relative to the origin, horizontally, in world space.
                    let n_v = m.vertices.len() as f32;
                    let centroid = m.vertices.iter().map(|v| arx_level::to_yup(v.pos)).sum::<Vec3>() / n_v;
                    let body = rot * centroid;
                    let (body_h, mid_y) = (Vec3::new(body.x, 0.0, body.z), origin.y + body.y);
                    if body_h.length() < 15.0 { continue; }
                    let dir = body_h.normalize();
                    let o = Vec3::new(origin.x, mid_y, origin.z);
                    let hit = |d: Vec3| walls.iter().filter_map(|t| ray_tri(o, d, t)).fold(f32::MAX, f32::min);
                    let (front, back) = (hit(dir), hit(-dir));
                    for (hi, r) in [rot, alt].into_iter().enumerate() {
                        let b = r * centroid;
                        let bh = Vec3::new(b.x, 0.0, b.z);
                        if bh.length() < 15.0 { continue; }
                        let d = bh.normalize();
                        let oo = Vec3::new(origin.x, origin.y + b.y, origin.z);
                        let h = |dd: Vec3| walls.iter().filter_map(|t| ray_tri(oo, dd, t)).fold(f32::MAX, f32::min);
                        let (f2, b2) = (h(d), h(-d));
                        if b2 < 40.0 && f2 > 100.0 { split[hi][diagonal].0 += 1; } else if f2 < 40.0 && b2 > 100.0 { split[hi][diagonal].1 += 1; }
                    }
                    // A wall-mounted object has its back (opposite the body) against the wall.
                    if back < 40.0 && front > 100.0 { toward_wall += 1; by_class.entry(e.class.rsplit('/').next().unwrap_or("").to_string()).or_default().0 += 1; }
                    else if front < 40.0 && back > 100.0 { away_from_wall += 1; by_class.entry(e.class.rsplit('/').next().unwrap_or("").to_string()).or_default().1 += 1; }
                    n += 1;
                    let _ = &mut flipped_check;
                }
            }
            println!("{n} entities with an off-centre mesh body; wall right behind the origin and open in front: {toward_wall}; wall in front of the body (object faces into the wall): {away_from_wall}");
            for (hi, name) in ["current Ry(-yaw)", "alternative Ry(+yaw)"].iter().enumerate() {
                println!("  {name:<22} axis-aligned yaws: back-to-wall {:>3} / into-wall {:>3}   diagonal yaws: back-to-wall {:>3} / into-wall {:>3}", split[hi][0].0, split[hi][0].1, split[hi][1].0, split[hi][1].1);
            }
            let mut v: Vec<_> = by_class.into_iter().collect();
            v.sort_by_key(|(_, (a, b))| -(a + b));
            for (cls, (back, front)) in v.into_iter().take(25) { println!("  {cls:<28} back-to-wall {back:>3}   facing-into-wall {front:>3}"); }
        }
        Cmd::OrientPanel { class_filter } => {
            use glam::{Quat, Vec3};
            fn closest(p: Vec3, t: &[Vec3; 3]) -> Vec3 {
                // Ericson, Real-Time Collision Detection: closest point on triangle.
                let (a, b, c) = (t[0], t[1], t[2]);
                let ab = b - a; let ac = c - a; let ap = p - a;
                let d1 = ab.dot(ap); let d2 = ac.dot(ap);
                if d1 <= 0.0 && d2 <= 0.0 { return a; }
                let bp = p - b; let d3 = ab.dot(bp); let d4 = ac.dot(bp);
                if d3 >= 0.0 && d4 <= d3 { return b; }
                let vc = d1 * d4 - d3 * d2;
                if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 { return a + ab * (d1 / (d1 - d3)); }
                let cp = p - c; let d5 = ab.dot(cp); let d6 = ac.dot(cp);
                if d6 >= 0.0 && d5 <= d6 { return c; }
                let vb = d5 * d2 - d1 * d6;
                if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 { return a + ac * (d2 / (d2 - d6)); }
                let va = d3 * d6 - d5 * d4;
                if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 { return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))); }
                let denom = 1.0 / (va + vb + vc);
                a + ab * (vb * denom) + ac * (vc * denom)
            }
            let rad = f32::to_radians;
            type Hyp = Box<dyn Fn([f32; 3]) -> Quat>;
            let hyps: Vec<(&str, Hyp)> = vec![
                ("Ry(-yaw)  [current]", Box::new(|a| arx_level::entity_rotation(a, false))),
                ("Ry(+yaw)", Box::new(move |a| Quat::from_rotation_y(rad(a[1])))),
                ("Ry(-yaw+90)", Box::new(move |a| Quat::from_rotation_y(-rad(a[1]) + std::f32::consts::FRAC_PI_2))),
                ("Ry(-yaw+180)", Box::new(move |a| Quat::from_rotation_y(-rad(a[1]) + std::f32::consts::PI))),
                ("Ry(+yaw+90)", Box::new(move |a| Quat::from_rotation_y(rad(a[1]) + std::f32::consts::FRAC_PI_2))),
                ("Ry(-yaw-90)", Box::new(move |a| Quat::from_rotation_y(-rad(a[1]) - std::f32::consts::FRAC_PI_2))),
            ];
            let mut score = vec![(0usize, 0usize, 0usize); hyps.len()]; // (clean panel & touches wall, panel crosses wall, floating in free space)
            let mut n = 0;
            for l in (0..=30u32).filter(|l| pak.contains(&format!("graph/levels/level{l}/level{l}.dlf"))) {
                let Ok(dlf) = pak.load_dlf(l) else { continue };
                let Ok(fts) = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{l}/fast.fts"))?) else { continue };
                let sp = Vec3::from(fts.scene_pos);
                let mut walls: Vec<[Vec3; 3]> = Vec::new();
                for p in fts.polys.iter().filter(|p| p.flags & (arx_formats::poly::HIDE | arx_formats::poly::NODRAW | arx_formats::poly::TRANS | arx_formats::poly::WATER) == 0) {
                    let v: Vec<Vec3> = p.verts[..p.vertex_count()].iter().map(|v| arx_level::to_yup(v.pos)).collect();
                    let mut push = |a: Vec3, b: Vec3, c: Vec3| { if (b - a).cross(c - a).normalize_or_zero().y.abs() < 0.6 { walls.push([a, b, c]); } };
                    push(v[0], v[1], v[2]);
                    if v.len() == 4 { push(v[3], v[2], v[1]); }
                }
                for e in dlf.entities.iter().filter(|e| e.class.contains("/fix_inter/") && e.class.contains(&class_filter)) {
                    let Some(m) = pak.read(&format!("game/{}.ftl", e.class)).ok().and_then(|b| arx_formats::ftl::Ftl::parse(&b).ok()) else { continue };
                    let origin = arx_level::to_yup([e.pos[0] + sp.x, e.pos[1] + sp.y, e.pos[2] + sp.z]);
                    let pts: Vec<Vec3> = m.vertices.iter().map(|v| arx_level::to_yup(v.pos)).collect();
                    let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
                    let along_x = (hi.x - lo.x) >= (hi.z - lo.z);
                    let mid = (lo + hi) * 0.5;
                    let sample = |t: f32| if along_x { Vec3::new(lo.x + (hi.x - lo.x) * t, mid.y, mid.z) } else { Vec3::new(mid.x, mid.y, lo.z + (hi.z - lo.z) * t) };
                    n += 1;
                    for (i, (_, rot)) in hyps.iter().enumerate() {
                        let r = rot(e.angle);
                        let dist = |local: Vec3| { let w = origin + r * local; walls.iter().map(|t| (closest(w, t) - w).length()).fold(f32::MAX, f32::min) };
                        let interior: Vec<f32> = (1..=8).map(|k| dist(sample(k as f32 / 9.0))).collect();
                        let crosses = interior.iter().filter(|d| **d < 6.0).count() >= 2;
                        let ends = dist(sample(-0.04)).min(dist(sample(1.04)));
                        if i == 0 && (crosses || ends >= 25.0) { println!("  level {l:<2} {:<22} #{:<4} yaw {:>7.1} {} (min interior distance {:.1}, end distance {:.1})", e.class.rsplit('/').next().unwrap_or(""), e.instance, e.angle[1], if crosses { "CROSSES WALL" } else { "floating" }, interior.iter().cloned().fold(f32::MAX, f32::min), ends); }
                        if crosses { score[i].1 += 1; } else if ends < 25.0 { score[i].0 += 1; } else { score[i].2 += 1; }
                    }
                }
            }
            println!("{n} '{class_filter}' entities. For each hypothesis: sits cleanly in an opening / panel crosses a wall (bad) / floating clear of walls (bad)");
            for ((name, _), s) in hyps.iter().zip(&score) { println!("  {name:<22} {:>4} / {:>4} / {:>4}", s.0, s.1, s.2); }
        }
        Cmd::Locale { language, key } => {
            let loc = pak.load_locale(&language).with_context(|| format!("no localisation/utext_{language}.ini"))?;
            if let Some(key) = key {
                println!("{} variants for {key:?}:", loc.count(&key));
                for (i, v) in loc.variants(&key).iter().enumerate() { println!("  {}: {v}", i + 1); }
                return Ok(());
            }
            println!("{language}: {} keys", loc.len());
            let (mut named, mut missing) = (0, std::collections::BTreeMap::<String, usize>::new());
            for l in (0..=30u32).filter(|l| pak.contains(&format!("graph/levels/level{l}/level{l}.dlf"))) {
                let Ok(dlf) = pak.load_dlf(l) else { continue };
                let Ok(fts) = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{l}/fast.fts"))?) else { continue };
                let s = arx_level::Scripts::build(&pak, &dlf, glam::Vec3::from(fts.scene_pos));
                for &id in &s.ids {
                    let Some(st) = s.host.state(id) else { continue };
                    if st.name.is_empty() { continue; }
                    named += 1;
                    if loc.get(&st.name).is_none() { *missing.entry(st.name.clone()).or_default() += 1; }
                }
            }
            println!("{named} entity names set by scripts; {} distinct names have no text:", missing.len());
            for (k, n) in missing.iter().take(20) { println!("  {k} (x{n})"); }

            // Every literal key given to `speak` / `herosay` by any script: does it have text and a voice?
            let (mut uses, mut no_text, mut no_voice) = (0usize, std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
            let mut keys = std::collections::BTreeSet::new();
            for (path, _) in pak.iter().filter(|(p, _)| p.ends_with(".asl")) {
                let Ok(bytes) = pak.read(path) else { continue };
                let text: String = bytes.iter().map(|&b| (b as char).to_ascii_lowercase()).collect();
                for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
                    let mut words = line.split_whitespace().map(|w| w.trim_matches(|c| c == '(' || c == ')'));
                    while let Some(w) = words.next() {
                        let spoken = w == "speak";
                        if !spoken && w != "herosay" { continue; }
                        let mut next = words.next();
                        let mut cinematic = false;
                        if let Some(f) = next.filter(|f| f.starts_with('-')) {
                            cinematic = spoken && f.contains('c');
                            next = words.next();
                        }
                        if cinematic {
                            // `-c <kind> <camera numbers...>` precedes the key.
                            let skip = match next { Some("zoom" | "side" | "side_l" | "side_r") => 6, Some(k) if k.starts_with("ccc") => 3, _ => 0 };
                            for _ in 0..skip { words.next(); }
                            if skip == 0 && next != Some("keep") { continue; }
                            next = words.next();
                        }
                        let Some(key) = next else { break };
                        let key = key.trim_matches('"');
                        let plain = key.trim_start_matches('[').trim_end_matches(']');
                        if plain.is_empty() || plain == "killall" || key.contains(['~', '#', '^', '$', '@', '\u{a7}', '\u{a3}']) || plain.starts_with(|c: char| c.is_ascii_digit()) || key.starts_with('[') != key.ends_with(']') { continue; }
                        uses += 1;
                        let k = plain.to_owned();
                        if loc.count(&k) == 0 { no_text.insert(k.clone()); }
                        if spoken { keys.insert(k); }
                    }
                }
            }
            for k in &keys {
                let n = loc.count(k).max(1);
                for v in 1..=n {
                    let file = if v == 1 { format!("speech/{language}/{k}.wav") } else { format!("speech/{language}/{k}{v}.wav") };
                    if !pak.contains(&file) { no_voice.insert(file); }
                }
            }
            println!("{uses} literal speak/herosay uses, {} distinct speak keys: {} keys without text, {} voice files missing", keys.len(), no_text.len(), no_voice.len());
            for k in no_text.iter().take(8) { println!("  no text: {k}"); }
            for k in no_voice.iter().take(8) { println!("  no voice: {k}"); }
        }
        Cmd::Game { level, steps } => {
            use arx_level::inventory::{self, PickUp};
            let dlf = pak.load_dlf(level).map_err(anyhow::Error::msg)?;
            let fts = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{level}/fast.fts"))?)?;
            let pak = std::sync::Arc::new(pak);
            let mut s = arx_level::Scripts::build(&pak, &dlf, glam::Vec3::from(fts.scene_pos));
            let loc = pak.load_locale("english").unwrap_or_default();
            let name = |s: &arx_level::Scripts, id: u32| {
                let n = s.host.state(id).map(|st| st.name.as_str()).filter(|n| !n.is_empty());
                format!("{} ({})", s.world.entity(id).id_string, n.map_or("?".to_owned(), |n| loc.text_or_key(n).to_owned()))
            };
            let find = |s: &arx_level::Scripts, id: &str| s.world.find(id, s.player).ok_or_else(|| anyhow::anyhow!("no entity {id}"));
            for step in &steps {
                let w: Vec<&str> = step.split_whitespace().collect();
                println!("> {step}");
                match w.as_slice() {
                    ["list", text] => {
                        for id in 0..s.world.entities.len() as u32 {
                            let e = s.world.entity(id);
                            if e.id_string.contains(text) { println!("  {}  pos {:.0},{:.0},{:.0}", name(&s, id), e.pos[0], e.pos[1], e.pos[2]); }
                        }
                    }
                    ["pickup", id] => {
                        let id = find(&s, id)?;
                        let r = inventory::pick_up(&mut s.world, &mut s.host, s.player, id);
                        println!("  {} -> {r:?}", name(&s, id));
                        if let PickUp::Stacked(t) = r { println!("  stack {} x{}", name(&s, t), s.host.state(t).map_or(0, |x| x.count)); }
                    }
                    ["use", id] => {
                        let id = find(&s, id)?;
                        println!("  {} -> {}", name(&s, id), inventory::use_item(&mut s.world, &mut s.host, s.player, id));
                    }
                    ["combine", a, b] => {
                        let (a, b) = (find(&s, a)?, find(&s, b)?);
                        let r = inventory::combine(&mut s.world, &mut s.host, s.player, a, b);
                        println!("  {} on {} -> {r:?}", name(&s, a), name(&s, b));
                    }
                    ["drop", id] => {
                        let id = find(&s, id)?;
                        let at = s.world.entity(s.player).pos;
                        println!("  {} -> {}", name(&s, id), inventory::drop_item(&mut s.world, &mut s.host, s.player, id, at));
                    }
                    ["send", id, event] => {
                        let id = find(&s, id)?;
                        let r = s.world.send_event(&mut s.host, Some(s.player), id, event, vec![]);
                        s.world.update(&mut s.host, 0.0);
                        println!("  {} {event} -> {r:?}", name(&s, id));
                    }
                    ["contents", id] => {
                        let id = find(&s, id)?;
                        let held = s.host.containers.get(&id).cloned();
                        match held {
                            None => println!("  {} holds nothing (no container)", name(&s, id)),
                            Some(list) => {
                                println!("  {} holds {} entries", name(&s, id), list.len());
                                for i in list { println!("    {} x{}", name(&s, i), s.host.state(i).map_or(0, |x| x.count)); }
                            }
                        }
                    }
                    ["open", id] => {
                        let id = find(&s, id)?;
                        println!("  {} -> opened: {}", name(&s, id), inventory::open_container(&mut s.world, &mut s.host, s.player, id));
                    }
                    ["close"] => inventory::close_container(&mut s.world, &mut s.host, s.player),
                    ["take", container, what] => {
                        let c = find(&s, container)?;
                        if *what == "all" {
                            println!("  took {}", inventory::take_all(&mut s.world, &mut s.host, s.player, c));
                        } else {
                            let item = find(&s, what)?;
                            println!("  {:?}", inventory::take_from_container(&mut s.world, &mut s.host, s.player, c, item));
                        }
                    }
                    ["status"] => {
                        if let Some(c) = s.host.open_container { println!("  open container: {}", name(&s, c)); }
                        let p = &s.host.player;
                        println!("  life {}/{}  mana {}/{}  hunger {}", p.life.current, p.life.max, p.mana.current, p.mana.max, p.hunger);
                        for &i in &p.inventory { println!("  carrying {} x{}", name(&s, i), s.host.state(i).map_or(0, |x| x.count)); }
                    }
                    _ => println!("  unknown step"),
                }
                for n in s.host.take_notes() { println!("  {:?}: {}", n.kind, loc.text_or_key(&n.text)); }
                for m in s.host.take_messages() { println!("  herosay: {}", loc.text_or_key(&m)); }
                for e in s.host.take_speech() { if let arx_script::SpeechEvent::Say(r) = e { println!("  speech: {}", loc.text_or_key(&r.key)); } }
                for snd in s.host.take_sounds() { println!("  sound: {}", snd.name); }
            }
        }
        Cmd::Fixtures { level } => {
            use arx_script::EntityKind;
            use std::collections::BTreeMap;
            #[derive(Default)]
            struct Row { tried: usize, reacted: usize, unknown: BTreeMap<String, usize>, example: String, effects: BTreeMap<&'static str, usize> }
            let mut rows: BTreeMap<String, Row> = BTreeMap::new();
            for l in (0..=30u32).filter(|l| level.is_none_or(|x| x == *l) && pak.contains(&format!("graph/levels/level{l}/level{l}.dlf"))) {
                let Ok(dlf) = pak.load_dlf(l) else { continue };
                let Ok(fts) = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{l}/fast.fts"))?) else { continue };
                let mut s = arx_level::Scripts::build(&pak, &dlf, glam::Vec3::from(fts.scene_pos));
                s.host.take_sounds(); s.host.take_messages(); s.host.take_speech();
                for id in 0..s.world.entities.len() as u32 {
                    let e = s.world.entity(id);
                    if e.kind != EntityKind::Fix || e.script.is_none() { continue; }
                    let class = e.class.rsplit('/').next().unwrap_or(&e.class).to_owned();
                    let before = s.host.state(id).cloned().unwrap_or_default();
                    let vars_before = format!("{:?}", s.world.entity(id).vars);
                    let unknown_before = s.world.stats.unknown_commands.clone();
                    s.world.send_event(&mut s.host, Some(s.player), id, "action", vec![]);
                    s.world.update(&mut s.host, 4000.0);
                    let after = s.host.state(id).cloned().unwrap_or_default();
                    let mut effects = Vec::new();
                    if after.anim_serial != before.anim_serial { effects.push("animation"); }
                    if !s.host.take_sounds().is_empty() { effects.push("sound"); }
                    if !s.host.take_messages().is_empty() || !s.host.take_speech().is_empty() { effects.push("text"); }
                    if (after.hidden, after.destroyed, after.collision, after.interactive) != (before.hidden, before.destroyed, before.collision, before.interactive) { effects.push("state"); }
                    if format!("{:?}", s.world.entity(id).vars) != vars_before { effects.push("variables"); }
                    let row = rows.entry(class).or_default();
                    row.tried += 1;
                    if row.example.is_empty() { row.example = s.world.entity(id).id_string.clone(); }
                    if !effects.is_empty() { row.reacted += 1; }
                    for f in &effects { *row.effects.entry(f).or_default() += 1; }
                    for (k, n) in &s.world.stats.unknown_commands {
                        if n > unknown_before.get(k).unwrap_or(&0) { *row.unknown.entry(k.clone()).or_default() += 1; }
                    }
                }
            }
            println!("{:<34} {:>5} {:>7}  effects / unimplemented commands hit by `action`", "class", "tried", "reacted");
            for (class, r) in &rows {
                let eff: Vec<String> = r.effects.iter().map(|(k, n)| format!("{k}x{n}")).collect();
                let unk: Vec<String> = r.unknown.iter().map(|(k, n)| format!("{k}x{n}")).collect();
                let flag = if r.reacted == 0 { "  <-- nothing observable" } else { "" };
                println!("{class:<34} {:>5} {:>7}  {} {}{flag}", r.tried, r.reacted, eff.join(","), if unk.is_empty() { String::new() } else { format!("[unimplemented: {}]", unk.join(",")) });
            }
        }
        Cmd::PlayerSpeeds => {
            let anims = [
                ("run (default forward)", "player_normal_run_test"), ("walk (stealth forward)", "human_normal_walk"),
                ("run backward", "player_normal_run_backward_test"), ("walk backward", "human_normal_walk_backward"),
                ("strafe run right", "player_normal_strafe_run_right"), ("strafe run left", "player_normal_strafe_run_left"),
                ("strafe right (stealth)", "human_normal_strafe_right"),
                ("crouch walk", "human_normal_crouch_walk_forward"), ("crouch walk backward", "human_normal_crouch_walk_backward"),
                ("crouch strafe right", "human_normal_crouch_strafe_right"),
                ("crouch in", "human_normal_crouch_in"), ("crouch out", "human_normal_crouch_out"),
                ("jump anticipation", "human_normal_jump_part1_anticipation"), ("jump up", "human_normal_jump_part2_jumpup"),
            ];
            println!("{:<26} {:>9} {:>9} {:>11} {:>14}", "animation", "root move", "time ms", "scale /ms", "steady u/s");
            for (label, file) in anims {
                let path = format!("graph/obj3d/anims/npc/{file}.tea");
                let Ok(bytes) = pak.read(&path) else { println!("{label:<26} missing {path}"); continue };
                let t = arx_formats::tea::Tea::parse(&bytes)?;
                let mv = t.frames.last().map_or(0.0, |f| f.translate.length());
                let ms = t.duration_us as f32 / 1000.0;
                let scale = mv / ms * 0.0125;
                println!("{label:<26} {mv:>9.1} {ms:>9.0} {scale:>11.5} {:>14.1}", scale / 0.009 * 1000.0);
            }
        }
        Cmd::Walk { level, no_entities, no_jump } => {
            use arx_physics::{CollisionWorld, MoveInput, Player, EYE_HEIGHT};
            let fts = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{level}/fast.fts"))?)?;
            let t = std::time::Instant::now();
            let mut world = CollisionWorld::from_fts(&fts);
            let mut scripts = None;
            if !no_entities {
                if let Ok(dlf) = pak.load_dlf(level) {
                    let scene_pos = glam::Vec3::from(fts.scene_pos);
                    let s = arx_level::Scripts::build(&pak, &dlf, scene_pos);
                    let obstacles = arx_level::EntityObstacles::build(&mut world, &pak, &dlf, scene_pos, &s.world, &s.host, &s.ids);
                    let enabled = obstacles.by_entity.values().filter(|o| world.obstacle_enabled(**o)).count();
                    println!("entity obstacles: {} ({} solid right now), {} character cylinders", obstacles.by_entity.len(), enabled, obstacles.characters.len());
                    let (mut rs, mut hs): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
                    for c in obstacles.characters.values().filter_map(|c| world.cylinder(*c)) { rs.push(c.radius); hs.push(c.height); }
                    if !rs.is_empty() {
                        let (rmin, rmax) = rs.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
                        let (hmin, hmax) = hs.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
                        println!("  character cylinders: radius {rmin:.0}..{rmax:.0}, height {hmin:.0}..{hmax:.0}");
                    }
                    scripts = Some((s, obstacles));
                }
            }
            let _ = &scripts;
            println!("collision world: {} triangles, built in {:.0?}", world.triangle_count(), t.elapsed());
            let mut start = glam::Vec3::new(fts.player_pos[0], -fts.player_pos[1], -fts.player_pos[2]);
            // The saved start is an editor position and can be on a ledge nowhere near the real floor;
            // entities stand on real floors, so start from the one nearest to it.
            if let Ok(dlf) = pak.load_dlf(level) {
                let sp = glam::Vec3::from(fts.scene_pos);
                let nearest = dlf.entities.iter()
                    .filter(|e| !e.class.contains("/items/") && !e.class.contains("/system/camera"))
                    .map(|e| arx_level::to_yup([e.pos[0] + sp.x, e.pos[1] + sp.y, e.pos[2] + sp.z]))
                    .min_by(|a, b| (a.x - start.x).hypot(a.z - start.z).total_cmp(&(b.x - start.x).hypot(b.z - start.z)));
                if let Some(p) = nearest { start = p; }
            }
            let mut p = Player::new(start + glam::Vec3::Y * 40.0);
            for _ in 0..300 { p.step(&world, 1.0 / 60.0, MoveInput::default()); }
            println!("dropped from 40 above the start: feet y {:.1} (start y {:.1}), on_ground {}, eye {:.1}", p.feet.y, start.y, p.on_ground, p.feet.y + EYE_HEIGHT);
            if !p.on_ground {
                // The saved start can be over nothing; use the centre of the largest upward-facing solid polygon instead.
                use arx_formats::poly::*;
                if let Some(best) = fts.polys.iter().filter(|q| q.flags & (WATER | TRANS | NOCOL) == 0 && q.norm[1] < -0.9).max_by(|a, b| a.area.total_cmp(&b.area)) {
                    let vs = &best.verts[..best.vertex_count()];
                    let n = vs.len() as f32;
                    let c = glam::Vec3::new(vs.iter().map(|v| v.pos[0]).sum::<f32>() / n, -vs.iter().map(|v| v.pos[1]).sum::<f32>() / n, -vs.iter().map(|v| v.pos[2]).sum::<f32>() / n);
                    p = Player::new(c + glam::Vec3::Y * 40.0);
                    for _ in 0..300 { p.step(&world, 1.0 / 60.0, MoveInput::default()); }
                    println!("start was over nothing; relocated to the largest floor polygon: feet y {:.1}, on_ground {}", p.feet.y, p.on_ground);
                }
            }
            let settled = p;
            for deg in (0..360).step_by(45) {
                let mut q = settled;
                let dir = glam::Vec2::from_angle((deg as f32).to_radians());
                let mut min_y = q.feet.y; let mut max_y = q.feet.y;
                for _ in 0..240 { q.step(&world, 1.0 / 60.0, MoveInput::toward(dir)); min_y = min_y.min(q.feet.y); max_y = max_y.max(q.feet.y); }
                let moved = (q.feet - settled.feet).truncate();
                println!("run {deg:>3} deg for 4s (about 1000 units free): moved {:>6.0}, height change {:+.0}..{:+.0}, on_ground {}", glam::Vec2::new(q.feet.x - settled.feet.x, q.feet.z - settled.feet.z).length(), min_y - settled.feet.y, max_y - settled.feet.y, q.on_ground);
                let _ = moved;
            }
            // Fuzz: wander randomly (with jumps) for a few minutes of game time and make sure the
            // player never drops out of the level or gets stuck falling.
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            let (mut grounded, mut frames, mut worst_fall) = (0usize, 0usize, 0usize);
            let mut fall_run = 0usize; let mut last_ground = settled.feet;
            let mut q = settled;
            let mut seed = 12345u32;
            let mut rnd = || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 8) as f32 / (1u32 << 24) as f32 };
            let mut heading = 0.0f32;
            for f in 0..60 * 180 {
                if f % 45 == 0 { heading += (rnd() - 0.5) * 3.0; }
                let sneak = rnd() < 0.3;
                q.step(&world, 1.0 / 60.0, MoveInput { stealth: sneak, jump: !no_jump && f % 200 == 0, ..MoveInput::toward(glam::Vec2::from_angle(heading)) });
                lo = lo.min(q.feet.y); hi = hi.max(q.feet.y);
                frames += 1; grounded += q.on_ground as usize;
                if q.on_ground { fall_run = 0; last_ground = q.feet } else { fall_run += 1; worst_fall = worst_fall.max(fall_run); if [1, 3, 6, 12, 30].contains(&fall_run) { println!("    airborne +{fall_run}: pos ({:.0}, {:.0}, {:.0}) vel_y {:.0}; floor anywhere below here: {:?}", q.feet.x, q.feet.y, q.feet.z, q.vel_y, world.floor_height(q.feet.x, q.feet.z, q.feet.y + 45.0)); }
                    if fall_run == 120 { println!("  fell off at frame {f}: last ground {:?}, heading {heading:.2}", last_ground);
                    // What does the level contain under/around the last ground position? (Arx coords: x, -y, -z)
                    let (gx, gz) = (last_ground.x, -last_ground.z);
                    let mut near: Vec<(f32, f32, u32, i32)> = fts.polys.iter().filter_map(|p| {
                        let vs = &p.verts[..p.vertex_count()];
                        let (x0, x1) = vs.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.pos[0]), b.max(v.pos[0])));
                        let (z0, z1) = vs.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.pos[2]), b.max(v.pos[2])));
                        if gx >= x0 - 60.0 && gx <= x1 + 60.0 && gz >= z0 - 60.0 && gz <= z1 + 60.0 {
                            let (y0, y1) = vs.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(-v.pos[1]), b.max(-v.pos[1])));
                            Some((y0, y1, p.flags, p.tex))
                        } else { None }
                    }).collect();
                    let mut cats = std::collections::BTreeMap::new();
                    for q in &fts.polys {
                        let vs = &q.verts[..q.vertex_count()];
                        let cx = vs.iter().map(|v| v.pos[0]).sum::<f32>() / vs.len() as f32;
                        let cz = vs.iter().map(|v| v.pos[2]).sum::<f32>() / vs.len() as f32;
                        let cy = -vs.iter().map(|v| v.pos[1]).sum::<f32>() / vs.len() as f32;
                        if (cx - gx).abs() < 250.0 && (cz - gz).abs() < 250.0 && cy < last_ground.y + 100.0 {
                            use arx_formats::poly::*;
                            let kind = if q.flags & WATER != 0 { "water" } else if q.flags & TRANS != 0 { "trans" } else if q.flags & NOCOL != 0 { "nocol" } else { "solid" };
                            *cats.entry((kind, if cy > last_ground.y - 60.0 { "near-floor" } else { "below" })).or_insert(0) += 1;
                        }
                    }
                    println!("    polys within 250 horizontally, at/below floor level: {cats:?}");
                    near.sort_by(|a, b| b.0.total_cmp(&a.0));
                    for (y0, y1, fl, tex) in near.iter().filter(|n| n.1 > last_ground.y - 40.0 && n.0 < last_ground.y + 200.0).take(40) { println!("    poly y {y0:.0}..{y1:.0} flags {fl:#x} tex {}", if *tex == 0 { "none" } else { "yes" }); }
                } }
            }
            {
                use arx_formats::poly::*;
                let mut area = std::collections::BTreeMap::<&str, f32>::new();
                for q in fts.polys.iter().filter(|q| q.norm[1].abs() > 0.9) {
                    let kind = if q.flags & WATER != 0 { "water" } else if q.flags & TRANS != 0 { "trans" } else if q.flags & NOCOL != 0 { "nocol" } else { "solid" };
                    *area.entry(kind).or_default() += q.area;
                }
                let total: f32 = area.values().sum();
                let pct: Vec<String> = area.iter().map(|(k, v)| format!("{k} {:.0}%", 100.0 * v / total)).collect();
                println!("flat floor/ceiling area by kind: {}", pct.join(", "));
            }
            let (floor_lo, floor_hi) = world_extent_y(&fts);
            println!("fuzz 3 min: {} rescues from falling out of the level; feet y {lo:.0}..{hi:.0} (level geometry spans {floor_lo:.0}..{floor_hi:.0}), on ground {:.0}% of frames, longest airborne run {} frames, final pos ({:.0}, {:.0}, {:.0})", q.rescues, 100.0 * grounded as f32 / frames as f32, worst_fall, q.feet.x, q.feet.y, q.feet.z);
        }
        Cmd::Audio { path: Some(path), out } => {
            let pcm = arx_formats::wav::decode(&pak.read(&path)?)?;
            println!("{path}: {} Hz, {} ch, {:.2}s", pcm.rate, pcm.channels, pcm.duration_secs());
            if let Some(out) = out { std::fs::write(&out, pcm.to_wav_bytes())?; println!("wrote {}", out.display()); }
        }
        Cmd::Audio { path: None, .. } => {
            let (mut ok, mut bad, mut secs) = (0, 0, 0.0f64);
            let mut by_dir: BTreeMap<String, usize> = BTreeMap::new();
            for name in pak.list("").into_iter().filter(|n| n.ends_with(".wav")) {
                match arx_formats::wav::decode(&pak.read(name)?) {
                    Ok(p) => { ok += 1; secs += p.duration_secs() as f64; *by_dir.entry(name.split('/').take(2).collect::<Vec<_>>().join("/")).or_default() += 1; }
                    Err(e) => { bad += 1; if bad <= 10 { eprintln!("FAIL {name}: {e}"); } }
                }
            }
            println!("{ok} decoded ({:.1} hours of audio), {bad} failed", secs / 3600.0);
            println!("by directory: {by_dir:?}");
        }
        Cmd::Script { level, top, details } => {
            use arx_script::{Script, ScriptWorld, EntityKind};
            let mut total = arx_script::Stats::default();
            let (mut ents, mut with_script, mut with_over) = (0, 0, 0);
            for l in (0..=30u32).filter(|l| level.is_none_or(|x| x == *l) && pak.contains(&format!("graph/levels/level{l}/level{l}.dlf"))) {
                let dlf = match pak.load_dlf(l) { Ok(d) => d, Err(e) => { println!("level {l}: {e}"); continue } };
                let mut world = ScriptWorld::new();
                let load = |path: &str| pak.read(path).ok().map(|b| std::sync::Arc::new(Script::new(&b)));
                for e in &dlf.entities {
                    let (dir, name) = e.class.rsplit_once('/').unwrap_or(("", &e.class));
                    let class_script = load(&format!("{}.asl", e.class));
                    let over = load(&format!("{dir}/{name}_{:04}/{name}.asl", e.instance));
                    ents += 1; with_script += class_script.is_some() as usize; with_over += over.is_some() as usize;
                    world.add_entity(EntityKind::from_class(&e.class), &e.class, e.instance, class_script, over);
                }
                let t = std::time::Instant::now();
                let mut host = arx_script::StdHost::new();
                let n = world.entities.len() as u32;
                for id in 0..n { world.send_event(&mut host, None, id, "load", vec![]); }
                for id in 0..n { world.send_init(&mut host, id); }
                for id in 0..n { world.send_event(&mut host, None, id, "game_ready", vec![]); }
                world.update(&mut host, 0.0);
                if level.is_some() && details {
                    for id in 0..n {
                        let e = world.entity(id);
                        if let Some(st) = host.state(id) {
                            let mut notes = Vec::new();
                            if st.destroyed { notes.push("DESTROYED".to_string()); }
                            if st.hidden { notes.push("hidden".to_string()); }
                            if let Some(m) = &st.mesh { notes.push(format!("mesh={m}")); }
                            if (st.scale - 1.0).abs() > 1e-6 { notes.push(format!("scale={}", st.scale)); }
                            if !st.interactive { notes.push("not-interactive".to_string()); }
                            if !notes.is_empty() { println!("    {:<34} {}", e.id_string, notes.join(" ")); }
                        }
                    }
                }
                let s = &world.stats;
                println!("level {l}: {} entities, init+initend: {} events, {} commands, {} runaway, {} warning kinds, {} unknown command kinds [{:.0?}]", world.entities.len(), s.events_run, s.commands_run, s.aborted_runaway, s.warnings.len(), s.unknown_commands.len(), t.elapsed());
                total.events_run += s.events_run; total.commands_run += s.commands_run; total.aborted_runaway += s.aborted_runaway;
                for (k, v) in &s.unknown_commands { *total.unknown_commands.entry(k.clone()).or_default() += v; }
                for (k, v) in &s.warnings { *total.warnings.entry(k.clone()).or_default() += v; }
            }
            println!("
TOTAL: {ents} entities ({with_script} class scripts, {with_over} instance scripts), {} events, {} commands, {} runaway", total.events_run, total.commands_run, total.aborted_runaway);
            let mut u: Vec<_> = total.unknown_commands.iter().collect(); u.sort_by(|a, b| b.1.cmp(a.1));
            println!("unknown commands (not part of the language core): {}", u.iter().take(top).map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(" "));
            let mut w: Vec<_> = total.warnings.iter().collect(); w.sort_by(|a, b| b.1.cmp(a.1));
            println!("warnings:"); for (k, v) in w.iter().take(top) { println!("  {v:>5}  {k}"); }
        }
        Cmd::Tea { path: None, .. } => {
            let (mut ok, mut bad) = (0, 0);
            let mut groups: BTreeMap<usize, usize> = BTreeMap::new();
            for name in pak.list("").into_iter().filter(|n| n.ends_with(".tea")) {
                match arx_formats::tea::Tea::parse(&pak.read(name)?) {
                    Ok(t) => { ok += 1; *groups.entry(t.group_count).or_default() += 1; }
                    Err(e) => { bad += 1; eprintln!("FAIL {name}: {e}"); }
                }
            }
            println!("{ok} parsed, {bad} failed; animations by bone count: {groups:?}");
        }
        Cmd::Tea { path: Some(path), model } => {
            let t = arx_formats::tea::Tea::parse(&pak.read(&path)?)?;
            println!("{:?}: {} keyframes, {} groups ({} static), {:.2}s", t.name, t.frames.len(), t.group_count,
                t.void_groups.iter().filter(|v| **v).count(), t.duration_us as f64 / 1e6);
            if let Some(model) = model {
                let ftl = arx_formats::ftl::Ftl::parse(&pak.read(&model)?)?;
                let sk = arx_formats::skeleton::Skeleton::from_ftl(&ftl);
                println!("model: {} vertices, {} bones; animation drives {} groups", ftl.vertices.len(), sk.bones.len(), t.group_count);
                let bind: Vec<glam::Vec3> = ftl.vertices.iter().map(|v| glam::Vec3::from(v.pos)).collect();
                let bounds = |p: &[glam::Vec3]| p.iter().fold((glam::Vec3::MAX, glam::Vec3::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
                println!("bind  bounds {:?}", bounds(&bind));
                let rest = sk.pose(&arx_formats::tea::Tea { frames: vec![t.frames[0].clone()], groups: vec![], group_count: 0, void_groups: vec![], ..t.clone() }, 0);
                let err = rest.iter().zip(&bind).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
                println!("rest pose reproduces bind pose within {err:.4}");
                for i in 0..=4 {
                    let tm = t.duration_us * i / 4;
                    let p = sk.pose(&t, t.looped_time(tm));
                    let b = bounds(&p);
                    let bad = p.iter().filter(|v| !v.is_finite()).count();
                    println!("t={:.2}s bounds {:?} ({bad} non-finite)", tm as f64 / 1e6, b);
                }
            }
        }
        Cmd::Dlf { level, entities } => {
            let levels: Vec<u32> = (0..=30).filter(|l| level.is_none_or(|x| x == *l) && pak.contains(&format!("graph/levels/level{l}/level{l}.dlf"))).collect();
            for l in levels {
                let d = match pak.load_dlf(l) { Ok(d) => d, Err(e) => { println!("level {l}: FAILED {e}"); continue; } };
                let fts = pak.read(&format!("game/graph/levels/level{l}/fast.fts")).ok().and_then(|b| arx_formats::fts::Fts::parse(&b).ok());
                let with_model = d.entities.iter().filter(|e| pak.contains(&format!("game/{}.ftl", e.class))).count();
                let zones = d.paths.iter().filter(|p| p.is_zone()).count();
                println!("level {l}: v{} {} entities ({with_model} with model), {} lights, {} fogs, {} paths + {zones} zones, trailing {} bytes, scene {:?}, start {:?} (fts start {:?}, scene_pos {:?})",
                    d.version, d.entities.len(), d.lights.len(), d.fogs.len(), d.paths.len() - zones, d.trailing, d.scene,
                    d.player_pos, fts.as_ref().map(|f| f.player_pos), fts.as_ref().map(|f| f.scene_pos));
                if entities {
                    for e in &d.entities { println!("  {:<70} #{:<4} pos {:?} angle {:?} flags {}", e.class, e.instance, e.pos, e.angle, e.flags); }
                }
            }
        }
        Cmd::Ftl { path: None } => {
            // How many models are not centred on their origin vertex (the engine shifts these)?
            let mut off = Vec::new();
            let mut total = 0;
            for name in pak.list("").into_iter().filter(|n| n.ends_with(".ftl")) {
                if let Ok(m) = arx_formats::ftl::Ftl::parse(&pak.read(name)?) {
                    total += 1;
                    let o = m.vertices[m.origin as usize].pos;
                    if o != [0.0; 3] { off.push((name.to_string(), o)); }
                }
            }
            println!("{} of {total} models have a non-zero origin vertex", off.len());
            for (n, o) in off.iter().take(12) { println!("  {n}: origin vertex at {o:?}"); }
            let (mut ok, mut bad, mut missing_tex) = (0, 0, 0);
            let mut missing = BTreeMap::<String, usize>::new();
            for name in pak.list("").into_iter().filter(|n| n.ends_with(".ftl")) {
                match arx_formats::ftl::Ftl::parse(&pak.read(name)?) {
                    Ok(m) => {
                        ok += 1;
                        for t in m.textures.iter().filter(|t| !t.is_empty()) {
                            if pak.find_texture(t).is_none() { missing_tex += 1; *missing.entry(t.clone()).or_default() += 1; }
                        }
                    }
                    Err(e) => { bad += 1; eprintln!("FAIL {name}: {e}"); }
                }
            }
            println!("{ok} parsed, {bad} failed, {missing_tex} unresolved texture references");
            for (t, n) in missing.iter().take(25) { println!("  missing: {t} (x{n})"); }
        }
    }
    Ok(())
}

fn world_extent_y(fts: &arx_formats::fts::Fts) -> (f32, f32) {
    fts.polys.iter().flat_map(|p| p.verts[..p.vertex_count()].iter().map(|v| -v.pos[1])).fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)))
}
