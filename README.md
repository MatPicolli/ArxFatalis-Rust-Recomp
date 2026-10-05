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
| `arx-level` | Glue that runs a level's entity scripts, turns solid entities into collision obstacles, and runs the characters (`npc`: anchor path-finding, behaviours, perception, melee) and the hero's way of fighting (`player_combat`). No Bevy dependency. |
| `arx-cli` (`arx`) | `stats`, `ls`, `extract`, `extract-all`, `verify`, `ftl`, `fts`, `dlf`, `tea`, `walk`, `script`, `npc`, `audio`, `polys-at` |
| `arx-viewer` | Bevy asset explorer for models and textures, and a free-fly level viewer with placed entities |

## Usage

The game directory defaults to `D:\Steam\steamapps\common\Arx Fatalis`; override with `--game-dir` or `ARX_DIR`.

```bash
cargo run --release -p arx-cli -- verify          # read & decompress every file in every PAK
cargo run --release -p arx-cli -- ls graph/levels
cargo run -p arx-viewer -- models barrel          # browse models whose path contains "barrel"
cargo run -p arx-viewer -- textures npc_          # browse textures
cargo run -p arx-viewer -- models bat --shot out.png   # headless screenshot, then exit
cargo run -p arx-viewer -- level 1                # play level 1 (levels 0-8, 10-23): main menu, character creation, intro
cargo run -p arx-viewer -- level 1 --no-menu      # straight into the level (--no-cutscenes also keeps the view and controls yours)
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
cargo run --release -p arx-cli -- npc 1 40              # run level 1's characters for 40 s headlessly: who walks, patrols, fights (-f <id> shows a route)
cargo run --release -p arx-cli -- tweaks                # build every model the scripts changed (heads, clothes, skins) and report what failed
ARX_LOG_FPS=1 cargo run --release -p arx-viewer -- level 1 --no-menu   # print the frame rate and what each system costs
ARX_LOG_NPC=goblin_base_0049 cargo run -p arx-viewer -- level 1 --focus goblin_base_0049   # log a character's commands and state once a second
cargo run -p arx-viewer -- level 1 --cam 9543,2945,5800 --look 180,-5 --equip short_sword_0005 --draw-weapon --attack-test --shot shots/fight.png   # first-person combat, scripted
```

Walking: WASD move (you run by default, like the original; hold Shift to sneak), X crouch (C toggles), Space jump, E use the thing you are looking at: take items, open chests, read notices, `chat` with NPCs, `action` for everything else. Fighting: double-click a weapon or armour piece in the backpack to equip it (a two-handed weapon takes the shield off), Tab draws or puts away the weapon, hold the left button to wind up a blow (the gauge at the bottom of the screen fills; the mouse direction picks left/right/top/bottom) and let go to strike; what the blade touches is hurt by the engine's damage formula (your damage and critical chance, the target's armour class and absorption), and killing gives its experience. You see your own body: legs when looking down, and the arm and weapon swinging. A long fall hurts (`(height - 400) / 15` life) and R revives you after dying. Click to capture the mouse for look, Esc opens the menu (and frees the mouse)
(right-drag also looks). F toggles free flight: Q/E down/up, Shift fast, scroll changes speed.
The HUD prints the eye position in Arx coordinates, ready to paste into `--cam`.

Model/texture controls: Left/Right = prev/next, PgUp/PgDn = +-25, Home = first, left-drag = orbit, scroll = zoom.

## Menu

The game starts on the original's main menu (`gui/MainMenu.cpp`: its background, font and positions), and `Esc` brings it back as the pause menu; while it is open nothing in the level moves and its sounds are paused. *New quest* opens character creation: the book's character sheet with 16 attribute and 18 skill points to hand out (left click spends, right click takes back), *Quick generation* (first the average hero, then random ones, as in the original), the hero's face, and *Done* once every point is spent, which starts the level's intro. *Options* has full screen, VSync, field of view, HUD size, subtitles, master/effects/speech volume, mouse sensitivity and inverted mouse; they take effect at once and are kept in `%APPDATA%\arx-fatalis-rust\options.cfg`. Loading, saving and the credits are shown but do nothing yet. With a game running, *New quest* asks first and then starts the program again on character creation.

## Going between levels

When a script sends the hero to another level (`teleport -l <level> <marker>`: the stairs and passages of the game), that level is loaded and the hero arrives at its marker with everything they carry: the pack, what is worn, gold, runes, quest log, the game's global variables. The level left behind is kept exactly as it was (doors opened, things moved or taken, who is dead, torches put out) and is found like that on the way back; its scripts are told with the `reload` event, as in the original. There is no confirmation icon yet (the journey starts at once) and no saving to disk.

```bash
cargo run --release -p arx-viewer -- level 1 --no-menu --no-cutscenes --go 2:marker_0217,1:marker_0367   # there and back by itself, 4 s apart
```

