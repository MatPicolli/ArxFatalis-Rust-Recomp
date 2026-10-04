//! The original interface, laid out like ArxLibertatis's `gui/Hud.cpp` (all sizes are the original's 640x480 pixels
//! times the interface scale):
//!
//! - the life and mana gauges in the bottom corners (a filled gauge shows through the empty gauge's frame),
//! - above the mana gauge, bottom to top: the backpack, the spell book and, once you have gold, the purse,
//! - the backpack panel (`hero_inventory`) sliding up from the bottom centre, a 16x3 grid of 32-pixel slots with the
//!   items' own icons and counts in the original's digit font,
//! - the chest panel (`ingame_inventory_*`) sliding in from the left with its pick-all and close buttons,
//! - the crosshair while looking around and the original cursors while using the interface.
//!
//! Everything is rebuilt as plain UI image nodes every frame (there are only a few dozen). [`geometry`] is the one
//! place that knows where things are, so drawing and mouse handling cannot disagree.

use crate::hud::{BAG_HIDDEN, Drag, PANEL_HIDDEN, Ui, display_name, drop_position};
use crate::scripting::Scripting;
use crate::speech::Speech;
use crate::{Arx, Fly};
use arx_level::inventory;
use arx_script::{BAG_HEIGHT, BAG_WIDTH, EntityId, is_gold_class};
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::HashMap;

/// A double click is two clicks this close together (seconds).
const DOUBLE_CLICK: f64 = 0.4;
/// Chest panel size and its slot grid, in unscaled pixels.
const PANEL_SIZE: Vec2 = Vec2::new(115.0, 378.0);
const PANEL_COLUMNS: u8 = 3;
const PANEL_ROWS: u8 = 11;
const BAG_SIZE: Vec2 = Vec2::new(562.0, 121.0);

#[derive(Component)]
pub struct HudNode;

#[derive(Clone)]
pub struct UiTex {
    pub handle: Handle<Image>,
    /// Size in pixels.
    pub size: Vec2,
}

/// Interface bitmaps from the game files, loaded on first use. Like the original, black is transparent.
#[derive(Resource, Default)]
pub struct UiAssets {
    cache: HashMap<String, Option<UiTex>>,
}

fn load(arx: &Arx, images: &mut Assets<Image>, path: &str) -> Option<UiTex> {
    let bytes = arx.0.read(path).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Bmp).ok()?;
    let (w, h) = (img.width(), img.height());
    let mut rgba = img.into_rgba8().into_raw();
    for px in rgba.chunks_exact_mut(4) {
        if px[..3] == [0, 0, 0] {
            px[3] = 0;
        }
    }
    let mut image = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    Some(UiTex { handle: images.add(image), size: Vec2::new(w as f32, h as f32) })
}

impl UiAssets {
    fn get(&mut self, arx: &Arx, images: &mut Assets<Image>, name: &str) -> Option<UiTex> {
        let path = format!("graph/interface/{name}.bmp");
        self.cache.entry(path.clone()).or_insert_with(|| load(arx, images, &path)).clone()
    }

    /// An item's inventory icon. Gold shows more coins the more there is; items without an icon get the question mark.
    fn icon(&mut self, arx: &Arx, images: &mut Assets<Image>, class: &str, count: u32) -> Option<UiTex> {
        let path = if is_gold_class(class) {
            let n = match count {
                0..=3 => count.max(1) as usize - 1,
                4..=8 => 3,
                9..=20 => 4,
                21..=50 => 5,
                _ => 6,
            };
            let base = class.trim_end_matches("gold_coin");
            if n == 0 { format!("{base}gold_coin[icon].bmp") } else { format!("{base}gold_coin{}[icon].bmp", n + 1) }
        } else {
            format!("{class}[icon].bmp")
        };
        let found = self.cache.entry(path.clone()).or_insert_with(|| load(arx, images, &path)).clone();
        found.or_else(|| self.get(arx, images, "misc/default[icon]"))
    }
}

