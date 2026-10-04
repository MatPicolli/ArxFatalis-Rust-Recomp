//! Letting go of a dragged item in the world: use it on what is under the cursor, or put it down, drop it or throw
//! it, and move the items that were dropped or thrown until they come to rest.

use crate::convert::to_bevy;
use crate::hud::Ui;
use crate::scripting::{Pickables, Scripting, pick_ray};
use crate::Fly;
use arx_level::inventory;
use arx_physics::items::{DragStatus, ItemBody};
use arx_script::{EntityId, EntityKind};
use bevy::prelude::*;

/// How far the cursor can pick something up from.
pub const PICK_REACH: f32 = 400.0;
/// Speed of a thrown item, units per second.
const THROW_SPEED: f32 = 700.0;

/// Items in flight.
#[derive(Resource, Default)]
pub struct ItemBodies(pub Vec<(EntityId, ItemBody)>);

/// The item `item`, being dragged in the world, is let go. `carried` says it came out of the inventory.
#[allow(clippy::too_many_arguments)]
pub fn release_in_world(
    ui: &mut Ui,
    s: &mut Scripting,
    fly: &Fly,
    bodies: &mut ItemBodies,
    item: EntityId,
    ray: Option<Ray3d>,
    pickables: &Pickables,
    carried: bool,
) {
    let player = s.player;
    // Dropped on something that can use it (a key on a door): that is what happens.
    if let Some(r) = ray
        && let Some((_, target)) = pick_ray(pickables, s, r.origin, *r.direction, PICK_REACH)
        && target != item
        && s.world.entity(target).kind != EntityKind::Item
        && carried
    {
        s.host.modify(item, |st| st.hidden = true);
        inventory::combine(&mut s.world, &mut s.host, player, item, target);
        return;
    }
    let Some(spot) = ui.drag_spot else {
        // No spot (no cursor ray): just put it back.
        if carried {
            s.host.modify(item, |st| st.hidden = true);
        }
        return;
    };
    let at = to_bevy(spot.pos.to_array());
    if carried {
        inventory::drop_item(&mut s.world, &mut s.host, player, item, at);
    }
    s.host.modify(item, |st| st.moved_to = Some(at));
    s.world.entity_mut(item).pos = at;
    match spot.status {
        DragStatus::OnGround => ui.sfx.push("interface_invstd"),
        DragStatus::Drop => {
            bodies.0.push((item, ItemBody::thrown(spot.pos, Vec3::new(0.0, 20.0, 0.0))));
            ui.sfx.push("whoosh07");
        }
        DragStatus::Throw => {
            // From the player's hand toward where the cursor pointed.
            let eye = fly.pos;
            let dir = (spot.pos - eye).normalize_or_zero();
            let start = eye + dir * 40.0 - Vec3::Y * 40.0;
            bodies.0.push((item, ItemBody::thrown(start, dir * THROW_SPEED)));
            ui.sfx.push("whoosh07");
        }
        DragStatus::Invalid => {}
    }
}

/// Move thrown and dropped items until they rest; whatever the player picks up meanwhile is let be.
pub fn step_bodies(time: Res<Time>, fly: Res<Fly>, mut bodies: ResMut<ItemBodies>, mut s: ResMut<Scripting>) {
    let Some(world) = fly.world.as_ref() else { return };
    let dt = time.delta_secs();
    let s = &mut *s;
    bodies.0.retain_mut(|(id, body)| {
        let loose = s.host.state(*id).is_some_and(|st| !st.in_inventory && !st.hidden && !st.destroyed);
        if !loose {
            return false;
        }
        body.step(world, dt);
        let at = to_bevy(body.pos.to_array());
        s.host.modify(*id, |st| st.moved_to = Some(at));
        s.world.entity_mut(*id).pos = at;
        !body.resting
    });
}

/// Headless testing aid (`--throw-test id[:pitch]`): at frame 30 the player takes the item, at 60 it is dragged out
/// along the view direction, at 90 it is let go (see [`release_in_world`]).
#[allow(clippy::too_many_arguments)]
pub fn debug_throw(
    mut frames: Local<u32>,
    args: Res<crate::LevelArgs>,
    fly: Res<Fly>,
    mut ui: ResMut<Ui>,
    mut s: ResMut<Scripting>,
    mut bodies: ResMut<ItemBodies>,
    pickables: Res<Pickables>,
) {
    let Some(spec) = args.throw_test.as_deref() else { return };
    *frames += 1;
    let (name, pitch) = spec.split_once(':').map_or((spec, -15.0f32), |(n, p)| (n, p.parse().unwrap_or(-15.0)));
    let s = &mut *s;
    let player = s.player;
    let Some(item) = s.world.find(name, player) else { return };
    match *frames {
        30 => {
            eprintln!("take {name}: {:?}", s.host.carry(&s.world, item));
        }
        60 => {
            let (yaw, pitch) = (fly.yaw, pitch.to_radians());
            let dir = Vec3::new(-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
            if let Some(world) = fly.world.as_ref() {
                let spot = arx_physics::items::drag_spot(world, fly.pos, dir, fly.pos, 20.0);
                eprintln!("dragging {name}: {spot:?}");
                let at = to_bevy(spot.pos.to_array());
                s.host.modify(item, |st| {
                    st.hidden = false;
                    st.moved_to = Some(at);
                });
                s.host.note_dropped(item);
                ui.drag = Some(crate::hud::Drag { item, grab: Vec2::ZERO, from_world: false, spawned: true });
                ui.drag_spot = Some(spot);
            }
        }
        90 => {
            ui.drag = None;
            eprintln!("let go of {name}");
            release_in_world(&mut ui, s, &fly, &mut bodies, item, None, &pickables, true);
        }
        _ => {}
    }
}
