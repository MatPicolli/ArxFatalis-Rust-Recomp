//! The player's interface state, its keyboard controls and the paper panel for notes. The original HUD itself
//! (gauges, icons, inventory panels, cursor) is drawn by [`crate::hud_ui`].

use crate::convert::to_bevy;
use crate::scripting::Scripting;
use crate::speech::Speech;
use crate::Fly;
use arx_level::inventory;
use arx_script::EntityId;
use bevy::prelude::*;

/// Fully slid out of view, in unscaled pixels (the bag slides up from the bottom edge).
pub const BAG_HIDDEN: f32 = 110.0;
/// Where the chest panel hides to the left (unscaled pixels).
pub const PANEL_HIDDEN: f32 = -160.0;

/// An item being dragged with the mouse: an icon over the interface, the 3D item itself out in the world.
#[derive(Clone, Copy)]
pub struct Drag {
    pub item: EntityId,
    /// Where the cursor grabbed the icon, relative to its top-left corner (pixels).
    pub grab: Vec2,
    /// It was picked up from the floor, not out of the inventory.
    pub from_world: bool,
    /// It has been given a model in the scene (needed for items that came out of chests).
    pub spawned: bool,
}

#[derive(Resource)]
pub struct Ui {
    /// The backpack panel is open.
    pub open: bool,
    /// Entry highlighted for the keyboard (index into the carried list).
    pub selected: usize,
    /// Item chosen to be used on something: another item, or what the player looks at (`E`).
    pub held: Option<EntityId>,
    /// Entry highlighted in the open container (chest, corpse).
    pub container_selected: usize,
    /// Something being read (a note, a sign, a book): what kind and what it says.
    pub reading: Option<(arx_script::NoteKind, String)>,
    /// Which bag of the inventory is shown.
    pub bag: u8,
    /// Slide position of the backpack panel (0 = fully up, [`BAG_HIDDEN`] = away).
    pub bag_slide: f32,
    /// Slide position of the chest panel (0 = fully in, [`PANEL_HIDDEN`] = away).
    pub panel_slide: f32,
    pub drag: Option<Drag>,
    /// The last click on an item, to recognise double clicks.
    pub last_click: Option<(EntityId, f64)>,
    /// The mouse cursor is free to use the interface (the world is not being looked around).
    pub cursor_mode: bool,
    /// The cursor is over some part of the interface (set by the interface each frame).
    pub over_hud: bool,
    /// Scale of the interface relative to the original's 640x480 layout.
    pub scale: f32,
    pub hud_scale: f32,
    /// The keyboard chose the highlighted item (cleared by the mouse).
    pub kbd: bool,
    /// The developer overlay (position, help line) is shown (`F3`).
    pub debug: bool,
    /// The open page of the player's book.
    /// Where the dragged item would go if let go now (while it is dragged over the world).
    pub drag_spot: Option<arx_physics::items::DragSpot>,
    /// The item lying in the world under the cursor.
    pub hover_item: Option<EntityId>,
    /// Interface sounds to play (`sfx/<name>.wav`).
    pub sfx: Vec<&'static str>,
    pub book: Option<crate::hud_book::BookPage>,
    /// First page (always even) of the note or quest log shown.
    pub note_page: usize,
    pub quest_page: usize,
    /// Character creation: only the book's character sheet is shown, and it stays open.
    pub creating: bool,
    /// `Ctrl` is held: the mouse draws runes and does nothing else.
    pub casting: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            open: false,
            selected: 0,
            held: None,
            container_selected: 0,
            reading: None,
            bag: 0,
            bag_slide: BAG_HIDDEN,
            panel_slide: PANEL_HIDDEN,
            drag: None,
            last_click: None,
            cursor_mode: false,
            over_hud: false,
            scale: 1.0,
            hud_scale: 0.5,
            kbd: false,
            debug: false,
            drag_spot: None,
            hover_item: None,
            sfx: Vec::new(),
            book: None,
            note_page: 0,
            quest_page: 0,
            creating: false,
            casting: false,
        }
    }
}

/// The class whose inventory picture an item shows: its own, or the one its script chose (`tweak icon`, as every
/// rune stone does).
pub fn icon_class(s: &Scripting, id: EntityId) -> String {
    let class = &s.world.entity(id).class;
    match s.host.state(id).and_then(|st| st.icon.as_ref()) {
        Some(icon) => format!("{}/{icon}", class.rsplit_once('/').map_or("", |(dir, _)| dir)),
        None => class.clone(),
    }
}

