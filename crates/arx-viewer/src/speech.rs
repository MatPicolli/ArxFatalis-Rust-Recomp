//! Dialogue and notifications: voiced `speak` lines with subtitles, `playspeech` samples and `herosay`
//! messages. Text comes from the game's localisation file, voices from `speech/<language>/<key>[N].wav`.

use crate::convert::to_bevy;
use crate::scripting::Scripting;
use crate::Arx;
use arx_formats::locale::Locale;
use arx_script::{EntityId, Script, SpeechEvent, SpeechFlags, SpeechRequest};
use bevy::{audio::{AudioPlayer, AudioSource, PlaybackSettings}, prelude::*};
use std::{collections::HashMap, sync::Arc};

/// At most this many `herosay` messages are on screen at once.
const MAX_NOTES: usize = 4;

/// Speech farther away than this is not written out (unless it is a conversation scene): at this distance its
/// voice is down to a quarter.
const SUBTITLE_DISTANCE: f32 = 700.0;

/// Shown for a message with no voice: base time plus time per character.
fn read_time(text: &str) -> f32 {
    2.0 + 0.06 * text.chars().count() as f32
}

struct Line {
    speaker: EntityId,
    script_entity: EntityId,
    text: Option<String>,
    remaining: f32,
    audio: Option<Entity>,
    unbreakable: bool,
    on_end: Option<(Arc<Script>, usize)>,
    /// Heard everywhere rather than from the speaker (the hero's own voice, a narrator).
    at_player: bool,
    mood: arx_script::Mood,
}

struct Note {
    text: String,
    remaining: f32,
}

#[derive(Resource)]
pub struct Speech {
    pub locale: Locale,
    pub language: String,
    pub subtitles: bool,
    pub muted: bool,
    /// How loud voices are, 0 to 1 (the options menu).
    pub volume: f32,
    /// Decoded voice files: handle and length in seconds (`None` if the file does not exist).
    cache: HashMap<String, Option<(Handle<AudioSource>, f32)>>,
    /// The variant of each key spoken last, so a repeated line is not repeated verbatim.
    last_variant: HashMap<String, usize>,
    lines: Vec<Line>,
    notes: Vec<Note>,
    rng: u32,
    /// Lines started, for diagnostics.
    pub said: u32,
}

impl Speech {
    pub fn new(locale: Locale, language: String, subtitles: bool, muted: bool) -> Self {
        Speech {
            locale,
            language,
            subtitles,
            muted,
            volume: 1.0,
            cache: HashMap::new(),
            last_variant: HashMap::new(),
            lines: Vec::new(),
            notes: Vec::new(),
            rng: 0x2545_F491,
            said: 0,
        }
    }

    fn random(&mut self, below: usize) -> usize {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as usize % below.max(1)
    }

    /// Pick a variant of `key`, avoiding the one used last time.
    fn pick_variant(&mut self, key: &str) -> usize {
        let n = self.locale.count(key);
        if n <= 1 {
            return 0;
        }
        let last = self.last_variant.get(key).copied();
        let mut v = self.random(n);
        if Some(v) == last {
            v = (v + 1 + self.random(n - 1)) % n;
        }
        self.last_variant.insert(key.to_owned(), v);
        v
    }

    /// Text shown for a message: the localised string, or the literal text a script gave.
    /// Nobody is speaking any more (the level was left).
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn text(&self, key: &str) -> String {
        self.locale.text_or_key(key).to_owned()
    }
}

#[derive(Component)]
pub struct SubtitleUi;

#[derive(Component)]
pub struct NotesUi;

