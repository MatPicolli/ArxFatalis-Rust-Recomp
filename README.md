# Arx Fatalis recomp (Rust + Bevy)

A Rust reimplementation of the 2002 RPG *Arx Fatalis* on the Bevy engine. It loads the **original game
assets** from your own installation; no game data is included.

Format and behaviour knowledge comes from [ArxLibertatis](https://github.com/arx/ArxLibertatis) (GPLv3), so
this project is **GPL-3.0-or-later**.

## Crates

| crate | purpose |
|---|---|
| `arx-formats` | Pure-Rust readers: PAK archives (+ PKWare DCL decompression), `.ftl` models, `.fts` level geometry, `.llf` baked lighting, `.dlf` scene definition (entities, fogs, paths/zones), `.tea` animations and skeletons, `.wav` sounds (MS-ADPCM and PCM). No Bevy dependency. |
| `arx-script` | Interpreter for the `.asl` entity scripting language (events, variables, goto/gosub, timers, event queue) and a host for the visual commands. No Bevy dependency. |
| `arx-physics` | Level collision (spatial hash over polygons, plus switchable entity obstacles such as doors) and the first-person player body. No Bevy dependency. |
| `arx-level` | Glue that runs a level's entity scripts and turns solid entities into collision obstacles. No Bevy dependency. |
| `arx-cli` (`arx`) | `stats`, `ls`, `extract`, `extract-all`, `verify`, `ftl`, `fts`, `dlf`, `tea`, `walk`, `script`, `audio`, `polys-at` |
| `arx-viewer` | Bevy asset explorer for models and textures, and a free-fly level viewer with placed entities |

## Usage

The game directory defaults to `D:\Steam\steamapps\common\Arx Fatalis`; override with `--game-dir` or `ARX_DIR`.

```bash
cargo run --release -p arx-cli -- verify          # read & decompress every file in every PAK
cargo run --release -p arx-cli -- ls graph/levels
cargo run -p arx-viewer -- models barrel          # browse models whose path contains "barrel"
cargo run -p arx-viewer -- textures npc_          # browse textures
cargo run -p arx-viewer -- models bat --shot out.png   # headless screenshot, then exit
cargo run -p arx-viewer -- level 1                # walk around level 1 (levels 0-8, 10-23)
cargo run -p arx-viewer -- level 1 --fly          # start in free flight (F toggles)
cargo run -p arx-viewer -- level 1 --no-npcs     # hide NPCs
cargo run -p arx-viewer -- level 1 --mute        # no sound
cargo run -p arx-viewer -- level 1 --focus light_door_0075   # camera in front of an entity (add :back or :side)
ARX_FULLBRIGHT=1 cargo run -p arx-viewer -- level 1          # ignore baked lighting, to inspect geometry
cargo run --release -p arx-cli -- orient-panel   # check door/portcullis placement against the level
cargo run -p arx-viewer -- models goblin_base --anim goblin_normal_wait   # play an animation; , and . cycle
cargo run --release -p arx-cli -- script 1 -d  # run every entity script of level 1, list what they did to entities
cargo run -p arx-viewer -- level 1 --use-entity light_door_0074:open   # headless: send an event, then --shot
cargo run --release -p arx-cli -- audio            # decode every game sound (ADPCM -> PCM)
cargo run --release -p arx-cli -- polys-at 11 8144 7467   # what level polygons cover a point (collision debugging)
cargo run --release -p arx-cli -- walk 1         # headless collision test: drop, run 8 ways, 3-minute fuzz (--no-jump separates collision bugs from long jumps)
cargo run --release -p arx-cli -- player-speeds  # the original's movement speeds, measured from the hero animations
cargo run --release -p arx-cli -- fixtures       # trigger every fixture (doors, levers, chests, ...) and list what reacts
cargo run -p arx-viewer -- level 1 --start dlf   # start at the level file's editor camera instead
cargo run -p arx-viewer -- level 1 --cam 8650,-6000,8550 --look 0,-89   # Arx coords, yaw,pitch degrees
```

Walking: WASD move (you run by default, like the original; hold Shift to sneak), X crouch (C toggles), Space jump, E use the thing you are looking at: take items, open chests, read notices, `chat` with NPCs, `action` for everything else. A long fall hurts (`(height - 400) / 15` life) and R revives you after dying. Click to capture the mouse for look, Esc releases it
(right-drag also looks). F toggles free flight: Q/E down/up, Shift fast, scroll changes speed.
The HUD prints the eye position in Arx coordinates, ready to paste into `--cam`.

Model/texture controls: Left/Right = prev/next, PgUp/PgDn = +-25, Home = first, left-drag = orbit, scroll = zoom.

## Interface

The HUD is laid out like the original's (`gui/Hud.cpp`) using its own bitmaps: the life gauge bottom-left and the mana gauge bottom-right (a filled gauge shows through the empty gauge's frame; click one for the number), and above the mana gauge the backpack, the spell book (not available yet) and, once you have gold, the purse (hover for the amount). The backpack slides up from the bottom: a 16x3 grid per bag (`addbag` adds more), every item taking as many 32-pixel slots as its icon is big, with the item's own icon and stack count drawn in the original's digit font. Chests slide in from the left with the chest's own skin and the pick-all and close buttons.

