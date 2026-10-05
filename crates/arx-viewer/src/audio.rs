//! Plays the sounds that scripts ask for (`play`), decoded from the game's ADPCM WAV files.

use crate::convert::to_bevy;
use crate::scripting::Scripting;
use crate::{Arx, Fly};
use arx_script::EntityId;
use bevy::{audio::{AudioPlayer, AudioSinkPlayback, AudioSource, PlaybackSettings, SpatialAudioSink, Volume}, prelude::*};
use std::collections::HashMap;

/// Sounds farther than this (Arx units, about centimetres) are not started, like the original's
/// "too far" check.
const AUDIBLE_DISTANCE: f32 = 2500.0;
/// Up to this distance a sound is at full strength, and beyond [`FALL_END`] it gets no fainter
/// (`ARX_SOUND_DEFAULT_FALLSTART` / `FALLEND`).
const FALL_START: f32 = 200.0;
const FALL_END: f32 = 2200.0;
/// How quickly a sound fades with distance (the engine's rolloff factor).
const ROLLOFF: f32 = 1.3;

/// How loud a sound is `distance` units away, 0..1: the engine's model (OpenAL's clamped inverse distance).
pub fn gain(distance: f32) -> f32 {
    FALL_START / (FALL_START + ROLLOFF * (distance.clamp(FALL_START, FALL_END) - FALL_START))
}

/// A sound in the world: its loudness at the source. The distance to the listener does the rest ([`falloff`]); the
/// audio engine's own distance model is switched off (see the spatial scale in `main.rs`) and only pans.
#[derive(Component)]
pub struct Falloff(pub f32);

/// Keep every sound in the world as loud as its distance from the listener says.
pub fn falloff(fly: Res<Fly>, mut sounds: Query<(&Falloff, &Transform, &mut SpatialAudioSink)>) {
    for (f, at, mut sink) in &mut sounds {
        sink.set_volume(Volume::Linear(f.0 * gain(at.translation.distance(fly.pos))));
    }
}

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

    /// The level was left: its looping sounds are gone.
    pub fn forget(&mut self) {
        self.unique.clear();
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
        let loudness = at.map_or(1.0, |p| gain(p.distance(listener)));
        let mut settings = if r.looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN }.with_volume(Volume::Linear(sounds.volume * loudness));
        if r.random_pitch {
            settings.speed = 0.9 + 0.2 * sounds.random();
        }
        let id = match at {
            Some(p) => commands.spawn((AudioPlayer::new(handle), settings.with_spatial(true), Transform::from_translation(p), Falloff(sounds.volume))).id(),
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

#[cfg(test)]
mod tests {
    use super::gain;

    #[test]
    fn sounds_fade_as_the_engine_fades_them() {
        // Full strength up to two metres, then the inverse of the distance (rolloff 1.3), and no fainter past 22.
        assert_eq!((gain(0.0), gain(200.0)), (1.0, 1.0));
        assert!((gain(600.0) - 200.0 / 720.0).abs() < 1e-6);
        assert!((gain(2200.0) - 200.0 / 2800.0).abs() < 1e-6);
        assert_eq!(gain(5000.0), gain(2200.0));
        assert!(gain(400.0) > gain(800.0) && gain(800.0) > gain(1600.0));
    }
}
