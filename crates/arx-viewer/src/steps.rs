//! The player's footsteps: every 120 units walked on the ground the engine plays the sample that the step table
//! (`localisation/snd_step.ini`) gives for what is on the feet (barefoot at first) and what is underneath, taken in
//! turn from its variants, with the pitch varied.

use crate::{Arx, Fly};
use crate::audio::Sounds;
use arx_formats::soundmap::SoundMap;
use arx_script::{SpeechEvent, SpeechFlags, SpeechRequest};
use bevy::{audio::{AudioPlayer, AudioSource, PlaybackSettings}, prelude::*};
use std::collections::HashMap;

/// What the feet wear; the original starts the hero barefoot and lets boots change it (`stepmaterial`).
const DEFAULT_FOOTWEAR: &str = "foot_bare";

#[derive(Resource, Default)]
pub struct StepSounds {
    map: SoundMap,
    /// (what hits, what is hit) -> sample names that exist and which to play next.
    variants: HashMap<(String, String), (Vec<String>, usize)>,
    samples: HashMap<String, Option<Handle<AudioSource>>>,
    rng: u32,
    /// Footsteps played (for diagnostics).
    pub played: u32,
}

pub fn load(mut commands: Commands, arx: Res<Arx>) {
    commands.insert_resource(StepSounds { map: arx.0.load_sound_map(), rng: 0x1234_5678, ..default() });
}

impl StepSounds {
    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// The samples of a material pair: `<name>.wav`, `<name>1.wav` ... `<name>4.wav` (or `<name>_1.wav`), those found.
    fn variants_of(&mut self, arx: &Arx, hitter: &str, surface: &str) -> &mut (Vec<String>, usize) {
        let key = (hitter.to_owned(), surface.to_owned());
        if !self.variants.contains_key(&key) {
            let mut found = Vec::new();
            if let Some(base) = self.map.sample(hitter, surface) {
                for i in 0..5 {
                    let name = if i == 0 { base.to_owned() } else { format!("{base}{i}") };
                    let alt = format!("{base}_{i}");
                    if arx.0.contains(&format!("sfx/{name}.wav")) {
                        found.push(name);
                    } else if i > 0 && arx.0.contains(&format!("sfx/{alt}.wav")) {
                        found.push(alt);
                    }
                }
            }
            self.variants.insert(key.clone(), (found, 0));
        }
        self.variants.get_mut(&key).expect("just inserted")
    }

    /// Start the next footstep for `hitter` on `surface`.
    #[allow(clippy::too_many_arguments)]
    fn play(&mut self, commands: &mut Commands, arx: &Arx, assets: &mut Assets<AudioSource>, hitter: &str, surface: &str, volume: f32) -> Option<String> {
        let (names, next) = self.variants_of(arx, hitter, surface);
        if names.is_empty() {
            return None;
        }
        let name = names[*next % names.len()].clone();
        *next = (*next + 1) % names.len();
        let handle = self
            .samples
            .entry(name.clone())
            .or_insert_with(|| {
                let bytes = arx.0.read(&format!("sfx/{name}.wav")).ok()?;
                let pcm = arx_formats::wav::decode(&bytes).ok()?;
                Some(assets.add(AudioSource { bytes: pcm.to_wav_bytes().into() }))
            })
            .clone()?;
        let mut settings = PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(volume));
        settings.speed = 0.975 + 0.5 * self.random();
        commands.spawn((AudioPlayer::new(handle), settings));
        self.played += 1;
        Some(name)
    }
}

impl StepSounds {
    /// Play `sfx/<name>.wav` once, not positioned (interface and personal sounds).
    fn play_sfx(&mut self, commands: &mut Commands, arx: &Arx, assets: &mut Assets<AudioSource>, name: &str) {
        let handle = self
            .samples
            .entry(name.to_owned())
            .or_insert_with(|| {
                let bytes = arx.0.read(&format!("sfx/{name}.wav")).ok()?;
                let pcm = arx_formats::wav::decode(&bytes).ok()?;
                Some(assets.add(AudioSource { bytes: pcm.to_wav_bytes().into() }))
            })
            .clone();
        if let Some(h) = handle {
            commands.spawn((AudioPlayer::new(h), PlaybackSettings::DESPAWN));
        }
    }
}

/// Play the sounds the interface asked for (opening the backpack, dropping an item, ...) and the fanfare of a new level.
pub fn ui_sounds(
    mut commands: Commands,
    arx: Res<Arx>,
    sounds: Res<Sounds>,
    mut ui: ResMut<crate::hud::Ui>,
    mut steps: ResMut<StepSounds>,
    mut assets: ResMut<Assets<AudioSource>>,
    script: Res<crate::scripting::Scripting>,
    mut level: Local<Option<i32>>,
) {
    let now = script.host.player.level;
    if level.is_some_and(|before| now > before) {
        ui.sfx.push("player_level_up");
    }
    *level = Some(now);
    let queued = std::mem::take(&mut ui.sfx);
    if sounds.muted {
        return;
    }
    for name in queued {
        steps.play_sfx(&mut commands, &arx, &mut assets, name);
    }
}

