//! The player's on-screen state: life and mana bars, the inventory panel and its controls.

use crate::convert::to_bevy;
use crate::scripting::Scripting;
use crate::speech::Speech;
use crate::Fly;
use arx_level::inventory;
use arx_script::EntityId;
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct Ui {
    pub open: bool,
    pub selected: usize,
    /// Item chosen to be used on something: another item, or what the player looks at (`E`).
    pub held: Option<EntityId>,
}

#[derive(Component)]
pub struct LifeFill;
#[derive(Component)]
pub struct ManaFill;
#[derive(Component)]
pub struct StatsText;
#[derive(Component)]
pub struct InventoryText;

const BAR_WIDTH: f32 = 220.0;
const BAR_HEIGHT: f32 = 16.0;

fn bar(commands: &mut Commands, left: Option<f32>, right: Option<f32>, fill: impl Component, color: Color) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(18.0),
                left: left.map_or(Val::Auto, Val::Px),
                right: right.map_or(Val::Auto, Val::Px),
                width: Val::Px(BAR_WIDTH),
                height: Val::Px(BAR_HEIGHT),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_children(|p| {
            p.spawn((fill, Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, BackgroundColor(color)));
        });
}

pub fn spawn(mut commands: Commands) {
    bar(&mut commands, Some(18.0), None, LifeFill, Color::srgb(0.75, 0.1, 0.1));
    bar(&mut commands, None, Some(18.0), ManaFill, Color::srgb(0.15, 0.3, 0.85));
    commands.spawn((
        StatsText,
        Text::new(""),
        TextFont { font_size: FontSize::Px(14.0), ..default() },
        TextColor(Color::WHITE),
        Node { position_type: PositionType::Absolute, bottom: Val::Px(38.0), left: Val::Px(18.0), ..default() },
    ));
    commands.spawn((
        InventoryText,
        Text::new(""),
        TextFont { font_size: FontSize::Px(17.0), ..default() },
        TextColor(Color::srgb(0.95, 0.92, 0.8)),
        BackgroundColor(Color::srgba(0.05, 0.04, 0.03, 0.8)),
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(90.0),
            right: Val::Px(18.0),
            width: Val::Px(380.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
    ));
}

/// The text shown for an entity: its localised name, or its id if the scripts never named it.
pub fn display_name(s: &Scripting, speech: &Speech, id: EntityId) -> String {
    match s.host.state(id).map(|st| st.name.as_str()).filter(|n| !n.is_empty()) {
        Some(name) => speech.text(name),
        None => s.world.entity(id).id_string.clone(),
    }
}

/// Headless testing aid (`--pickup ids`, `--life n`): set up the player shortly after start-up.
pub fn debug_pickup(mut frames: Local<u32>, args: Res<crate::LevelArgs>, mut s: ResMut<Scripting>) {
    *frames += 1;
    if *frames != 20 {
        return;
    }
    let s = &mut *s;
    let player = s.player;
    if let Some(life) = args.life {
        s.host.player.life.current = life.clamp(0.0, s.host.player.life.max);
    }
    for name in &args.pickup {
        match s.world.find(name, player) {
            Some(id) => eprintln!("pickup {name}: {:?}", inventory::pick_up(&mut s.world, &mut s.host, player, id)),
            None => eprintln!("no such entity: {name}"),
        }
    }
}

/// Inventory keys: `I` opens it; then Up/Down choose, Enter uses (or combines with the held item), `H` holds
/// an item to use on what you look at, `G` drops.
pub fn input(
    keys: Res<ButtonInput<KeyCode>>,
    fly: Res<Fly>,
    cam: Single<&Transform, With<Camera3d>>,
    mut ui: ResMut<Ui>,
    mut s: ResMut<Scripting>,
    speech: Res<Speech>,
) {
    let s = &mut *s;
    if keys.just_pressed(KeyCode::KeyI) {
        ui.open = !ui.open;
    }
    if !ui.open {
        return;
    }
    let len = s.host.player.inventory.len();
    if keys.just_pressed(KeyCode::ArrowDown) && len > 0 {
        ui.selected = (ui.selected + 1) % len;
    }
    if keys.just_pressed(KeyCode::ArrowUp) && len > 0 {
        ui.selected = (ui.selected + len - 1) % len;
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
        let forward = Vec3::new(cam.forward().x, 0.0, cam.forward().z).normalize_or_zero();
        let feet = if fly.walk { fly.player.feet } else { fly.pos - Vec3::Y * arx_physics::EYE_HEIGHT };
        let at = to_bevy((feet + forward * 90.0).into());
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

#[allow(clippy::too_many_arguments)]
pub fn update(
    ui: Res<Ui>,
    s: Res<Scripting>,
    speech: Res<Speech>,
    mut life: Single<&mut Node, (With<LifeFill>, Without<ManaFill>)>,
    mut mana: Single<&mut Node, (With<ManaFill>, Without<LifeFill>)>,
    mut stats: Single<&mut Text, (With<StatsText>, Without<InventoryText>)>,
    mut inv: Single<(&mut Text, &mut Visibility), (With<InventoryText>, Without<StatsText>)>,
) {
    let p = &s.host.player;
    life.width = Val::Percent(p.life.fraction() * 100.0);
    mana.width = Val::Percent(p.mana.fraction() * 100.0);
    stats.0 = format!(
        "Life {:.0}/{:.0}     Mana {:.0}/{:.0}     Hunger {:.0}%     [I] inventory",
        p.life.current, p.life.max, p.mana.current, p.mana.max, p.hunger
    );
    let (text, vis) = &mut *inv;
    **vis = if ui.open { Visibility::Inherited } else { Visibility::Hidden };
    if !ui.open {
        return;
    }
    let mut out = String::from("Inventory   (Up/Down choose, Enter use, H hold, G drop)\n");
    if p.inventory.is_empty() {
        out.push_str("\n  empty - walk up to an item and press E");
    }
    for (i, &id) in p.inventory.iter().enumerate() {
        let count = s.host.state(id).map_or(1, |st| st.count);
        let held = if ui.held == Some(id) { "  [held: press E on something]" } else { "" };
        let n = if count > 1 { format!(" x{count}") } else { String::new() };
        out.push_str(&format!("\n{} {}{n}{held}", if i == ui.selected { ">" } else { " " }, display_name(&s, &speech, id)));
    }
    text.0 = out;
}
