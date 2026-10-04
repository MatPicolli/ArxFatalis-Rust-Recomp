//! Plays the sounds that scripts ask for (`play`), decoded from the game's ADPCM WAV files.

use crate::convert::to_bevy;
use crate::scripting::Scripting;
use crate::{Arx, Fly};
use arx_script::EntityId;
use bevy::{audio::{AudioPlayer, AudioSource, PlaybackSettings}, prelude::*};
use std::collections::HashMap;

/// Sounds farther than this (Arx units, about centimetres) are not started, like the original's
/// "too far" check.
const AUDIBLE_DISTANCE: f32 = 2500.0;
/// Upper bound on simultaneously playing sounds.
const MAX_VOICES: usize = 48;

#[derive(Resource, Default)]
pub struct Sounds {
    cache: HashMap<String, Option<Handle<AudioSource>>>,
    /// The sound each entity started with `play -i`, so it can be replaced or stopped.
    unique: HashMap<EntityId, Entity>,
    pub muted: bool,
    /// How loud effects are, 0 to 1 (the options menu).
    pub volume: f32,
    /// Number of sounds actually started (for diagnostics).
    pub started: u32,
    rng: u32,
}

impl Sounds {
    pub fn new(muted: bool) -> Self {
        Sounds { muted, volume: 1.0, rng: 0x9E37_79B9, ..default() }
    }

    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

pub fn play_sounds(
    mut commands: Commands,
    arx: Res<Arx>,
    fly: Res<Fly>,
    mut script: ResMut<Scripting>,
    mut sounds: ResMut<Sounds>,
    mut assets: ResMut<Assets<AudioSource>>,
    voices: Query<(), With<AudioPlayer>>,
) {
    let requests = script.host.take_sounds();
    if requests.is_empty() {
        return;
    }
    let listener = fly.pos;
    let mut live = voices.iter().count();
    for r in requests {
        if r.stop || r.unique {
            if let Some(old) = sounds.unique.remove(&r.entity) {
                commands.entity(old).try_despawn();
                live = live.saturating_sub(1);
            }
            if r.stop {
                continue;
            }
        }
        if sounds.muted || live >= MAX_VOICES {
            continue;
        }
        let at = r.positional.then(|| {
            let p = script.world.entity(r.entity).pos;
            Vec3::from(to_bevy(p))
        });
        if at.is_some_and(|p| p.distance(listener) > AUDIBLE_DISTANCE) {
            continue;
        }
        let handle = sounds
            .cache
            .entry(r.name.clone())
            .or_insert_with(|| {
                let bytes = arx.0.read(&format!("sfx/{}.wav", r.name)).ok()?;
                let pcm = arx_formats::wav::decode(&bytes).ok()?;
                Some(assets.add(AudioSource { bytes: pcm.to_wav_bytes().into() }))
            })
            .clone();
        let Some(handle) = handle else {
            script.world.stats.warnings.entry(format!("unable to load sound sfx/{}.wav", r.name)).and_modify(|n| *n += 1).or_insert(1);
            continue;
        };
        let mut settings = if r.looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN }.with_volume(bevy::audio::Volume::Linear(sounds.volume));
        if r.random_pitch {
            settings.speed = 0.9 + 0.2 * sounds.random();
        }
        let id = match at {
            Some(p) => commands.spawn((AudioPlayer::new(handle), settings.with_spatial(true), Transform::from_translation(p))).id(),
            None => commands.spawn((AudioPlayer::new(handle), settings)).id(),
        };
        if r.unique {
            sounds.unique.insert(r.entity, id);
        }
        if std::env::var_os("ARX_LOG_SOUND").is_some() {
            eprintln!("sound: {} from {} (positional {}, loop {})", r.name, script.world.entity(r.entity).id_string, r.positional, r.looping);
        }
        sounds.started += 1;
        live += 1;
    }
}