/// The interface scale of the original (`getInterfaceScale`): `factor` of the largest scale that fits 640x480,
/// rounded to whole or half steps.
pub fn interface_scale(width: f32, height: f32, factor: f32) -> f32 {
    let max = (width / 640.0).min(height / 480.0);
    let scale = 1.0f32.max(max * factor).min(max);
    if max > 1.0 {
        if scale < 1.3 || max < 1.5 {
            return 1.0;
        }
        if scale < 1.75 || max < 2.0 {
            return 1.5;
        }
        return (scale + 0.5).min(max).floor();
    }
    scale
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, x + w, y + h)
}

/// Where everything is, in logical window pixels.
pub struct Geo {
    pub s: f32,
    pub health: Rect,
    pub mana: Rect,
    pub backpack: Rect,
    pub book: Rect,
    pub purse: Rect,
    /// Top-left of the backpack panel's slot (0, 0).
    pub bag_slots: Vec2,
    pub bag: Rect,
    pub arrow_up: Rect,
    pub arrow_down: Rect,
    pub panel: Rect,
    pub pick_all: Rect,
    pub close: Rect,
}

pub fn geometry(w: f32, h: f32, ui: &Ui) -> Geo {
    let s = ui.scale;
    let health = rect(0.0, h - 80.0 * s + 2.0 * s, 33.0 * s, 80.0 * s);
    let mana = rect(w - 33.0 * s, h - 80.0 * s, 33.0 * s, 80.0 * s);
    // The icons stack up from the mana gauge, 3 pixels apart.
    let icon = |above: f32| rect(w - 32.0 * s, above - 3.0 - 32.0 * s, 32.0 * s, 32.0 * s);
    let backpack = icon(mana.min.y);
    let book = icon(backpack.min.y);
    let purse = icon(book.min.y);
    let anchor = Vec2::new(w / 2.0 - 320.0 * s + 35.0 * s, h - 101.0 * s + ui.bag_slide * s);
    let bag = rect(anchor.x, anchor.y - 5.0 * s, BAG_SIZE.x * s, BAG_SIZE.y * s);
    let arrows = anchor + Vec2::new((BAG_SIZE.x - 35.0) * s, 22.0 * s);
    let panel = rect(ui.panel_slide * s, 0.0, PANEL_SIZE.x * s, PANEL_SIZE.y * s);
    Geo {
        s,
        health,
        mana,
        backpack,
        book,
        purse,
        bag_slots: anchor + Vec2::new(7.0, 6.0) * s,
        bag,
        arrow_up: rect(arrows.x, arrows.y, 32.0 * s, 32.0 * s),
        arrow_down: rect(arrows.x, arrows.y + 37.0 * s, 32.0 * s, 32.0 * s),
        pick_all: rect(panel.min.x + 16.0 * s, panel.max.y - 16.0 * s, 16.0 * s, 16.0 * s),
        close: rect(panel.max.x - 32.0 * s, panel.max.y - 16.0 * s, 16.0 * s, 16.0 * s),
        panel,
    }
}

impl Geo {
    fn slot_rect(&self, x: u8, y: u8, w: u8, h: u8) -> Rect {
        let o = self.bag_slots + Vec2::new(x as f32, y as f32) * 32.0 * self.s;
        Rect::from_corners(o, o + Vec2::new(w as f32, h as f32) * 32.0 * self.s)
    }

    fn panel_slot_rect(&self, x: u8, y: u8, w: u8, h: u8) -> Rect {
        let o = Vec2::new(self.panel.min.x + 2.0 * self.s, 13.0 * self.s) + Vec2::new(x as f32, y as f32) * 32.0 * self.s;
        Rect::from_corners(o, o + Vec2::new(w as f32, h as f32) * 32.0 * self.s)
    }

    /// The bag panel is on screen (it slides in and out).
    fn bag_visible(&self, ui: &Ui) -> bool {
        ui.bag_slide < BAG_HIDDEN - 1.0
    }

