# AGENTS.md

Guidance for AI coding agents working in this repository. Read this before changing anything; most of it
records things that were expensive to find out.

## What this is

A Rust reimplementation of the 2002 RPG *Arx Fatalis* on the Bevy engine (0.19, Rust edition 2024). It
loads the **original game assets** from the user's own installation (default
`D:\Steam\steamapps\common\Arx Fatalis`, override with `--game-dir` or `ARX_DIR`). It is not a
static recompilation of `arx.exe` and not a mechanical C++ translation.

Format and behaviour knowledge comes from the GPLv3 project **ArxLibertatis**
(<https://github.com/arx/ArxLibertatis>). Use it as the reference: when behaviour is unclear, read its source
(`git clone --depth 1` it somewhere outside this repo) instead of guessing. This project is GPL-3.0-or-later.

## Rules that must not be broken

- **Never commit game data** (PAKs, extracted assets, screenshots of them). `shots/` and `target/` are ignored.
- Do not copy ArxLibertatis/Arkane file headers or code verbatim; port the logic and write it idiomatically.
- Keep `arx-formats`, `arx-physics`, `arx-script` and `arx-level` **free of Bevy**. Only `arx-viewer` may use it.
- Do not push, force-push or rewrite history without being asked.
- Verify claims by measuring against the real game data. A plausible-looking render is not proof (see "Lessons").

## Crates

| crate | role |
|---|---|
| `arx-formats` | PAK archives + PKWare DCL decompression, `.ftl` models, `.fts` level geometry, `.llf` baked lighting, `.dlf` scenes, `.tea` animations + skeletons, `.wav` (MS-ADPCM/PCM), localisation text (`locale`) |
| `arx-physics` | level collision (spatial hash), switchable entity obstacles (doors), first-person player body |
| `arx-script` | `.asl` interpreter (events, variables, goto/gosub, timers, event queue) and `StdHost` for visual/sound commands |
| `arx-level` | glue: runs a level's scripts, builds entity obstacles, `entity_rotation` |
| `arx-cli` (`arx`) | inspection/verification tools (see below) |
| `arx-viewer` | Bevy app: model/texture browser and the walkable level viewer |

Dependency direction: `formats` <- `physics`, `script` <- `level` <- `cli`, `viewer`.

## Build, test, run

```bash
cargo test --workspace                                  # all unit tests (no game data needed)
cargo run --release -p arx-cli -- verify                # reads every file of every PAK (needs the game)
cargo run --release -p arx-viewer -- level 1            # play (levels 0-8, 10-23)
```

The first build of `arx-viewer` compiles Bevy and takes many minutes; do not time out on it. Debug builds are
configured with optimised dependencies (`[profile.dev.package."*"] opt-level = 3`).

## Coordinates and conventions (verified)

- Arx is +Y down, +Z forward, right-handed (`x × y = z`). Bevy/physics are +Y up, -Z forward. Conversion is
  `(x, y, z) -> (x, -y, -z)` (a 180° rotation about X), so **triangle winding is preserved**. Use
  `arx_level::to_yup` / `arx-viewer`'s `to_bevy`.
- **Entity rotation**: the engine remaps the stored yaw before drawing: objects use `270 - yaw`, NPCs
  `180 - yaw` (`scene/Interactive.cpp`, `UpdateInter`/`RenderInter`). Objects: Arx `Rz(-roll)·Rx(pitch)·Ry(yaw')`.
  NPCs use `QuatFromAngles`. Implemented in `arx_level::entity_rotation(angle, is_npc)`. Using the raw yaw was a
  real bug that left doors and portcullises a quarter/half turn off.
- Entity and light positions in `.dlf`/`.llf` are relative to the scene origin: add `Fts::scene_pos`.
- The editor start positions (`.fts` player pos, `.dlf` `pos_edit`) can be on a ledge far from the real floor.
- Level start order: `load` → `init` + `initend` per entity → `game_ready`. Scripts hide/destroy/scale/swap-mesh
  entities at start, so use the scripted state, never the raw `.dlf`.
- Textures: strip extension, try `.png .jpg .jpeg .bmp .tga`; `.bmp` uses black as a colour key.
- Level lighting is baked per vertex (`.llf`) and multiplies the texture; objects are lit per vertex from the
  10 nearest static lights plus ambient (see `arx-viewer/src/entities.rs`).
- Script text is Latin-1, lowercased on load; `§` (0xA7) and `£` (0xA3) mark local int/text variables;
  `(` and `)` are whitespace; events are found by an exact `on <name>` search.
- Collision: decide which side of a wall to push the player to from the **previous** position, never from the
  stored polygon normal; ground is the nearest surface below ignoring orientation; solid entity surfaces are
  ground too; only a grounded player steps over low obstacles.

More detail on each format is in the doc comments at the top of the modules in `arx-formats/src/`.

## Debugging and verification tools

```bash
cargo run --release -p arx-cli -- ls graph/levels          # list the virtual file system
cargo run --release -p arx-cli -- script 1 -d              # run a level's scripts; list what they did to entities
cargo run --release -p arx-cli -- walk 11                  # headless collision test: drop, 8 walks, 3-minute fuzz
cargo run --release -p arx-cli -- polys-at 11 8144 7467    # level polygons covering an Arx (x, z) point
cargo run --release -p arx-cli -- orient-panel             # door/portcullis placement vs. level geometry
cargo run --release -p arx-cli -- orient-wall lever        # wall-mounted objects: back to wall vs. facing into it
cargo run --release -p arx-cli -- audio                    # decode every game sound
cargo run --release -p arx-cli -- locale                   # do entity names / speak keys resolve to text and voices?
```

Viewer helpers (all work headless; `--shot out.png` saves a screenshot then exits and **ignores real input**):

```bash
cargo run -p arx-viewer -- level 1 --focus light_door_0075 --shot shots/a.png   # camera in front of an entity (:back, :side)
cargo run -p arx-viewer -- level 1 --use-entity light_door_0074:open --shot shots/b.png   # send an event, then look
cargo run -p arx-viewer -- level 1 --cam 8650,2945,8550 --look 0,-89 --fly --shot shots/c.png   # Arx coords, yaw,pitch
```

```bash
cargo run -p arx-viewer -- level 1 --focus goblin_base_0051 --say goblin_base_0051:goblinlord_forbidden --mute --shot shots/d.png   # subtitle test
```

Environment: `ARX_FULLBRIGHT=1` ignores baked lighting (dark levels hide misalignment), `ARX_SHOT_FRAME=n`
sets the screenshot frame (large levels need ~60 frames before everything appears), `ARX_LOG_SOUND=1` logs sounds, `ARX_LOG_SPEECH=1` logs dialogue and `herosay` messages.

## Lessons (do not repeat these)

- **Test with asymmetric things.** A flat door looks identical under a ±yaw or 180° error; wall-mounted levers and
  portcullis-in-corridor placement exposed the rotation bug that door checks missed.
- Rendering that "looks right" at one spot is weak evidence; add a numeric check against level geometry.
- Bevy 0.19 API: `GlobalAmbientLight` is the resource (`AmbientLight` is a component);
  `Image::new` asserts the data length, use `new_uninit` + set `data`/`mip_level_count` for mip chains;
  `compute_flat_normals` needs non-indexed meshes; `TextFont.font_size` is `FontSize::Px(..)`.
- Windows path length: extracting many scripts to a deep folder needs the `\\?\` prefix when read from Python.
- `arx` panics with "failed printing to stdout" when its output is piped into `head` and the pipe closes early; this is
  harmless (it uses `println!`), not a bug in whatever you were checking.
- Text: `localisation/utext_<language>.ini` is UTF-16. Keys are case- and bracket-insensitive (`[description_door]`);
  `String`, `String2`, ... are variants and each has its own voice file `speech/<language>/<key>[N].wav` (no number for
  the first). `speak` options form a single flag word (`-to`, not `-t -o`); the rest of the line runs when the speech
  ends (`ScriptWorld::run_line`). `setname` stores a key, so show `locale.text_or_key(name)`.
- Level 9 does not exist. Many levels' saved start is not on a real floor; the viewer starts at the nearest entity
  when the saved start is more than 250 units off its floor.

## Current state and known gaps

Done: PAK/FTL/FTS/LLF/DLF/TEA/WAV readers, level rendering with baked lighting, placed entities with per-vertex
lighting, skeletal animation (CPU skinning), walkable player with collision, script interpreter running every
entity's start-up (all 23 levels), doors/levers/portcullises (animation, collision, sound), spatial sound,
localised names, voiced dialogue with subtitles (`speak`, `playspeech`) and `herosay` notifications.

Missing: cinematic cameras for `speak -c`, NPC behaviour (`behavior`, `settarget`), pickup/inventory, combat, spells,
player HUD (health/mana/inventory), footsteps/music/ambiance zones, fog and dynamic/flickering lights, menus and save games. About 60 script
commands are skipped (the interpreter ignores a command it does not know, line by line, and counts it in
`Stats::unknown_commands`; `arx script` prints the most frequent ones). A few levels (3, 10, 19, 20) have genuine
drops where the player falls out of the world and is put back on their last solid ground.

When you finish a change, say how to run it (the viewer command that shows the change).
