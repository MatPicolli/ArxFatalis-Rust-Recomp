//! Characters in the scene: runs their behaviour (`arx_level::npc`) and puts the models where the simulation has
//! them, facing the way it says.

use crate::convert::to_bevy;
use crate::scripting::{BaseAngle, ScriptRef, Scripting};
use crate::Fly;
use arx_level::npc::{Env, NpcWorld};
use arx_script::Skill;
use bevy::prelude::*;

/// The level's characters (`None` until the level is loaded).
#[derive(Resource, Default)]
pub struct Npcs(pub Option<NpcWorld>);

/// Move the characters on by the frame's time. They see and hear the player; the player is where the script world
/// last put them.
pub fn update(time: Res<Time>, fly: Res<Fly>, mut npcs: ResMut<Npcs>, mut s: ResMut<Scripting>) {
    let (Some(npcs), Some(world)) = (npcs.0.as_mut(), fly.world.as_ref()) else { return };
    let s = &mut *s;
    let env = Env {
        collision: world,
        player_pos: Vec3::from(s.world.entity(s.player).pos),
        player_alive: !s.host.player.is_dead(),
        player_stealth: 15.0 + s.host.player.skills.get(Skill::Stealth) / 10.0,
        player_light: 255.0,
        player_torch: false,
    };
    npcs.update(&mut s.world, &mut s.host, &env, time.delta_secs().min(0.1) * 1000.0);
}

/// Put each character's model at its simulated position and facing.
pub fn apply(npcs: Res<Npcs>, mut q: Query<(&ScriptRef, &BaseAngle, &mut Transform)>) {
    let Some(npcs) = npcs.0.as_ref() else { return };
    for (r, base, mut tf) in &mut q {
        if !base.npc {
            continue;
        }
        let Some(n) = npcs.npc(r.0) else { continue };
        tf.translation = Vec3::from(to_bevy(n.pos.to_array()));
        tf.rotation = arx_level::entity_rotation([base.angle[0], n.yaw, base.angle[2]], true);
    }
}

/// Diagnostics: with `ARX_LOG_NPC=<text>` print, once a second, what the characters whose id contains `<text>` are doing.
pub fn log(time: Res<Time>, npcs: Res<Npcs>, s: Res<Scripting>, mut since: Local<f32>) {
    let (Some(filter), Some(npcs)) = (std::env::var("ARX_LOG_NPC").ok(), npcs.0.as_ref()) else { return };
    *since += time.delta_secs();
    if *since < 1.0 {
        return;
    }
    *since = 0.0;
    for n in npcs.iter() {
        let id = &s.world.entity(n.id).id_string;
        if id.contains(&filter) {
            eprintln!(
                "npc {id}: pos {:.0},{:.0},{:.0} yaw {:.0} behavior {} anim {:?} traveling {} reached {} detect {}",
                n.pos.x, n.pos.y, n.pos.z, n.yaw, n.behavior, n.animation(), n.is_traveling(), n.reached, n.detect
            );
        }
    }
}