## Dynamic lights and shadows (experimental)

*Options > Dynamic lights and shadows* (or `--dynamic-light`) turns the torches and fires near you into real lights: they light every pixel instead of every vertex, and the two nearest cast shadows (of walls, bars, furniture, people). The level's baked light stays underneath as ambient light. This is not how the game looked, it costs frame rate (about a quarter on level 1), lights pop as the nearest ones change, and things placed in the level get the torch light twice. Off by default.

## Magic

Hold **Ctrl**, draw a rune with the left mouse button held, and let the button go: the rune is named aloud and its stone appears on screen. Draw the next rune the same way, then let go of Ctrl to cast what the runes make (Aam Yok lights the torches around you, Nhi Yok puts them out, Aam Taar is a magic missile, Mega Vitae heals, Mega Kaom is armour, Mega Movis speed). You need the runes (use a rune stone from the backpack to learn it) and the mana; spells are as strong as a tenth of casting skill plus mind, and mana comes back slowly by itself.

Runes are drawn as the rune stones show them, but the recognition is not the original's: instead of demanding an exact list of directions it compares your stroke with every rune's shape as a whole, so wobbles, rounded corners and uneven strokes are fine, while a scribble is still refused. All 49 spells of the game are known (their runes, cost and the `spellcast` event that scripts react to); the six above do something so far, the others say so when cast.

```bash
cargo run --release -p arx-viewer -- level 1 --no-menu --no-cutscenes --runes all     # every rune, to try it out
cargo run --release -p arx-viewer -- level 1 --no-menu --no-cutscenes --runes all --cast aam,taar --focus goblin_base_0050   # cast without drawing
```

## Mods

Drop a mod into the `mods` folder next to where you run the game (it is made on the first run; `--mods-dir` or `ARX_MODS` points somewhere else) and it is used: no installing, no editing of the game's files.

- A mod is a **folder** or a **`.pak` archive**. Inside it, files sit where the game has them: `graph/obj3d/textures/...` (textures), `graph/obj3d/interactive/.../x.asl` (scripts), `game/graph/obj3d/...ftl` (models), `game/graph/levels/...` and `graph/levels/...` (levels), `graph/interface/...` (menus, HUD), `sfx/...`, `speech/<language>/...`, `localisation/...`, `misc/...`.
- A file a mod has **replaces** the game's file of that name; anything else is **added**. Mods apply in alphabetical order, so of two mods with the same file the later one wins.
- An optional `mod.ini` at the top (`name = ...`, `author = ...`, `version = ...`, `description = ...`) is what the menu shows.
- With anything in the folder, the main/pause menu gets a **Mods** entry: tick mods on and off there, then *Apply* (the game starts again, because mods are read at start-up). Which mods are off is kept in `%APPDATA%\arx-fatalis-rust\mods-off.cfg`, so the mods themselves stay untouched.

```bash
cargo run --release -p arx-cli -- extract graph/obj3d/interactive/items/provisions/bottle_wine/bottle_wine.asl -o my_mod/graph/obj3d/interactive/items/provisions/bottle_wine/bottle_wine.asl   # start from a game file
cargo run --release -p arx-cli -- pack my_mod -o mods/my_mod.pak     # a folder into one .pak (the game's own format)
cargo run --release -p arx-cli -- mods -f                            # what is in mods/, and every file it replaces or adds
cargo run --release -p arx-cli -- --mods-dir mods script 1           # the tools look at the plain game unless told to use mods
```

Not there yet: changing the load order in the menu, mods that patch part of a file (a mod replaces whole files), and mods that add native code.

## Interface

The HUD is laid out like the original's (`gui/Hud.cpp`) using its own bitmaps: the life gauge bottom-left and the mana gauge bottom-right (a filled gauge shows through the empty gauge's frame; click one for the number), and above the mana gauge the backpack, the spell book (not available yet) and, once you have gold, the purse (hover for the amount). The backpack slides up from the bottom: a 16x3 grid per bag (`addbag` adds more), every item taking as many 32-pixel slots as its icon is big, with the item's own icon and stack count drawn in the original's digit font. Chests slide in from the left with the chest's own skin and the pick-all and close buttons.

The cursor is free while the backpack, the book or a chest is open: drag items inside the bag, double-click to use or eat one, drop one on another item to combine them (a key on a door works the same way: release it while looking at the door), release it out in the world to drop it, onto an open chest to put it away, and click an item in a chest to take it. Keyboard: `I`, then Up/Down, Enter use, `H` hold an item to use on what you look at, `G` drop; with a chest open Up/Down, Enter takes, `T` takes all, Backspace closes. `B` (or a click on the book icon) opens the player's book: the character sheet with level, experience, the four attributes, the nine skills and the derived numbers (armour, resistances, life, mana, damage), where left-click on an attribute or skill icon hands out a point (right-click takes one back while the hero is still level 0) and hovering explains it with the game's own texts; the quest log (`quest` in scripts) is its second page. The level-up icon above the book icon shows when points are waiting (a new hero has 16 attribute and 18 skill points; levels come from experience, `addxp`). Notes, signs and books are drawn on the original's stone, paper and book backgrounds in the game's own font, growing and paging as the text needs. `F3` shows the developer overlay (position, help line). `--hud-scale 0.5` gives the original's own pixel size, 1.0 (the default) the biggest size that fits.