    fn panel_visible(&self, ui: &Ui) -> bool {
        ui.panel_slide > PANEL_HIDDEN + 1.0
    }
}

/// Places for what a container holds, in the engine's order (row by row, first fit): item, column, row, size.
fn container_layout(s: &Scripting, container: EntityId) -> Vec<(EntityId, u8, u8, u8, u8)> {
    let mut used = [[false; PANEL_COLUMNS as usize]; PANEL_ROWS as usize];
    let mut out = Vec::new();
    for &item in s.host.containers.get(&container).map_or(&[][..], Vec::as_slice) {
        let (w, h) = s.host.item_slots(&s.world.entity(item).class);
        let fits = |used: &[[bool; 3]; 11], x: u8, y: u8| {
            x + w <= PANEL_COLUMNS && y + h <= PANEL_ROWS && (0..h).all(|dy| (0..w).all(|dx| !used[(y + dy) as usize][(x + dx) as usize]))
        };
        let spot = (0..PANEL_ROWS).flat_map(|y| (0..PANEL_COLUMNS).map(move |x| (x, y))).find(|&(x, y)| fits(&used, x, y));
        if let Some((x, y)) = spot {
            for dy in 0..h {
                for dx in 0..w {
                    used[(y + dy) as usize][(x + dx) as usize] = true;
                }
            }
            out.push((item, x, y, w, h));
        }
    }
    out
}

/// Draw list for a frame.
enum Item {
    Image { tex: Handle<Image>, at: Rect, part: Option<Rect>, tint: Color },
    Text { text: String, at: Vec2, size: f32, color: Color },
}

#[derive(Default)]
struct Canvas(Vec<Item>);

impl Canvas {
    fn image(&mut self, tex: &UiTex, at: Rect) {
        self.tinted(tex, at, Color::WHITE);
    }

    fn tinted(&mut self, tex: &UiTex, at: Rect, tint: Color) {
        self.0.push(Item::Image { tex: tex.handle.clone(), at, part: None, tint });
    }

    /// Draw an icon that is `pixels` big at `scale`, top-left at `pos`.
    fn sized(&mut self, tex: &UiTex, pos: Vec2, scale: f32, tint: Color) -> Rect {
        let at = Rect::from_corners(pos, pos + tex.size * scale);
        self.tinted(tex, at, tint);
        at
    }

    /// Numbers in the original's bitmap digits, drawn right to left from `right_top` (`ARX_INTERFACE_DrawNumber`).
    fn number(&mut self, font: &UiTex, right_top: Vec2, mut n: u64, scale: f32) {
        let mut x = right_top.x;
        while n != 0 {
            let digit = (n % 10) as f32;
            n /= 10;
            x -= 10.0 * scale;
            let u = digit * 11.0 + 1.5;
            self.0.push(Item::Image {
                tex: font.handle.clone(),
                at: rect(x, right_top.y, 10.0 * scale, 10.0 * scale),
                part: Some(Rect::new(u, 1.5, u + 10.0, 11.5)),
                tint: Color::WHITE,
            });
        }
    }

    fn flush(self, commands: &mut Commands) {
        for item in self.0 {
            match item {
                Item::Image { tex, at, part, tint } => {
                    commands.spawn((HudNode, node(at), ImageNode { image: tex, color: tint, rect: part, ..default() }));
                }
                Item::Text { text, at, size, color } => {
                    commands.spawn((
                        HudNode,
                        Text::new(text),
                        TextFont { font_size: FontSize::Px(size), ..default() },
                        TextColor(color),
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
                        Node { position_type: PositionType::Absolute, left: Val::Px(at.x), top: Val::Px(at.y), padding: UiRect::axes(Val::Px(4.0), Val::Px(1.0)), ..default() },
                    ));
                }
            }
        }
    }
}

fn node(at: Rect) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(at.min.x),
        top: Val::Px(at.min.y),
        width: Val::Px(at.width()),
        height: Val::Px(at.height()),
        ..default()
    }
}

