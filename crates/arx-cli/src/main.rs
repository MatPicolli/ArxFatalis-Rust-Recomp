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
    /// Simulate the player in a level: drop from the start, then walk in 8 directions and report how
    /// far each walk got before hitting a wall (a headless check of the collision system)
    Walk { level: u32 },
    /// Parse a .tea animation (and with --model, pose that .ftl at several times); no path = parse all
    Tea { path: Option<String>, #[arg(long)] model: Option<String> },
    /// Parse level scene definitions (`graph/levels/levelN/levelN.dlf`); no argument = all levels
    Dlf { level: Option<u32>, /// print every entity
        #[arg(short, long)] entities: bool },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let pak = PakSet::open_game_dir(&cli.game_dir)
        .with_context(|| format!("opening {}", cli.game_dir.display()))?;

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
        Cmd::Walk { level } => {
            use arx_physics::{CollisionWorld, Player, EYE_HEIGHT};
            let fts = arx_formats::fts::Fts::parse(&pak.read(&format!("game/graph/levels/level{level}/fast.fts"))?)?;
            let t = std::time::Instant::now();
            let world = CollisionWorld::from_fts(&fts);
            println!("collision world: {} triangles, built in {:.0?}", world.triangle_count(), t.elapsed());
            let start = glam::Vec3::new(fts.player_pos[0], -fts.player_pos[1], -fts.player_pos[2]);
            let mut p = Player::new(start + glam::Vec3::Y * 40.0);
            for _ in 0..300 { p.step(&world, 1.0 / 60.0, glam::Vec2::ZERO, false); }
            println!("dropped from 40 above the start: feet y {:.1} (start y {:.1}), on_ground {}, eye {:.1}", p.feet.y, start.y, p.on_ground, p.feet.y + EYE_HEIGHT);
            if !p.on_ground {
                // The saved start can be over nothing; use the centre of the largest upward-facing solid polygon instead.
                use arx_formats::poly::*;
                if let Some(best) = fts.polys.iter().filter(|q| q.flags & (WATER | TRANS | NOCOL) == 0 && q.norm[1] < -0.9).max_by(|a, b| a.area.total_cmp(&b.area)) {
                    let vs = &best.verts[..best.vertex_count()];
                    let n = vs.len() as f32;
                    let c = glam::Vec3::new(vs.iter().map(|v| v.pos[0]).sum::<f32>() / n, -vs.iter().map(|v| v.pos[1]).sum::<f32>() / n, -vs.iter().map(|v| v.pos[2]).sum::<f32>() / n);
                    p = Player::new(c + glam::Vec3::Y * 40.0);
                    for _ in 0..300 { p.step(&world, 1.0 / 60.0, glam::Vec2::ZERO, false); }
                    println!("start was over nothing; relocated to the largest floor polygon: feet y {:.1}, on_ground {}", p.feet.y, p.on_ground);
                }
            }
            let settled = p;
            for deg in (0..360).step_by(45) {
                let mut q = settled;
                let dir = glam::Vec2::from_angle((deg as f32).to_radians());
                let mut min_y = q.feet.y; let mut max_y = q.feet.y;
                for _ in 0..240 { q.step(&world, 1.0 / 60.0, dir * 300.0, false); min_y = min_y.min(q.feet.y); max_y = max_y.max(q.feet.y); }
                let moved = (q.feet - settled.feet).truncate();
                println!("walk {deg:>3} deg for 4s at 300/s (1200 units free): moved {:>6.0}, height change {:+.0}..{:+.0}, on_ground {}", glam::Vec2::new(q.feet.x - settled.feet.x, q.feet.z - settled.feet.z).length(), min_y - settled.feet.y, max_y - settled.feet.y, q.on_ground);
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
                let speed = if rnd() < 0.7 { 300.0 } else { 600.0 };
                q.step(&world, 1.0 / 60.0, glam::Vec2::from_angle(heading) * speed, f % 200 == 0);
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