/// The text shown for an entity: its localised name, or its id if the scripts never named it.
pub fn display_name(s: &Scripting, speech: &Speech, id: EntityId) -> String {
    match s.host.state(id).map(|st| st.name.as_str()).filter(|n| !n.is_empty()) {
        Some(name) => speech.text(name),
        None => s.world.entity(id).id_string.clone(),
    }
}

/// Where something dropped by the player lands: a little in front of their feet (Arx coordinates).
pub fn drop_position(fly: &Fly, cam: &Transform) -> [f32; 3] {
    let forward = Vec3::new(cam.forward().x, 0.0, cam.forward().z).normalize_or_zero();
    let feet = if fly.walk { fly.player.feet } else { fly.pos - Vec3::Y * arx_physics::EYE_HEIGHT };
    let mut at = feet + forward * 90.0;
    // It lands on the floor there.
    if let Some(floor) = fly.world.as_ref().and_then(|w| w.floor_height(at.x, at.z, feet.y + 80.0)) {
        at.y = floor;
    }
    to_bevy(at.into())
}

/// Headless testing aid (`--pickup ids`, `--life n`, `--open-chest id`): set up the player shortly after start-up.
pub fn debug_pickup(
    mut frames: Local<u32>,
    args: Res<crate::LevelArgs>,
    fly: Res<Fly>,
    cam: Single<&Transform, (With<Camera3d>, Without<crate::book_hero::BookCamera>)>,
    mut s: ResMut<Scripting>,
    mut ui: ResMut<Ui>,
) {
    *frames += 1;
    let s = &mut *s;
    let player = s.player;
    if *frames == 40 {
        for name in &args.give_and_drop {
            if let Some(id) = s.world.find(name, player) {
                let at = drop_position(&fly, &cam);
                eprintln!("drop {name}: {}", inventory::drop_item(&mut s.world, &mut s.host, player, id, at));
            }
        }
        return;
    }
    if *frames != 20 {
        return;
    }
    for name in &args.give_and_drop {
        if let Some(id) = s.world.find(name, player) {
            eprintln!("give {name}: {:?}", s.host.carry(&s.world, id));
        }
    }
    if let Some(life) = args.life {
        s.host.player.life.current = life.clamp(0.0, s.host.player.life.max);
    }
    if let Some(name) = &args.open_container {
        match s.world.find(name, player) {
            Some(id) => eprintln!("open {name}: {}", inventory::open_container(&mut s.world, &mut s.host, player, id)),
            None => eprintln!("no such entity: {name}"),
        }
    }
    for name in &args.pickup {
        match s.world.find(name, player) {
            Some(id) => eprintln!("pickup {name}: {:?}", inventory::pick_up(&mut s.world, &mut s.host, player, id)),
            None => eprintln!("no such entity: {name}"),
        }
    }
    if args.gold > 0 {
        s.host.player.gold += args.gold;
    }
    if args.show_inventory {
        ui.open = true;
    }
    if args.xp > 0 {
        s.host.player.add_xp(args.xp);
    }
    if let Some(page) = &args.show_book {
        ui.book = Some(if page == "quests" { crate::hud_book::BookPage::Quests } else { crate::hud_book::BookPage::Stats });
    }
}