/// Brighter than white: the original draws hovered icons a second time additively.
fn bright() -> Color {
    Color::linear_rgb(1.9, 1.9, 1.9)
}

/// Mouse use of the interface: backpack and book icons, the bag (drag items around, double click to use, drop on
/// another item or on the world to use it there), the chest panel (click an item to take it).
#[allow(clippy::too_many_arguments)]
pub fn mouse(
    time: Res<Time>,
    window: Single<&Window>,
    buttons: Res<ButtonInput<MouseButton>>,
    shot: Option<Res<crate::Shot>>,
    fly: Res<Fly>,
    cam: Single<&Transform, With<Camera3d>>,
    speech: Res<Speech>,
    mut ui: ResMut<Ui>,
    mut s: ResMut<Scripting>,
) {
    ui.scale = interface_scale(window.width(), window.height(), ui.hud_scale);
    ui.over_hud = false;
    let s = &mut *s;
    let (w, h) = (window.width(), window.height());
    let g = geometry(w, h, &ui);
    // Screenshot runs ignore the real mouse.
    let Some(pos) = window.cursor_position().filter(|_| shot.is_none()) else { return };
    let player = s.player;

    // Is the cursor on part of the interface?
    let in_bag = g.bag_visible(&ui) && g.bag.contains(pos);
    let in_panel = s.host.open_container.is_some() && g.panel_visible(&ui) && g.panel.contains(pos);
    ui.over_hud = in_bag || in_panel || [g.backpack, g.book, g.health, g.mana].iter().any(|r| r.contains(pos)) || (s.host.player.gold > 0 && g.purse.contains(pos));

    // Which bag slot is under the cursor?
    let cell = |g: &Geo| {
        let rel = (pos - g.bag_slots) / (32.0 * g.s);
        (rel.x >= 0.0 && rel.y >= 0.0 && rel.x < BAG_WIDTH as f32 && rel.y < BAG_HEIGHT as f32).then(|| (rel.x as u8, rel.y as u8))
    };

    if buttons.just_pressed(MouseButton::Left) {
        ui.kbd = false;
        if g.backpack.contains(pos) {
            ui.open = !ui.open;
        } else if g.book.contains(pos) {
            s.host.push_message("The spell book is not available yet".to_owned());
        } else if g.health.contains(pos) {
            let n = s.host.player.life.current as i64;
            s.host.push_message(n.to_string());
        } else if g.mana.contains(pos) {
            let n = s.host.player.mana.current as i64;
            s.host.push_message(n.to_string());
        } else if g.bag_visible(&ui) && ui.bag > 0 && g.arrow_up.contains(pos) {
            ui.bag -= 1;
        } else if g.bag_visible(&ui) && (ui.bag as usize) + 1 < s.host.player.bags && g.arrow_down.contains(pos) {
            ui.bag += 1;
        } else if in_bag {
            if let Some((x, y)) = cell(&g)
                && let Some(item) = s.host.player.item_at(ui.bag, x, y)
            {
                let slot = s.host.player.slots[&item];
                let top_left = g.slot_rect(slot.x, slot.y, slot.w, slot.h).min;
                let now = time.elapsed_secs_f64();
                if ui.last_click.is_some_and(|(id, t)| id == item && now - t < DOUBLE_CLICK) {
                    ui.last_click = None;
                    inventory::use_item(&mut s.world, &mut s.host, player, item);
                } else {
                    ui.last_click = Some((item, now));
                    ui.drag = Some(Drag { item, grab: pos - top_left });
                }
            }
        } else if in_panel {
            let container = s.host.open_container.expect("panel is only shown for an open container");
            if g.close.contains(pos) {
                inventory::close_container(&mut s.world, &mut s.host, player);
            } else if g.pick_all.contains(pos) {
                crate::hud::take_all(s, container);
            } else {
                let hit = container_layout(s, container).into_iter().find(|&(_, x, y, w, h)| g.panel_slot_rect(x, y, w, h).contains(pos));
                if let Some((item, ..)) = hit {
                    crate::hud::take(s, &speech, container, item);
                }
            }
        }
    }

    if buttons.just_released(MouseButton::Left)
        && let Some(drag) = ui.drag.take()
    {
        if !s.host.player.inventory.contains(&drag.item) {
            return;
        }
        let name = display_name(s, &speech, drag.item);
        if in_bag {
            let under = cell(&g).and_then(|(x, y)| s.host.player.item_at(ui.bag, x, y)).filter(|&i| i != drag.item);
            if let Some(target) = under {
                // Dropped on another item: use one on the other.
                inventory::combine(&mut s.world, &mut s.host, player, drag.item, target);
            } else {
                // Moved: where the icon's top-left now is.
                let top_left = (pos - drag.grab - g.bag_slots) / (32.0 * g.s);
                let (x, y) = (top_left.x.round().max(0.0) as u8, top_left.y.round().max(0.0) as u8);
                s.host.player.move_item(drag.item, ui.bag, x, y);
            }
        } else if in_panel {
            if let Some(container) = s.host.open_container {
                inventory::store_in_container(&mut s.world, &mut s.host, player, container, drag.item);
                s.host.push_message(format!("Put {name} away"));
            }
        } else if !ui.over_hud {
            // Out in the world: use it on what you are looking at, or drop it.
            match s.target {
                Some(target) => {
                    inventory::combine(&mut s.world, &mut s.host, player, drag.item, target);
                }
                None => {
                    let at = drop_position(&fly, &cam);
                    if inventory::drop_item(&mut s.world, &mut s.host, player, drag.item, at) {
                        s.host.push_message(format!("Dropped {name}"));
                    }
                }
            }
        }
        s.host.prune_inventory();
    }
}