Press Esc to release the mouse and use the cursor (it opens up by itself while the backpack or a chest is open): drag items inside the bag, double-click to use or eat one, drop one on another item to combine them (a key on a door works the same way: release it while looking at the door), release it out in the world to drop it, onto an open chest to put it away, and click an item in a chest to take it. Keyboard: `I`, then Up/Down, Enter use, `H` hold an item to use on what you look at, `G` drop; with a chest open Up/Down, Enter takes, `T` takes all, Backspace closes. `F3` shows the developer overlay (position, help line). `--hud-scale 0.5` gives the original's own pixel size, 1.0 (the default) the biggest size that fits.

## Coordinates

Arx is +Y down, +Z forward. Bevy is +Y up, -Z forward. The conversion `(x, y, z) -> (x, -y, -z)` is a
proper rotation, so triangle winding is unchanged. Entity yaw is remapped like the original engine does
(`270 - yaw` for objects, `180 - yaw` for NPCs) before it is turned into a rotation; see `arx-level`.

## Roadmap

1. [x] PAK reader, `.ftl` models, asset explorer
2. [x] Level geometry (`fast.fts`) with baked vertex lighting (`.llf`), mipmaps, free-fly camera
3. [~] Scene definition: entities placed and lit per vertex like the original; fogs, paths and zones are parsed but not used yet. Still missing: dynamic/flickering lights, fog rendering, particles
4. [x] Animations and player: skeletal animation (CPU skinned; NPCs idle with the animation their script names). The player moves like the original (ported from its source and measured against the hero animations: running 266.7 units/s, sneaking 188.3, crouched 100, a jump rises 130 units in 200 ms then falls slowly, falls over 400 units hurt, landing slows you briefly), crouches (X / C) and hits its head on ceilings, must crouch to enter low gaps and stays crouched under them, steps up 40 units, and is blocked by closed doors, solid fixtures and living NPCs (collision cylinders computed like the engine's). Doors, levers, chests (locked ones need their key), notices, signs and puzzle wheels work. There is no swimming: the original has none either (water polygons are not solid; you wade). Not covered: ladders (`POLY_CLIMB`), leaning (Q/E), head bob, and teleporters between levels (they need the save/transfer system, see 7)
5. [~] Scripting, entity collision and sound: closed doors, portcullises and other solid entities block the player and can be walked on (a trapdoor plugging a hole), and a door becomes passable when its script turns collision off; `play` sounds (door, ambient loops) are decoded from the game's ADPCM files and play positionally. Scripting: the interpreter runs every entity's `load`/`init`/`initend`/`game_ready` at level start (all 23 levels, 117k commands, no runaways) and entities are shaped by it (mesh variants, scale, hidden/destroyed, animations); `E` triggers events and doors/levers animate. Implemented game commands: usemesh, setscale, objecthide, loadanim, playanim (incl. `-e`), collision, setinteractivity, setgroup, setname, destroy, speak, playspeech, herosay, playerstacksize, setfood/setweight/setprice, eatme, specialfx (heal, mana). Text is localised (`--language`, default english): the HUD shows entity names, `herosay` shows notifications, and `speak` plays the voice file with subtitles (`--no-subtitles` to hide them) and runs the rest of its line when the speech ends. The interface is the original's (see "Interface" below): items are taken with `E` (same kinds stack, gold goes to the purse), the backpack opens with `I` or a click on its icon, and chests, corpses and other containers (`inventory create/add/addmulti/open`) show their own panel, `note` shows notices and signs, `rotate` turns puzzle wheels. Still missing: equipment and weapons, footsteps, music/ambiance zones, NPC behaviour (`behavior`, `settarget`), spells, level changes (`teleport -l`, `worldfade`), cameras and cinematics, ~60 other commands
6. [ ] Inventory, combat, magic
7. [ ] Audio (`.wav`), UI, save games