## Coordinates

Arx is +Y down, +Z forward. Bevy is +Y up, -Z forward. The conversion `(x, y, z) -> (x, -y, -z)` is a
proper rotation, so triangle winding is unchanged. Entity yaw is remapped like the original engine does
(`270 - yaw` for objects, `180 - yaw` for NPCs) before it is turned into a rotation; see `arx-level`.

## Roadmap

1. [x] PAK reader, `.ftl` models, asset explorer
2. [x] Level geometry (`fast.fts`) with baked vertex lighting (`.llf`), mipmaps, free-fly camera
3. [~] Scene definition: entities placed and lit per vertex like the original; zones send their enter/leave events and paths carry cutscene cameras; torches and fires light the level per vertex each frame, flickering, with flames and smoke, and everything standing in the scene has the original's blob shadows. Still missing: fog rendering, light flares
4. [x] Animations and player: skeletal animation (CPU skinned; NPCs idle with the animation their script names). The player moves like the original (ported from its source and measured against the hero animations: running 266.7 units/s, sneaking 188.3, crouched 100, a jump rises 130 units in 200 ms then falls slowly, falls over 400 units hurt, landing slows you briefly), crouches (X / C) and hits its head on ceilings, must crouch to enter low gaps and stays crouched under them, steps up 40 units, and is blocked by closed doors, solid fixtures and living NPCs (collision cylinders computed like the engine's). Doors, levers, chests (locked ones need their key), notices, signs and puzzle wheels work; gates and trapdoors move by the animation's whole-object translation, like the original. There is no swimming: the original has none either (water polygons are not solid; you wade). Not covered: ladders (`POLY_CLIMB`), leaning (Q/E), head bob, and teleporters between levels (they need the save/transfer system, see 7)
5. [~] Scripting, entity collision and sound: closed doors, portcullises and other solid entities block the player and can be walked on (a trapdoor plugging a hole), and a door becomes passable when its script turns collision off; `play` sounds (door, ambient loops) are decoded from the game's ADPCM files and play positionally. Scripting: the interpreter runs every entity's `load`/`init`/`initend`/`game_ready` at level start (all 23 levels, 117k commands, no runaways) and entities are shaped by it (mesh variants, scale, hidden/destroyed, animations); `E` triggers events and doors/levers animate. Implemented game commands: usemesh, setscale, objecthide, loadanim, playanim (incl. `-e`), collision, setinteractivity, setgroup, setname, destroy, speak, playspeech, herosay, playerstacksize, setfood/setweight/setprice, eatme, specialfx (heal, mana). Text is localised (`--language`, default english): the HUD shows entity names, `herosay` shows notifications, and `speak` plays the voice file with subtitles (`--no-subtitles` to hide them) and runs the rest of its line when the speech ends. The interface is the original's (see "Interface" below): items are taken with `E` (same kinds stack, gold goes to the purse), the backpack opens with `I` or a click on its icon, and chests, corpses and other containers (`inventory create/add/addmulti/open`) show their own panel, `note` shows notices and signs, `rotate` turns puzzle wheels. Characters (see `arx-level/src/npc.rs`): they path-find over the level's anchors (weighted A* as the engine's thread does, fleeing, wandering with pauses, looking for someone), walk at the speed of their animation's root motion with the level's collision, turn at 330 degrees/s, open doors by bumping them (`collide_door`), notice the player by sight (110 degree cone, 2000 units, line of sight) and hear their steps (`hear`), obey `behavior`, `settarget`, `setmovemode`, `setnpcstat`, `setdetect`, `setspeed`, `physical` ..., fight at 220 units with their strike animations and hurt the hero, and die (`die`, `target_death`). Still missing: music/ambiance zones, spells, level changes (`teleport -l`, `worldfade`), cameras and cinematics, NPC weapons drawn in the hand, NPC footsteps, `usepath` patrols, about 40 other commands
6. [~] Inventory, equipment and combat: backpack, containers, equipment slots with the items' `setequip` modifiers, the hero's first-person body with weapon animations, melee blows and damage ported from the engine, characters that attack back. Not yet: bows and arrows, armour changing the hero's model (`tweak`), durability loss, magic
7. [~] Menus: main/pause menu, options, character creation. Not yet: save games, credits, key bindings