/// Animate the sliding panels and draw the whole interface.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    mut commands: Commands,
    time: Res<Time>,
    arx: Res<Arx>,
    window: Single<&Window>,
    old: Query<Entity, With<HudNode>>,
    mut assets: ResMut<UiAssets>,
    mut images: ResMut<Assets<Image>>,
    mut ui: ResMut<Ui>,
    s: Res<Scripting>,
    speech: Res<Speech>,
    shot: Option<Res<crate::Shot>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let (w, h) = (window.width(), window.height());
    ui.scale = interface_scale(w, h, ui.hud_scale);
    let dt = time.delta_secs().min(0.1);
    let p = &s.host.player;
    if p.is_dead() {
        return;
    }

    // The panels slide: the backpack up from the bottom, the chest in from the left.
    let target = if ui.open { 0.0 } else { BAG_HIDDEN };
    let step = 330.0 * dt;
    ui.bag_slide += (target - ui.bag_slide).clamp(-step, step);
    let target = if s.host.open_container.is_some() { 0.0 } else { PANEL_HIDDEN };
    let step = 1000.0 * dt;
    ui.panel_slide += (target - ui.panel_slide).clamp(-step, step);

    let g = geometry(w, h, &ui);
    let mut c = Canvas::default();
    let cursor = window.cursor_position().filter(|_| shot.is_none());
    let hover = |r: Rect| cursor.is_some_and(|p| r.contains(p));
    let mut tooltip: Option<(String, Vec2)> = None;

    // --- gauges: the filled gauge is cut off from the top and shows through the empty gauge's frame.
    let gauge = |c: &mut Canvas, assets: &mut UiAssets, images: &mut Assets<Image>, colour: &str, at: Rect, amount: f32, tint: Color| {
        if let (Some(filled), Some(empty)) = (assets.get(&arx, images, &format!("bars/filled_gauge_{colour}")), assets.get(&arx, images, &format!("bars/empty_gauge_{colour}"))) {
            let amount = amount.clamp(0.0, 1.0);
            let shown = Rect::new(at.min.x, at.min.y + at.height() * (1.0 - amount), at.max.x, at.max.y);
            let part = Rect::new(0.0, filled.size.y * (1.0 - amount), filled.size.x, filled.size.y);
            c.0.push(Item::Image { tex: filled.handle, at: shown, part: Some(part), tint });
            c.image(&empty, at);
        }
    };
    // The red gauge texture is grey: the engine tints it red.
    gauge(&mut c, &mut assets, &mut images, "red", g.health, p.life.fraction(), Color::srgb(1.0, 0.0, 0.0));
    gauge(&mut c, &mut assets, &mut images, "blue", g.mana, p.mana.fraction(), Color::WHITE);

    // --- the icons above the mana gauge.
    for (name, at, shown) in [("icons/backpack", g.backpack, true), ("icons/book", g.book, true), ("inventory/gold", g.purse, p.gold > 0)] {
        if !shown {
            continue;
        }
        if let Some(tex) = assets.get(&arx, &mut images, name) {
            c.tinted(&tex, at, if hover(at) { bright() } else { Color::WHITE });
        }
    }
    if p.gold > 0
        && hover(g.purse)
        && let Some(font) = assets.get(&arx, &mut images, "font/font10x10_inventory")
    {
        c.number(&font, Vec2::new(g.purse.max.x, g.purse.min.y - 15.0 * g.s), p.gold, g.s);
    }

    // --- the backpack panel.
    if g.bag_visible(&ui) {
        if let Some(tex) = assets.get(&arx, &mut images, "inventory/hero_inventory") {
            c.image(&tex, g.bag);
        }
        let font = assets.get(&arx, &mut images, "font/font10x10_inventory");
        let selected = p.inventory.get(ui.selected).copied();
        for &item in &p.inventory {
            let Some(slot) = p.slots.get(&item).filter(|sl| sl.bag == ui.bag) else { continue };
            if ui.drag.is_some_and(|d| d.item == item) {
                continue;
            }
            let st = s.host.state(item);
            let count = st.map_or(1, |st| st.count);
            let Some(tex) = assets.icon(&arx, &mut images, &s.world.entity(item).class, count) else { continue };
            let at = g.slot_rect(slot.x, slot.y, slot.w, slot.h);
            let lit = hover(at) || (ui.kbd && selected == Some(item)) || ui.held == Some(item);
            let icon = c.sized(&tex, at.min, g.s, if lit { bright() } else { Color::WHITE });
            if hover(at) {
                tooltip = Some((display_name(&s, &speech, item), cursor.unwrap_or_default()));
            }
            if count != 1
                && let Some(font) = &font
            {
                c.number(font, Vec2::new(icon.max.x, icon.min.y), count as u64, g.s);
            }
        }
        if ui.bag > 0
            && let Some(tex) = assets.get(&arx, &mut images, "inventory/scroll_up")
        {
            c.tinted(&tex, g.arrow_up, if hover(g.arrow_up) { bright() } else { Color::WHITE });
        }
        if (ui.bag as usize) + 1 < p.bags
            && let Some(tex) = assets.get(&arx, &mut images, "inventory/scroll_down")
        {
            c.tinted(&tex, g.arrow_down, if hover(g.arrow_down) { bright() } else { Color::WHITE });
        }
    }

    // --- the chest panel.
    if let (Some(container), true) = (s.host.open_container, g.panel_visible(&ui)) {
        let skin = s.host.state(container).map(|st| st.inventory_skin.clone()).filter(|k| !k.is_empty());
        let skin = skin.map_or("inventory/ingame_inventory".to_owned(), |k| format!("inventory/{k}"));
        let tex = assets.get(&arx, &mut images, &skin).or_else(|| assets.get(&arx, &mut images, "inventory/ingame_inventory"));
        if let Some(tex) = tex {
            c.image(&tex, g.panel);
        }
        let font = assets.get(&arx, &mut images, "font/font10x10_inventory");
        for (item, x, y, iw, ih) in container_layout(&s, container) {
            let count = s.host.state(item).map_or(1, |st| st.count);
            let Some(tex) = assets.icon(&arx, &mut images, &s.world.entity(item).class, count) else { continue };
            let at = g.panel_slot_rect(x, y, iw, ih);
            let lit = hover(at);
            let icon = c.sized(&tex, at.min, g.s, if lit { bright() } else { Color::WHITE });
            if lit {
                tooltip = Some((display_name(&s, &speech, item), cursor.unwrap_or_default()));
            }
            if count != 1
                && !is_gold_class(&s.world.entity(item).class)
                && let Some(font) = &font
            {
                c.number(font, Vec2::new(icon.max.x, icon.min.y), count as u64, g.s);
            }
        }
        for (name, at) in [("inventory/inv_pick", g.pick_all), ("inventory/inv_close", g.close)] {
            if let Some(tex) = assets.get(&arx, &mut images, name) {
                c.tinted(&tex, at, if hover(at) { bright() } else { Color::WHITE });
            }
        }
        // The chest's name sits above the grid.
        tooltip.get_or_insert((display_name(&s, &speech, container), Vec2::new(g.panel.min.x + 4.0 * g.s, 0.0)));
    }

    // --- what the cursor carries, the tooltip, and the cursor itself.
    if let (Some(d), Some(pos)) = (ui.drag, cursor) {
        let class = s.world.entity(d.item).class.clone();
        let count = s.host.state(d.item).map_or(1, |st| st.count);
        if let Some(tex) = assets.icon(&arx, &mut images, &class, count) {
            c.sized(&tex, pos - d.grab, g.s, Color::WHITE);
        }
    } else if let Some((text, at)) = tooltip {
        c.0.push(Item::Text { text, at: at + Vec2::new(14.0, 18.0) * g.s, size: 15.0, color: Color::srgb(0.95, 0.92, 0.8) });
    }
    if ui.cursor_mode
        && let Some(pos) = cursor
    {
        let name = if ui.drag.is_some() { None } else if ui.over_hud { Some("cursors/interaction_on") } else { Some("cursors/cursor") };
        if let Some(tex) = name.and_then(|n| assets.get(&arx, &mut images, n)) {
            c.sized(&tex, pos, g.s, Color::WHITE);
        }
    } else if let Some(tex) = assets.get(&arx, &mut images, "cursors/cruz") {
        // What the crosshair is on.
        if let Some(target) = s.target {
            let at = Vec2::new(w, h) / 2.0 + Vec2::new(14.0, 14.0) * g.s;
            c.0.push(Item::Text { text: display_name(&s, &speech, target), at, size: 15.0, color: Color::srgb(0.95, 0.92, 0.8) });
        }
        // Looking around: the crosshair, half transparent in the middle of the screen.
        let size = tex.size * g.s;
        c.tinted(&tex, Rect::from_center_size(Vec2::new(w, h) / 2.0, size), Color::srgba(1.0, 1.0, 1.0, 0.5));
    }
    c.flush(&mut commands);
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_script::{EntityKind, Script};
    use std::sync::Arc;

    #[test]
    fn the_interface_scale_follows_the_originals_rule() {
        assert_eq!(interface_scale(640.0, 480.0, 0.5), 1.0);
        assert_eq!(interface_scale(1280.0, 720.0, 0.5), 1.0, "the original default stays at its pixel size");
        assert_eq!(interface_scale(1280.0, 720.0, 1.0), 1.5);
        assert_eq!(interface_scale(1920.0, 1080.0, 1.0), 2.0);
        assert_eq!(interface_scale(3840.0, 2160.0, 1.0), 4.0);
        assert_eq!(interface_scale(320.0, 240.0, 0.5), 0.5, "a small window shrinks it");
    }

    #[test]
    fn the_hud_is_laid_out_like_the_original() {
        let ui = Ui { scale: 1.0, bag_slide: 0.0, panel_slide: 0.0, ..Ui::default() };
        let g = geometry(800.0, 600.0, &ui);
        // Gauges: life bottom-left (hiding the texture's 2 pixel gap), mana bottom-right.
        assert_eq!((g.health.min.x, g.health.max.y), (0.0, 602.0));
        assert_eq!((g.mana.max.x, g.mana.max.y), (800.0, 600.0));
        assert_eq!((g.health.width(), g.health.height()), (33.0, 80.0));
        // The icons stack up from the mana gauge with 3 pixels between them: backpack, book, purse.
        assert_eq!(g.backpack.max, Vec2::new(800.0, 520.0 - 3.0));
        assert_eq!(g.book.max.y, g.backpack.min.y - 3.0);
        assert_eq!(g.purse.max.y, g.book.min.y - 3.0);
        // The bag: 562x121, centred a little left of the screen centre, slots offset by (7, 6).
        assert_eq!(g.bag.width(), 562.0);
        assert_eq!(g.bag_slots, Vec2::new(400.0 - 320.0 + 35.0 + 7.0, 600.0 - 101.0 + 6.0));
        assert_eq!(g.slot_rect(2, 1, 1, 2).min, g.bag_slots + Vec2::new(64.0, 32.0));
        // The chest panel hugs the top-left corner; its buttons sit on the bottom edge.
        assert_eq!((g.panel.min, g.panel.size()), (Vec2::ZERO, Vec2::new(115.0, 378.0)));
        assert_eq!(g.pick_all.min, Vec2::new(16.0, 362.0));
        assert_eq!(g.close.min, Vec2::new(115.0 - 32.0, 362.0));
        assert_eq!(g.panel_slot_rect(1, 0, 1, 1).min, Vec2::new(2.0 + 32.0, 13.0));
        // At twice the size everything doubles, except the 3 pixel gaps.
        let g2 = geometry(1600.0, 1200.0, &Ui { scale: 2.0, ..Ui::default() });
        assert_eq!(g2.health.width(), 66.0);
        assert_eq!(g2.backpack.max.y, 1200.0 - 160.0 - 3.0);
    }

    #[test]
    fn a_closed_bag_is_off_screen_and_slides_in() {
        let closed = geometry(800.0, 600.0, &Ui { scale: 1.0, ..Ui::default() });
        assert!(closed.bag.min.y > 600.0 - 101.0, "mostly below the bottom edge");
        assert!(!closed.bag_visible(&Ui::default()), "the bag is hidden until opened");
        assert!(!closed.panel_visible(&Ui::default()), "the chest panel starts hidden");
    }

    #[test]
    fn chest_contents_are_placed_row_by_row() {
        let mut s = Scripting::default();
        let chest = s.world.add_entity(EntityKind::Fix, "fix_inter/chest/chest", 1, Some(Arc::new(Script::new(b"on init {\n accept\n}"))), None);
        s.host.set_icon_size(Box::new(|class| if class.contains("armor") { Some((64, 96)) } else { Some((32, 32)) }));
        let mut items = Vec::new();
        for (n, class) in ["items/gem/gem", "items/gem/gem", "items/armor/helm/helm", "items/gem/gem", "items/gem/gem"].iter().enumerate() {
            items.push(s.world.add_entity(EntityKind::Item, class, n as i32, None, None));
        }
        s.host.containers.insert(chest, items.clone());
        let l = container_layout(&s, chest);
        let at = |i: usize| l.iter().find(|e| e.0 == items[i]).map(|e| (e.1, e.2, e.3, e.4)).unwrap();
        assert_eq!(at(0), (0, 0, 1, 1));
        assert_eq!(at(1), (1, 0, 1, 1));
        // The 2x3 armour cannot start at column 2 (only one column left), so it goes to the next free row.
        assert_eq!(at(2), (0, 1, 2, 3));
        assert_eq!(at(3), (2, 0, 1, 1), "small items fill the gap the big one left");
        assert_eq!(at(4), (2, 1, 1, 1));
    }
}