/// Footsteps while walking, the jump voice, and the pain the player cries out when a fall hurts.
#[allow(clippy::too_many_arguments)]
pub fn footsteps(
    mut commands: Commands,
    arx: Res<Arx>,
    mut fly: ResMut<Fly>,
    sounds: Res<Sounds>,
    mut steps: ResMut<StepSounds>,
    mut assets: ResMut<Assets<AudioSource>>,
    mut script: ResMut<crate::scripting::Scripting>,
    mut npcs: ResMut<crate::npcs::Npcs>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if !fly.walk {
        return;
    }
    if fly.player.take_jump() {
        let player = script.player;
        script.host.push_speech(SpeechEvent::Say(SpeechRequest {
            speaker: player,
            script_entity: player,
            key: "player_jump".to_owned(),
            flags: SpeechFlags { no_text: true, ..default() },
            on_end: None,
        }));
    }
    let n = fly.player.take_steps();
    if n > 0
        && let Some(npcs) = npcs.0.as_mut()
    {
        // Characters nearby hear every step; sneaking with some stealth skill carries less far.
        let feet = fly.player.feet;
        let stealth = script.host.player.skills.get(arx_script::Skill::Stealth) / 100.0;
        let factor = if keys.pressed(KeyCode::ShiftLeft) { 1.0 + 3.5 * stealth } else { 1.0 };
        let (player, sc) = (script.player, &mut *script);
        npcs.hear(&mut sc.world, &mut sc.host, player, Vec3::new(feet.x, -feet.y, -feet.z), arx_level::npc::HEAR_STEP_DISTANCE / factor);
    }
    if n == 0 || sounds.muted {
        return;
    }
    let feet = fly.player.feet;
    let (hitter, surface) = match &fly.world {
        Some(w) if w.water_level_at(feet.x, feet.z).is_some_and(|level| level > feet.y + 5.0) => (DEFAULT_FOOTWEAR, "water"),
        Some(w) => (DEFAULT_FOOTWEAR, w.floor_material(feet.x, feet.z, feet.y + 10.0).unwrap_or("earth")),
        None => (DEFAULT_FOOTWEAR, "earth"),
    };
    let (hitter, surface) = (hitter.to_owned(), surface.to_owned());
    for _ in 0..n {
        let played = steps.play(&mut commands, &arx, &mut assets, &hitter, &surface, 1.0);
        if std::env::var_os("ARX_LOG_SOUND").is_some() {
            eprintln!("step: {hitter} on {surface}: {played:?}");
        }
    }
}

/// The noises of blows: weapons on armour and flesh (`snd_armor.ini`, `snd_weapon.ini`).
pub fn combat_sounds(
    mut commands: Commands,
    arx: Res<Arx>,
    sounds: Res<Sounds>,
    mut steps: ResMut<StepSounds>,
    mut assets: ResMut<Assets<AudioSource>>,
    mut npcs: ResMut<crate::npcs::Npcs>,
) {
    let Some(world) = npcs.0.as_mut() else { return };
    for snd in world.take_sounds() {
        if sounds.muted {
            continue;
        }
        let played = steps.play(&mut commands, &arx, &mut assets, &snd.hitter, &snd.surface, snd.volume);
        if std::env::var_os("ARX_LOG_SOUND").is_some() {
            eprintln!("blow: {} on {}: {played:?}", snd.hitter, snd.surface);
        }
    }
}

/// Other characters' footsteps: whenever a walking animation puts a foot down, within earshot, with what the character
/// wears on its feet (`setstepmaterial`, bare by default) on whatever the floor is made of.
pub fn npc_footsteps(
    mut commands: Commands,
    arx: Res<Arx>,
    fly: Res<Fly>,
    sounds: Res<Sounds>,
    mut steps: ResMut<StepSounds>,
    mut assets: ResMut<Assets<AudioSource>>,
    mut npcs: ResMut<crate::npcs::Npcs>,
    script: Res<crate::scripting::Scripting>,
) {
    let Some(world) = npcs.0.as_mut() else { return };
    let list = world.take_footsteps();
    let (Some(level), false) = (fly.world.as_ref(), sounds.muted) else { return };
    let player = Vec3::from(script.world.entity(script.player).pos);
    for f in list {
        let dist = f.pos.distance(player);
        if dist > 1500.0 {
            continue;
        }
        let feet = Vec3::new(f.pos.x, -f.pos.y, -f.pos.z);
        let surface = if level.water_level_at(feet.x, feet.z).is_some_and(|l| l > feet.y + 5.0) { "water" } else { level.floor_material(feet.x, feet.z, feet.y + 10.0).unwrap_or("earth") };
        let wear = script.host.state(f.id).map(|s| s.step_material.to_ascii_lowercase()).filter(|m| !m.is_empty()).unwrap_or_else(|| DEFAULT_FOOTWEAR.to_owned());
        let volume = (1.0 - dist / 1500.0).clamp(0.05, 1.0);
        let played = steps.play(&mut commands, &arx, &mut assets, &wear, surface, volume);
        if std::env::var_os("ARX_LOG_SOUND").is_some() {
            eprintln!("npc step: {wear} on {surface}: {played:?}");
        }
    }
}