/// Keyboard: `I` opens the backpack; Up/Down choose, Enter uses (or combines with the held item), `H` holds an item
/// to use on what you look at, `G` drops. With a chest open it has the keys instead: Up/Down, Enter takes, `T` takes
/// all, Backspace closes. (The mouse does all of this too; see [`crate::hud_ui`].)
pub fn input(
    keys: Res<ButtonInput<KeyCode>>,
    fly: Res<Fly>,
    cam: Single<&Transform, (With<Camera3d>, Without<crate::book_hero::BookCamera>)>,
    mut ui: ResMut<Ui>,
    mut s: ResMut<Scripting>,
    speech: Res<Speech>,
) {
    let s = &mut *s;
    if keys.just_pressed(KeyCode::F3) {
        ui.debug = !ui.debug;
    }
    if keys.just_pressed(KeyCode::KeyB) && ui.reading.is_none() {
        let was_open = ui.book.is_some();
        ui.sfx.push(if was_open { "book_close" } else { "book_open" });
        ui.book = if was_open { None } else { Some(crate::hud_book::BookPage::Stats) };
    }
    // Things to read (notes, signs) come first; any of these keys puts them away.
    for note in s.host.take_notes() {
        ui.reading = Some((note.kind, speech.text(&note.text)));
        ui.note_page = 0;
        ui.book = None;
    }
    if ui.reading.is_some() {
        if keys.any_just_pressed([KeyCode::Escape, KeyCode::Enter, KeyCode::Space, KeyCode::KeyE, KeyCode::Backspace]) {
            ui.reading = None;
        }
        return;
    }
    if keys.just_pressed(KeyCode::KeyI) {
        ui.open = !ui.open;
        ui.sfx.push("interface_backpack");
    }
    // An open container (chest, corpse) has the keyboard until it is closed or the player walks away.
    if let Some(container) = s.host.open_container {
        ui.open = true; // so there is somewhere to put things
        let (p, q) = (s.world.entity(s.player).pos, s.world.entity(container).pos);
        let far = (p[0] - q[0]).hypot(p[2] - q[2]) > 450.0;
        let player = s.player;
        if far || keys.just_pressed(KeyCode::Backspace) {
            inventory::close_container(&mut s.world, &mut s.host, player);
            return;
        }
        let list = s.host.containers.get(&container).cloned().unwrap_or_default();
        ui.container_selected = ui.container_selected.min(list.len().saturating_sub(1));
        if keys.just_pressed(KeyCode::ArrowDown) && !list.is_empty() {
            ui.container_selected = (ui.container_selected + 1) % list.len();
        }
        if keys.just_pressed(KeyCode::ArrowUp) && !list.is_empty() {
            ui.container_selected = (ui.container_selected + list.len() - 1) % list.len();
        }
        if keys.just_pressed(KeyCode::Enter)
            && let Some(&item) = list.get(ui.container_selected)
        {
            take(s, &speech, container, item);
        }
        if keys.just_pressed(KeyCode::KeyT) && !list.is_empty() {
            take_all(s, container);
        }
        return;
    }
    if !ui.open {
        return;
    }
    let len = s.host.player.inventory.len();
    if keys.just_pressed(KeyCode::ArrowDown) && len > 0 {
        ui.selected = (ui.selected + 1) % len;
        ui.kbd = true;
    }
    if keys.just_pressed(KeyCode::ArrowUp) && len > 0 {
        ui.selected = (ui.selected + len - 1) % len;
        ui.kbd = true;
    }
    let Some(&item) = s.host.player.inventory.get(ui.selected) else { return };
    let player = s.player;
    if keys.just_pressed(KeyCode::KeyH) {
        ui.held = if ui.held == Some(item) { None } else { Some(item) };
    }
    if keys.just_pressed(KeyCode::Enter) {
        match ui.held.filter(|&h| h != item) {
            Some(held) => {
                inventory::combine(&mut s.world, &mut s.host, player, held, item);
            }
            None => {
                inventory::use_item(&mut s.world, &mut s.host, player, item);
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyG) {
        let at = drop_position(&fly, &cam);
        let name = display_name(s, &speech, item);
        if inventory::drop_item(&mut s.world, &mut s.host, player, item, at) {
            s.host.push_message(format!("Dropped {name}"));
        }
    }
    s.host.prune_inventory();
    let len = s.host.player.inventory.len();
    ui.selected = ui.selected.min(len.saturating_sub(1));
    if ui.held.is_some_and(|h| !s.host.player.inventory.contains(&h)) {
        ui.held = None;
    }
}

/// Take one item out of a container, telling the player how it went.
pub fn take(s: &mut Scripting, speech: &Speech, container: EntityId, item: EntityId) {
    let name = display_name(s, speech, item);
    let player = s.player;
    match inventory::take_from_container(&mut s.world, &mut s.host, player, container, item) {
        Some(inventory::PickUp::Gold(n)) => s.host.push_message(format!("{n} gold")),
        Some(inventory::PickUp::Refused(_)) => s.host.push_message("Your inventory is full".to_owned()),
        Some(_) => s.host.push_message(format!("Took {name}")),
        None => {}
    }
}

pub fn take_all(s: &mut Scripting, container: EntityId) {
    let player = s.player;
    let n = inventory::take_all(&mut s.world, &mut s.host, player, container);
    let left = s.host.containers.get(&container).map_or(0, Vec::len);
    s.host.push_message(if left > 0 { "Your inventory is full".to_owned() } else { format!("Took {n} things") });
}