pub fn spawn_ui(mut commands: Commands, font: Res<crate::hud_ui::UiFont>) {
    commands.spawn((
        SubtitleUi,
        Text::new(""),
        TextFont { font: font.0.clone().into(), font_size: FontSize::Px(22.0), ..default() },
        TextColor(Color::srgb(1.0, 0.96, 0.8)),
        TextLayout::justify(Justify::Center),
        TextShadow::default(),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(60.0),
            left: Val::Percent(15.0),
            width: Val::Percent(70.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
    commands.spawn((
        NotesUi,
        Text::new(""),
        TextFont { font: font.0.clone().into(), font_size: FontSize::Px(20.0), ..default() },
        TextColor(Color::srgb(0.85, 0.95, 1.0)),
        TextLayout::justify(Justify::Center),
        TextShadow::default(),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(70.0),
            left: Val::Percent(20.0),
            width: Val::Percent(60.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
}

/// Load a voice file, caching the result.
fn voice(
    cache: &mut HashMap<String, Option<(Handle<AudioSource>, f32)>>,
    arx: &Arx,
    assets: &mut Assets<AudioSource>,
    path: &str,
) -> Option<(Handle<AudioSource>, f32)> {
    cache
        .entry(path.to_owned())
        .or_insert_with(|| {
            let bytes = arx.0.read(path).ok()?;
            let pcm = arx_formats::wav::decode(&bytes).ok()?;
            Some((assets.add(AudioSource { bytes: pcm.to_wav_bytes().into() }), pcm.duration_secs()))
        })
        .clone()
}

fn start_line(
    commands: &mut Commands,
    arx: &Arx,
    assets: &mut Assets<AudioSource>,
    speech: &mut Speech,
    script: &Scripting,
    req: SpeechRequest,
) {
    let variant = speech.pick_variant(&req.key);
    let text = speech.locale.variants(&req.key).get(variant).cloned().filter(|t| !t.is_empty());
    let file = if variant == 0 {
        format!("speech/{}/{}.wav", speech.language, req.key)
    } else {
        format!("speech/{}/{}{}.wav", speech.language, req.key, variant + 1)
    };
    let sample = voice(&mut speech.cache, arx, assets, &file);

    let mut audio = None;
    let at_player = req.flags.off_voice || req.speaker == script.player;
    if let (Some((handle, _)), false) = (&sample, speech.muted) {
        let entity = if at_player {
            commands.spawn((AudioPlayer::new(handle.clone()), PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(speech.volume)))).id()
        } else {
            let p = Vec3::from(to_bevy(script.world.entity(req.speaker).pos));
            let listener = Vec3::from(to_bevy(script.world.entity(script.player).pos));
            let loudness = speech.volume * crate::audio::gain(p.distance(listener));
            commands
                .spawn((
                    AudioPlayer::new(handle.clone()),
                    PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(loudness)).with_spatial(true),
                    Transform::from_translation(p),
                    crate::audio::Falloff(speech.volume),
                ))
                .id()
        };
        audio = Some(entity);
    }

    let shown = (!req.flags.no_text).then(|| text.clone().or_else(|| Some(req.key.clone()))).flatten();
    // Without a voice, the text stays up long enough to read.
    let remaining = match (&sample, &text) {
        (Some((_, secs)), _) => *secs + 0.2,
        (None, Some(t)) => read_time(t),
        (None, None) => 1.0,
    };
    if std::env::var_os("ARX_LOG_SPEECH").is_some() {
        eprintln!(
            "speech: {} says {:?} ({}, {:.1}s, voice {})",
            script.world.entity(req.speaker).id_string,
            req.key,
            text.as_deref().unwrap_or("no text"),
            remaining,
            sample.is_some()
        );
    }
    speech.said += 1;
    speech.lines.push(Line {
        speaker: req.speaker,
        script_entity: req.script_entity,
        text: shown,
        remaining,
        audio,
        unbreakable: req.flags.unbreakable,
        on_end: req.on_end,
        at_player,
        mood: req.flags.mood,
    });
}

/// Headless testing aid (`--say id:key`): make an entity speak a line shortly after start-up.
pub fn debug_say(mut frames: Local<u32>, args: Res<crate::LevelArgs>, mut script: ResMut<Scripting>) {
    *frames += 1;
    let Some(spec) = args.say.as_deref().filter(|_| *frames == 20) else { return };
    let (name, key) = spec.split_once(':').unwrap_or(("", spec));
    let player = script.player;
    let speaker = script.world.find(name, player).unwrap_or(player);
    let key = arx_formats::locale::key_of(key).to_lowercase();
    script.host.push_speech(SpeechEvent::Say(SpeechRequest {
        speaker,
        script_entity: speaker,
        key,
        flags: SpeechFlags::default(),
        on_end: None,
    }));
}

/// Handle what scripts said this frame and advance the lines already being spoken.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    time: Res<Time>,
    arx: Res<Arx>,
    mut script: ResMut<Scripting>,
    mut speech: ResMut<Speech>,
    mut assets: ResMut<Assets<AudioSource>>,
    mut subtitle: Single<&mut Text, (With<SubtitleUi>, Without<NotesUi>)>,
    mut notes: Single<&mut Text, (With<NotesUi>, Without<SubtitleUi>)>,
) {
    let dt = time.delta_secs().min(0.1);
    let mut finished: Vec<Line> = Vec::new();

    for ev in script.host.take_speech() {
        match ev {
            SpeechEvent::Say(req) => {
                // A new line from the same speaker replaces theirs, unless theirs may not be interrupted.
                if speech.lines.iter().any(|l| l.speaker == req.speaker && l.unbreakable) {
                    continue;
                }
                let (replaced, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut speech.lines).into_iter().partition(|l| l.speaker == req.speaker);
                speech.lines = kept;
                finished.extend(replaced);
                let s = &mut *speech;
                start_line(&mut commands, &arx, &mut assets, s, &script, req);
            }
            SpeechEvent::Clear(e) => {
                let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut speech.lines).into_iter().partition(|l| l.speaker == e);
                speech.lines = kept;
                finished.extend(gone);
            }
            SpeechEvent::KillAll => finished.extend(std::mem::take(&mut speech.lines)),
            SpeechEvent::PlaySample { entity, name } => {
                let file = format!("speech/{}/{}.wav", speech.language, name.strip_suffix(".wav").unwrap_or(&name));
                let s = &mut *speech;
                if let (Some((handle, _)), false) = (voice(&mut s.cache, &arx, &mut assets, &file), s.muted) {
                    let p = Vec3::from(to_bevy(script.world.entity(entity).pos));
                    commands.spawn((AudioPlayer::new(handle), PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(s.volume)).with_spatial(true), Transform::from_translation(p), crate::audio::Falloff(s.volume)));
                }
            }
        }
    }

    for msg in script.host.take_messages() {
        let text = speech.text(&msg);
        if text.is_empty() {
            continue;
        }
        let remaining = read_time(&text);
        if std::env::var_os("ARX_LOG_SPEECH").is_some() {
            eprintln!("herosay: {text}");
        }
        speech.notes.push(Note { text, remaining });
        let excess = speech.notes.len().saturating_sub(MAX_NOTES);
        speech.notes.drain(..excess);
    }

    for l in &mut speech.lines {
        l.remaining -= dt;
    }
    let (done, live): (Vec<_>, Vec<_>) = std::mem::take(&mut speech.lines).into_iter().partition(|l| l.remaining <= 0.0);
    speech.lines = live;
    finished.extend(done);
    for n in &mut speech.notes {
        n.remaining -= dt;
    }
    speech.notes.retain(|n| n.remaining > 0.0);

    // Scripts ask whether somebody is speaking (`^speaking`) before starting another line.
    for (who, speaking) in finished.iter().map(|l| (l.speaker, 0)).chain(speech.lines.iter().map(|l| (l.speaker, 1))).collect::<Vec<_>>() {
        script.world.entity_mut(who).props.insert("^speaking".to_owned(), arx_script::Value::Int(speaking));
    }

    // A line that ended (or was cut off) runs the rest of its `speak` command.
    for l in finished {
        if let Some(a) = l.audio {
            commands.entity(a).try_despawn();
        }
        if let Some((s, pos)) = l.on_end {
            let sc = &mut *script;
            sc.world.run_line(&mut sc.host, l.script_entity, &s, pos);
        }
    }

    // What is said in a conversation scene (the black bars) is always written out, as in the original; outside one,
    // only what is close enough to be heard clearly, so that chatter rooms away does not fill the screen.
    let conversation = script.host.stage.cinemascope;
    let listener = Vec3::from(script.world.entity(script.player).pos);
    let near = |l: &Line| l.at_player || conversation || Vec3::from(script.world.entity(l.speaker).pos).distance(listener) < SUBTITLE_DISTANCE;
    let subs = if speech.subtitles {
        speech
            .lines
            .iter()
            .filter(|l| near(l))
            .filter_map(|l| l.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    if subtitle.0 != subs {
        subtitle.0 = subs;
    }
    let msgs = speech.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join("\n");
    if notes.0 != msgs {
        notes.0 = msgs;
    }
}

/// Whoever speaks moves their mouth: the character's talking animation (`talk_neutral`, `talk_happy`, `talk_angry`,
/// which move the head and jaw only) plays over whatever the body is doing, for as long as the line lasts
/// (`ARX_SPEECH_Update`). The hero's first-person body is left alone: its second layer is the arms.
pub fn talk(
    arx: Res<Arx>,
    speech: Res<Speech>,
    body: Res<crate::player_body::PlayerBody>,
    mut script: ResMut<Scripting>,
    mut q: Query<(Entity, &crate::scripting::ScriptRef, &mut crate::animated::Animated)>,
    mut talking: Local<HashMap<Entity, String>>,
) {
    if speech.lines.is_empty() && talking.is_empty() {
        return;
    }
    for (entity, r, mut animated) in &mut q {
        if Some(entity) == body.entity {
            continue;
        }
        let want = speech.lines.iter().find(|l| l.speaker == r.0).and_then(|l| {
            let st = script.host.state(r.0)?;
            let slot = match l.mood {
                arx_script::Mood::Happy => "talk_happy",
                arx_script::Mood::Angry => "talk_angry",
                arx_script::Mood::Neutral => "talk_neutral",
            };
            st.anims.get(slot).or_else(|| st.anims.get("talk_neutral")).cloned()
        });
        match want {
            Some(path) => {
                if (talking.get(&entity) != Some(&path) || animated.overlay.is_none())
                    && let Some(tea) = script.anim(&arx.0, &path)
                {
                    animated.overlay = Some(crate::animated::Overlay { anim: tea, elapsed_us: 0, looping: true });
                    talking.insert(entity, path);
                }
            }
            None => {
                if talking.remove(&entity).is_some() {
                    animated.overlay = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_do_not_repeat_back_to_back() {
        let locale = Locale::parse(b"[hello]
string=\"a\"
string2=\"b\"
string3=\"c\"
[one]
string=\"x\"
");
        let mut s = Speech::new(locale, "english".into(), true, true);
        let mut last = usize::MAX;
        for _ in 0..200 {
            let v = s.pick_variant("hello");
            assert!(v < 3 && v != last);
            last = v;
        }
        assert_eq!(s.pick_variant("one"), 0);
        assert_eq!(s.pick_variant("unknown"), 0);
    }

    #[test]
    fn notes_read_time_grows_with_length() {
        assert!(read_time("a much longer message than the other") > read_time("hi"));
    }
}
