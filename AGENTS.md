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
| `arx-physics` | level collision (spatial hash), switchable entity obstacles (doors), NPC cylinders, first-person player body ported from the engine (movement, jump, crouch, ceilings, fall damage) |
| `arx-script` | `.asl` interpreter (events, variables, goto/gosub, timers, event queue) and `StdHost` for visual/sound commands |
| `arx-level` | glue: runs a level's scripts, builds entity obstacles, `entity_rotation`, `inventory` (pick up / use / combine / drop), `anchors` (the path-finding graph), `npc` (character AI + `npc/combat.rs`: damage formula, hits, deaths, characters' blows) and `player_combat` (the hero's draw / wind-up / strike stages) |
| `arx-cli` (`arx`) | inspection/verification tools (see below) |
| `arx-viewer` | Bevy app: model/texture browser and the walkable level viewer; `hud.rs` is the interface state and keys, `hud_ui.rs` draws the original HUD, `menu.rs` the main/pause menu, options and character creation |

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
- **Player movement is a port of `PlayerMovementIterate`** (`arx-physics`): keys push the body with a force of
  `animation root speed * 0.0125` per ms, horizontal velocity is damped by `0.009` per ms (so it settles at
  push / 0.009: running 266.7 units/s, sneaking 188.3, crouched 100; measure with `arx player-speeds`), the engine
  uses the *default forward animation = run* and Shift = sneak. A jump rises 130 units in 200 ms with no gravity, then
  falls with `JUMP_GRAVITY` (600 units/s^2; ordinary gravity is 3000 until a fall exceeds 450 units/s, then it also
  becomes 600), a fall of more than 400 units costs `(height - 400) / 15` life and landing from a fall stops the
  slide. After landing the push is halved for 300 ms and then the engine's own formula goes negative until 600 ms (a
  real quirk, kept). In the air the push is 7.9 forward / 0.8 back / 2.6 sideways instead of an animation's, so a
  held-forward jump carries far. Step height is 40, the cylinder 52 x 170 (120 crouched, only after the 708 ms crouch-in
  animation). The original does **not** crouch for you when walking into a low gap: you bump the edge until you crouch.
  There is no swimming: water/trans/nocol polygons are simply not solid. NPC collision cylinders come from the model
  (`arx_level::character_cylinder`).
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
cargo run --release -p arx-cli -- npc 1 40                 # run a level's characters for 40 s; -f <id> prints a character's route
cargo run --release -p arx-cli -- tweaks                   # build every model scripts changed (`tweak`); what could not be applied (-d lists them)
cargo run --release -p arx-cli -- audio                    # decode every game sound
cargo run --release -p arx-cli -- locale                   # do entity names / speak keys resolve to text and voices?
cargo run --release -p arx-cli -- player-speeds            # original movement speeds from the hero animations
cargo run --release -p arx-cli -- fixtures [level]         # send `action` to every fixture; which react, which commands are missing
cargo run --release -p arx-cli -- walk 1 --no-jump         # separates collision bugs from long jumps in the fuzz
cargo run --release -p arx-cli -- game 1 "list key" "pickup key_base_0005" "combine key_base_0005 light_door_0076" "send light_door_0076 action" "status"   # headless gameplay
```

Viewer helpers (all work headless; `--shot out.png` saves a screenshot then exits and **ignores real input**):

```bash
cargo run -p arx-viewer -- level 1 --focus light_door_0075 --shot shots/a.png   # camera in front of an entity (:back, :side)
cargo run -p arx-viewer -- level 1 --use-entity light_door_0074:open --shot shots/b.png   # send an event, then look
cargo run -p arx-viewer -- level 1 --cam 8650,2945,8550 --look 0,-89 --fly --shot shots/c.png   # Arx coords, yaw,pitch
```

```bash
cargo run -p arx-viewer -- level 1 --pickup food_fish_0006,key_base_0005 --gold 241 --show-inventory --life 5 --shot shots/e.png   # the HUD
cargo run -p arx-viewer -- level 1 --focus chest_metal_0051 --open-chest chest_metal_0051 --shot shots/f.png   # container panel
cargo run -p arx-viewer -- level 1 --focus goblin_base_0051 --say goblin_base_0051:goblinlord_forbidden --mute --shot shots/d.png   # subtitle test
```

```bash
cargo run -p arx-viewer -- level 1 --cam 9543,2945,5800 --look 180,-5 --equip short_sword_0005 --draw-weapon --attack-test --shot shots/fight.png   # hero's weapon, scripted swings
```

```bash
cargo run -p arx-viewer -- level 1 --menu options --shot shots/g.png   # a menu screen (main, options, create, quit)
cargo run -p arx-viewer -- level 1 --menu main --menu-do new,quickgen,skin,done --shot shots/h.png   # click through the menu
cargo run -p arx-viewer -- level 1 --no-cutscenes --focus goblin_base_0050 --shot shots/i.png   # scripts run, view stays yours
```

A plain `level N` run starts on the main menu; `--shot` and `--no-menu` skip it, `--new-quest` starts on character creation.

Environment: `ARX_LOG_NPC=<id text>` logs the commands scripts give those characters and their state once a second, `ARX_LOG_BODY=1` the hero's body/blow windows, `ARX_FULLBRIGHT=1` ignores baked lighting (dark levels hide misalignment), `ARX_SHOT_FRAME=n`
sets the screenshot frame (large levels need ~60 frames before everything appears), `ARX_LOG_SOUND=1` logs sounds, `ARX_LOG_SPEECH=1` logs dialogue and `herosay` messages, `ARX_LOG_FPS=1` frames per second and the milliseconds each system takes (VSync off; see `perf.rs`), `ARX_SKIP=lighting,shadows,particles` leaves systems out, `ARX_LOG_TWEAKS=1` models built from tweaks, `ARX_LOG_BODY=1` also the hero's scripted poses, `ARX_LOG_STAGE=1` cutscene state (camera, bars, fades, teleports), `ARX_LOG_ZONES=1` zone events, `ARX_LOG_ANIM=1` animations started, `ARX_LOG_LIGHTS=1` the level's lights, `ARX_PRESS_E=<frame>` (with `ARX_PRESS_E_ON=<id>`) presses `E` by itself.

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
- Items: an item entity is a *stack* (`EntityState::count`, max `stack_size` from `playerstacksize`); picking up merges
  into a carried stack of the same class. `eatme`/`destroy` remove one from a stack before destroying the entity.
  Doors and chests are unlocked by sending `combine` with `^$param1` = the key's id string (`key_base_0005`), which the
  door tests with `^$param1 isin £key`. `§` is the **int** variable prefix, `£` the **text** one (easy to swap).
  `specialfx heal N` adds N life directly. A new hero has life 12 and mana 6 (attribute 6 x (level+2) / (level+1)).
- Containers: engine clicks on a chest/corpse send `inventory2_open` (a `refuse` keeps it locked and the script
  complains), not `action`; `inventory add` item paths use doubled backslashes (`PROVISIONS\\\\GARLIC\\\\GARLIC`).
  `inventory addfromscene` moves an existing level item into the chest (the goblin outpost key is in a chest).
- **The HUD** (`arx-viewer/src/hud_ui.rs`, from `ArxLibertatis/src/gui/Hud.cpp` and `gui/hud/*`): everything is in the
  original's 640x480 pixels times `interface_scale` (`getInterfaceScale`: 0.5 of the largest scale that fits, whole or
  half steps, so 1.0 at 720p; the viewer defaults to `--hud-scale 1.0`). `geometry()` is the single source of truth for
  positions (tests pin them): health 33x80 at the bottom-left (+2px so the texture's gap is hidden), mana bottom-right,
  icons stacked up from the mana gauge 3px apart (backpack, book, purse), bag `hero_inventory` 562x121 at
  `(W/2 - 320 + 35, H - 101 + slide)` with slots at `+(7, 6)` and 32px pitch, chest panel 115x378 at the top-left with
  items at `(2 + 32x, 13 + 32y)`. UI bitmaps are `.bmp` with black as transparent; the filled red gauge is grey and is
  tinted red by the engine. Digits come from `font10x10_inventory` (digit `n` at x = 11n + 1.5, 10x10), drawn right to
  left. Item icons are `<class>[icon].bmp` next to the model; an item's slot size is its icon size / 32, rounded up, 1 to 3
  (`StdHost::item_slots`). The player grid (`PlayerState::slots`) is 16x3 per bag filled column by column; gold items
  (`.../gold_coin/gold_coin`) go to `PlayerState::gold`. The HUD is rebuilt as plain UI image nodes every frame.
- **Animations of fixtures**: a door swings by bone rotation, but a portcullis or trapdoor has a *static* bone and moves by
  the keyframe's whole-object translation (`KeyFrame::translate`). The original (`Cedric_ConcatenateTM`) adds it to the root
  bone for everything that is not an NPC; NPCs would double-count it, because their entity moves by it instead.
  `Skeleton::pose_object` does this. A dropped item that never had a scene object (loot from a chest) is spawned on demand
  by `entities::spawn_dropped`; `StdHost::take_dropped` is the queue.
- **Stats**: ported from `Player.cpp` into `PlayerState` (`full_skills`, `misc`, `add_xp`, `xp_for_level`): skills = spent
  points + attribute formulas (e.g. stealth = 2 x dexterity, mecanism = dexterity + mind), life = constitution x (level + 2),
  mana = mind x (level + 1), a level is 15 skill + 1 attribute point. Scripts read them as `^player_skill_*`,
  `^player_attribute_*`, `^player_life` and so on, which the host publishes into `ScriptWorld::sys` every frame. The book
  (`hud_book.rs` holds all its coordinates, tested; `hud_ui.rs` draws it) uses the game's own font `misc/arx*.ttf` at 18 px
  x `smallTextScale`; text is wrapped and paged by an estimate of character width, since Bevy cannot measure text up front.
- **Never commit the original source code** ("Arx Fatalis original source code/" in the repo folder, supplied by the owner
  for reference): it is in `.gitignore`. It holds the 2002 DANAE engine (`DANAE/ARX_Cedric.cpp` for skinning,
  `DANAE/ARX_Script.cpp`, `EERIE/`); prefer it, then ArxLibertatis, when a behaviour is unclear.
- **Characters face and walk by the engine's formula, not by what "looks right"**: walking animations move a model along its object-space -Z (root translation of the last keyframe), and the engine rotates that by `VRotateY(180 - yaw)`; so stored NPC yaw 0 walks toward +Z (Arx), 90 toward -X, 180 toward -Z, 270 toward +X (`arx_level::npc::facing` / `yaw_toward`). Rendering with `entity_rotation(.., true)` agrees. An earlier guess (+Z forward in model space) made characters walk backwards.
- Characters only fall and collide while *moving*: an NPC with behaviour NONE (the default) or playing a script animation (`playanim -l wait`, as hanging corpses do) keeps the height the level gave it; the engine returns before applying gravity.
- NPC behaviour is entirely script-driven through commands the host queues as `NpcRequest`s (arx-script never decides anything); `NpcWorld::update` applies them in order, then simulates only characters within 5000 units of the player. Scripts react to `reachedtarget`, `lostTarget`, `pathfinder_failure`, `detectplayer`, `hear`, `collide_door` (a door told by a character that bumps it opens for it), `strike`, `hit` (a script that does not ACCEPT cancels the damage), `ouch`, `die`, `target_death`.
- The hero is the full `human_base` model with the faces touching the "1st" selection left out, placed where the player stands (rotation `yaw + PI` about Y); the legs play `wait/walk/run/...` and the arms a second animation layer on top (`Skeleton::pose_layers`: a bone that a higher layer animates takes nothing from the lower ones). The hero's own animations drive 44 groups against the model's 39: extra groups are ignored. The weapon is attached with the bone rotation of the hand vertex (`primary_attach`) and the weapon's own `primary_attach` vertex; blows test the weapon's `hit_<radius>` vertices against the characters' cylinders.
- **The menu** (`arx-viewer/src/menu.rs`, from `gui/MainMenu.cpp` and `gui/CharacterCreation.cpp`): `layout()` is the single
  source of truth for a screen (tests pin it): entries at (370, 100 + 50n) of 640x480 stretched to the window, a page in the
  window at (20, 25) 321x430. While `Menu::screen` is set, every game system is skipped (`run_if(menu::closed)`), so add new
  level systems inside that group. Character creation runs only `hud_ui::mouse`/`draw` with `Ui::creating` (the book alone).
  Options are ten 0..10 values saved outside the repository (`%APPDATA%/arx-fatalis-rust/options.cfg`); screenshot runs never
  read or write them. "New quest" with a game running restarts the program with `--new-quest` (there is no level reload).
- **Light**: lights with `extras & 1` (every torch) are *not* in the baked colours; `lighting.rs` adds them per vertex at 30 Hz
  (`rgb x cos x falloff x intensity x 0.85 x 0.5`, in 0..255 display values, then to linear). Blob shadows (`shadows.rs`) and
  particles (`particles.rs`) are single meshes rebuilt every frame; **a mesh must never be left with zero vertices** (Bevy's
  allocator logs use-after-free errors), so they keep one zero-size triangle. Bevy blends in linear light: to darken the
  picture by `s` as the engine does, use alpha `1 - (1 - s)^2.2`.
- **Cutscenes and zones**: `arx_level::stage` (cameras on the level's paths, bars, fades, control lock, teleports) and
  `arx_level::zones` (paths with a height are zones: `enterzone`/`leavezone`, `controlledzone_*`). Scripts depend on system
  variables being there: an unknown `^var` reads as 0, `^target`/`^speaking`/`^life` are published, the hero's id is `player`,
  and every entity gets a `main` heartbeat. A skeleton is made for every model with animations (no bone-count check).
- **The jump and the crouch are deliberately not the original's** (the owner asked: "more grounded, even if different").
  `arx_physics::Player::classic = true` gives the port described above; the default throws the body up 95 units under one
  gravity of 1700 units/s^2 for jump and fall alike, keeps about running speed in the air (1.3x forward) and on landing,
  and ducks in 140 ms with the cylinder low at once. Gaps the original's long jump (three times running speed in the air)
  crossed may now be out of reach; if a level needs one, that is the place to look. The eyes ease through a crouch
  (`PlayerBody::eye_rise`).
- **Fighting characters**: a blow starts whatever the legs' animation is (only a forced animation, a blow or `die` hold
  it back). Requiring `wait` made a goblin that arrived in `fight_walk_forward` stand still until it was hit again.
  `arx npc 1 20 --hit goblin_base_0050 [--behind] [--dark]` strikes a character every two seconds and prints what it does.
- **Speech**: whoever speaks plays `talk_neutral|happy|angry` (head and jaw only) as a second animation layer
  (`speech::talk`). Subtitles are written during conversation scenes (black bars), as in the original, and otherwise only
  for speakers within 700 units. Sound fades by the engine's model (`audio::gain`: full to 200 units, inverse distance with
  rolloff 1.3, flat past 2200); Bevy's own distance fading is switched off by a tiny spatial scale and only pans.
- **The hero bends to look** (`Skeleton::pose_layers_bent`, the engine's `ex_rotate`): head/neck/chest/belt take
  0.1/0.1/0.4/0.4 of the pitch with a weapon drawn, a quarter each otherwise, so blows go where the eyes point. Losing the
  controls to a cutscene puts the weapon away (`PutPlayerInNormalStance`), or the outside body T-poses its arms.
- The captured mouse is re-centred every frame and released when the window loses focus; while it is captured the
  interface gets no cursor position (a hidden cursor used to click whatever it was parked on).
- **Magic** (`arx-level/src/magic.rs`, `arx-viewer/src/magic.rs`): runes are the engine's direction strings
  (`Rune::strokes`), but recognition is our own by the owner's wish: the stroke and every rune's ideal path (at several
  proportions) are resampled to 48 points, centred, scaled by the longer side, and compared by point distance + heading +
  path length; the best wins if under `ACCEPT` and clearly ahead of the next rune. A test draws 3000 shaky runes (>96 %
  right, <0.4 % mistaken) and refuses circles, spirals and zigzags: keep it passing when tuning. The spell table, mana
  costs, spell level and effect numbers are the engine's. `ActiveSpells::cast` pays mana and queues `spellcast` (name,
  level) to every entity, which scripts act on; ignit/douse flip `Torch::lit`, magic missile is a flying sphere that
  calls `NpcWorld::hurt`, heal/armor/lower_armor/speed run in `ActiveSpells`. The other 43 spells only notify scripts.
  `--runes all --cast aam,yok` and `ARX_LOG_MAGIC=1` test it headlessly. Runes are learnt with `rune -a <name>`.
- **Hitting things**: the hero's blade also strikes fixtures whose pickable sphere it reaches (`player_strike_object` ->
  `hit` with damage and weapon kind); the cell's `jail_wood_grid` breaks that way. `activatephysics` sets items loose to
  fall (`StdHost::take_loosened`). A double click on an item in the pack uses it and then keeps it in the hand
  (`Ui::held`, the original's COMBINE): the next click, on an item or on something in the world, sends `combine`.
  `tweak icon` changes an item's inventory picture (`hud::icon_class`), which is what tells the rune stones apart.
- **Mods** (`arx-formats/src/mods.rs`, README "Mods"): folders or `.pak` archives in `mods/` (ignored by git) layered over
  the game's files in alphabetical order by `PakSet::apply_mods`; everything that reads the game goes through `PakSet`, so
  a mod can replace or add any file. `pak::write_archive` writes the game's archive format (stored files, obfuscated table
  starting with an empty root directory); `arx pack` / `arx mods -f` are the tools. The menu shows *Mods* only when the
  folder has some; applying a choice restarts the program. **Read game files through `PakSet` only**, never straight from
  the game directory, or mods will not reach that code.
- **Items turn into other items** with `replaceme` (`StdHost`, tested): one of a stack changes, and the new item takes
  the old one's place in the pack (or joins a stack of its kind), in a chest, on the hero or on the floor. That is how wine
  leaves an empty bottle, a bottle is filled, and flour and water make dough; `arx game 1 "pickup bottle_wine_0002" "use
  bottle_wine_0002" status` shows it. Only items are replaced so far (not characters or fixtures).
- A blow or readying a weapon animates arms and chest only: the viewer plays the fighting stance under it
  (`Animated::under`), or the legs freeze.
- **Frame rate: never rewrite a mesh that did not change.** Every `Assets<Mesh>::get_mut` uploads that mesh again and costs
  0.01-0.02 ms inside Bevy, whatever its size; a level has ~3000 meshes. The torch light used to rewrite 1900 of them per
  tick (13 ms). Now each torch's effect on each vertex is precomputed (`LitChunk::lit_by`), only torches within 2200 units
  flicker, and at most `CHUNKS_PER_TICK` meshes are rewritten per tick; `animated::animate` skips poses that did not change
  (`Animated::shown`) and rebuilds distant ones every 3rd/10th frame. Measure with `ARX_LOG_FPS=1` (level 1: ~8 ms). An
  unfocused window is held to 60 updates/s by winit, which looks like VSync in measurements.
- **Tweaks** (`arx-formats/src/tweak.rs`, `arx-level/src/model.rs`): `tweak head|torso|legs|all|upper|lower <name>` takes body
  parts from `<model dir>/tweaks/<name>.ftl` (the `head`/`chest`/`leggings` selections, vertices matched by position, as
  `CreateIntermediaryMesh`), `tweak skin a b` swaps a texture. They are kept in `EntityState::tweaks` and applied when the
  model is built (`entities::resolve_model`, cached by tweak list); `model_serial` makes the viewer build a model again
  (`entities::respawn`). The hero's model is `human_base` plus what armour says (`setplayertweak`) and the chosen face
  (`StdHost::refresh_player_model`), so vertex indices such as `primary_attach` must come from the built model.
- **The hero in the book** (`book_hero.rs`): a second copy of the hero's model on render layer 1, drawn by a second camera
  into a picture the HUD puts on the page (position, turn, focal 520 and lights from `RenderBookPlayerCharacter`). Because of
  that second camera every other system asks for `(With<Camera3d>, Without<BookCamera>)`. Equipping an item takes it out of
  the backpack (the original does too): it is on the hero in the book, and a click on its slot there takes it off.
- **The player's yaw is the engine's**: forward is `(-sin yaw, cos yaw)` in Arx x/z, the same as an NPC's stored yaw and as
  `Fly::yaw` (an earlier `PI - yaw` in the cutscene teleport turned the hero round). `loadanim -p` / `playanim -p` act on the
  hero (lying, dragged, sitting): `player_body::drive` plays that pose on the bodies, moves the hero by the animation's own
  translation and switches the player's physics off meanwhile (`Fly::posed`).
- A dead character hears no events but `dead`, `die`, `executeline`, `reload` and being searched (`ScriptEntity::dead`);
  without that a corpse stands up when a script addresses its group.
- Level 9 does not exist. Many levels' saved start is not on a real floor; the viewer starts at the nearest entity
  when the saved start is more than 250 units off its floor.

## Current state and known gaps

Done: PAK/FTL/FTS/LLF/DLF/TEA/WAV readers, level rendering with baked lighting, placed entities with per-vertex
lighting, skeletal animation (CPU skinning), walkable player with collision, script interpreter running every
entity's start-up (all 23 levels), doors/levers/portcullises (animation, collision, sound), spatial sound,
localised names, voiced dialogue with subtitles (`speak`, `playspeech`) and `herosay` notifications, player life/mana/
hunger, the original HUD (gauges, backpack/book/purse icons, grid inventory with item icons, chest panel, crosshair and
cursors), item pickup with stacking, eating/healing, and keys that
unlock doors and chests (`combine`), original-faithful player movement (run/sneak/crouch/jump, ceilings, fall damage), NPC and
fixture collision, chests and corpses as containers, readable notices (`note`) and `rotate`, NPC behaviour and combat,
equipment, zones, script cutscenes, torch light with flames and smoke, blob shadows, the main/pause menu with options and
character creation.

Done since: footsteps from the engine's material tables (`SoundMap`), dragging items in the 3D world and throwing them, NPC path-finding / walking / patrolling / perception / melee, equipment (slots, `setequip` modifiers, `equip`), the hero's first-person body and weapon animations, the hit-strength gauge, blows and damage, characters that die and give experience.

Missing: the map and spell pages of the book, combat cursors, active-spell and hunger icons, the HUD sliding away in free look,
cinematic cameras for `speak -c`, most spell effects (see Magic), precasting, NPC spell casting (`spellcast`), bows and arrows, NPC weapons drawn in hand, NPC footsteps, `usepath`, inventory weight limits,
level changes (`teleport -l`, needs state transfer), the 2D `.cin` cinematics (skipped: `cine_end` is sent at once), ladders, leaning, music/ambiance zones, fog, light flares, save games, credits and key bindings. About 60 script
commands are skipped (the interpreter ignores a command it does not know, line by line, and counts it in
`Stats::unknown_commands`; `arx script` prints the most frequent ones). With jumping off, `arx walk N --no-jump` has no
rescues in 21 of 23 levels; levels 10 and 20 have genuine drops where the player falls out of the world and is put back on
their last solid ground (long jumps make the random fuzz leave the world in many more levels: that is the faithful jump).

When you finish a change, say how to run it (the viewer command that shows the change).
