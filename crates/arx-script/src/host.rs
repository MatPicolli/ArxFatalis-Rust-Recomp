//! A [`Host`] that implements the commands which change how entities look and behave, keeping the
//! result as plain data per entity for the renderer to apply. Commands it does not know are skipped
//! by the interpreter (the rest of their line is ignored), so gameplay-only commands cost nothing.

use crate::interp::{Args, CmdResult, Host, has_flag};
use crate::world::{EntityId, EntityKind, Timer};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PlayAnim {
    /// Slot name (`wait`, `action1`, `die`, ...).
    pub slot: String,
    pub looping: bool,
}

#[derive(Debug, Clone)]
pub struct EntityState {
    /// `usemesh` model, as a virtual path without extension (`graph/obj3d/interactive/...`).
    pub mesh: Option<String>,
    pub scale: f32,
    pub hidden: bool,
    pub destroyed: bool,
    pub collision: bool,
    pub interactive: bool,
    /// Slot name -> animation file stem.
    pub anims: HashMap<String, String>,
    pub playing: Option<PlayAnim>,
    /// Bumped whenever `playing` is (re)started, so the renderer can restart the animation.
    pub anim_serial: u32,
    /// Bumped whenever anything the renderer cares about changes.
    pub revision: u32,
    pub name: String,
}

impl Default for EntityState {
    fn default() -> Self {
        EntityState {
            mesh: None,
            scale: 1.0,
            hidden: false,
            destroyed: false,
            collision: true,
            interactive: true,
            anims: HashMap::new(),
            playing: None,
            anim_serial: 0,
            revision: 0,
            name: String::new(),
        }
    }
}

/// A request from a script to play (or stop) a sound, for the audio system to carry out.
#[derive(Debug, Clone)]
pub struct SoundRequest {
    pub entity: EntityId,
    /// Sample name without folder or extension (`door_wood_open` plays `sfx/door_wood_open.wav`).
    pub name: String,
    pub looping: bool,
    /// Replace this entity's previous "unique" sound.
    pub unique: bool,
    /// Stop the entity's unique sound instead of playing anything.
    pub stop: bool,
    /// Vary the pitch by up to 10% each way.
    pub random_pitch: bool,
    /// Heard from the entity's position (otherwise at full volume wherever the player is).
    pub positional: bool,
}

/// Returns the length in milliseconds of the animation file at a virtual path.
pub type AnimDuration = Box<dyn Fn(&str) -> Option<f64> + Send + Sync>;

#[derive(Default)]
pub struct StdHost {
    states: Vec<EntityState>,
    anim_duration: Option<AnimDuration>,
    sounds: Vec<SoundRequest>,
}

impl StdHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tell the host how to measure animations (needed for `playanim -e`, which runs a command when
    /// the animation ends).
    pub fn set_anim_duration(&mut self, f: AnimDuration) {
        self.anim_duration = Some(f);
    }

    /// Sound requests made since the last call.
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.sounds)
    }

    pub fn state(&self, id: EntityId) -> Option<&EntityState> {
        self.states.get(id as usize)
    }

    fn state_mut(&mut self, id: EntityId) -> &mut EntityState {
        if self.states.len() <= id as usize {
            self.states.resize_with(id as usize + 1, EntityState::default);
        }
        let s = &mut self.states[id as usize];
        s.revision += 1;
        s
    }
}

/// `usemesh` paths are relative to a folder chosen by the entity kind.
fn mesh_dir(kind: EntityKind) -> Option<&'static str> {
    match kind {
        EntityKind::Npc => Some("graph/obj3d/interactive/npc"),
        EntityKind::Fix => Some("graph/obj3d/interactive/fix_inter"),
        EntityKind::Item => Some("graph/obj3d/interactive/items"),
        _ => None,
    }
}

fn anim_dir(kind: EntityKind) -> &'static str {
    if matches!(kind, EntityKind::Npc | EntityKind::Player) {
        "graph/obj3d/anims/npc"
    } else {
        "graph/obj3d/anims/fix_inter"
    }
}

impl Host for StdHost {
    fn command(&mut self, name: &str, a: &mut Args) -> Option<CmdResult> {
        let me = a.entity();
        Some(match name {
            "usemesh" => {
                let raw = a.get_word();
                let mut path = raw.to_ascii_lowercase().replace('\\', "/");
                if let Some(stem) = path.strip_suffix(".teo").or_else(|| path.strip_suffix(".ftl")) {
                    path = stem.to_owned();
                }
                if let Some(dir) = mesh_dir(a.world.entity(me).kind) {
                    self.state_mut(me).mesh = Some(format!("{dir}/{path}"));
                }
                CmdResult::Success
            }
            "setscale" => {
                let s = a.get_float() * 0.01;
                self.state_mut(me).scale = s;
                CmdResult::Success
            }
            "objecthide" => {
                let _flags = a.get_flags();
                let target = a.get_word();
                let hide = a.get_bool();
                match a.world.find(&target, me) {
                    Some(t) => {
                        self.state_mut(t).hidden = hide;
                        CmdResult::Success
                    }
                    None => CmdResult::Failed,
                }
            }
            "collision" => {
                let on = a.get_bool();
                self.state_mut(me).collision = on;
                CmdResult::Success
            }
            "setinteractivity" => {
                let w = a.get_word();
                self.state_mut(me).interactive = !matches!(w.as_str(), "none" | "hide");
                CmdResult::Success
            }
            "setname" => {
                let w = a.get_word();
                self.state_mut(me).name = w;
                CmdResult::Success
            }
            "setgroup" => {
                let flags = a.get_flags();
                let w = a.get_word();
                let group = a.string_var(&w).to_ascii_lowercase();
                if group.is_empty() {
                    a.warn("missing group");
                    return Some(CmdResult::Failed);
                }
                let groups = &mut a.world.entity_mut(me).groups;
                if has_flag(&flags, 'r') {
                    groups.remove(&group);
                } else {
                    groups.insert(group);
                }
                CmdResult::Success
            }
            "loadanim" => {
                let slot = a.get_word();
                let file = a.get_word().to_ascii_lowercase();
                let slot = slot.to_ascii_lowercase();
                let dir = anim_dir(a.world.entity(me).kind);
                let st = self.state_mut(me);
                if file == "none" {
                    st.anims.remove(&slot);
                } else {
                    st.anims.insert(slot, format!("{dir}/{file}.tea"));
                }
                CmdResult::Success
            }
            "playanim" | "forceanim" => {
                let mut entity = me;
                let mut looping = false;
                if name == "playanim" {
                    let flags = a.get_flags();
                    looping = has_flag(&flags, 'l');
                    if has_flag(&flags, 'p') {
                        match a.world.player {
                            Some(p) => entity = p,
                            None => return Some(CmdResult::Failed),
                        }
                    }
                    let slot = a.get_word().to_ascii_lowercase();
                    let result = self.play(entity, &slot, looping, a);
                    if has_flag(&flags, 'e') {
                        // Run the rest of the line when the animation ends (at least 1s later).
                        let Some(pos) = a.skip_command() else {
                            a.warn("used -e flag without command to execute");
                            return Some(result);
                        };
                        let path = self.state(entity).and_then(|s| s.anims.get(&slot)).cloned();
                        let mut interval: f64 = 1000.0;
                        if let (Some(f), Some(p)) = (&self.anim_duration, path) {
                            interval = interval.max(f(&p).unwrap_or(0.0));
                        }
                        let name = a.world.default_timer_name(me, "anim_timer");
                        let start = a.world.now_ms;
                        let script = a.ctx.script.clone();
                        a.world.add_timer(Timer { entity: me, name, script, pos, interval_ms: interval, start_ms: start, count: 1 });
                    }
                    return Some(result);
                }
                let slot = a.get_word().to_ascii_lowercase();
                self.play(entity, &slot, looping, a)
            }
            "play" => {
                let flags = a.get_flags();
                let w = a.get_word();
                let name = a.string_var(&w).to_ascii_lowercase();
                let name = name.strip_suffix(".wav").unwrap_or(&name).replace('\\', "/");
                // Inventory-use sounds are played at the player, whatever the flags say.
                let positional = !has_flag(&flags, 'o') && a.ctx.event != "inventoryuse";
                self.sounds.push(SoundRequest {
                    entity: me,
                    name,
                    looping: has_flag(&flags, 'l'),
                    unique: has_flag(&flags, 'i'),
                    stop: has_flag(&flags, 's'),
                    random_pitch: has_flag(&flags, 'p'),
                    positional,
                });
                CmdResult::Success
            }
            "destroy" => {
                let w = a.get_word();
                let target = a.string_var(&w);
                match a.world.find(&target, me) {
                    Some(t) => {
                        self.state_mut(t).destroyed = true;
                        if t == me { CmdResult::AbortAccept } else { CmdResult::Success }
                    }
                    None => CmdResult::Success,
                }
            }
            _ => return None,
        })
    }
}

impl StdHost {
    fn play(&mut self, entity: EntityId, slot: &str, looping: bool, a: &mut Args) -> CmdResult {
        if slot == "none" {
            self.state_mut(entity).playing = None;
            return CmdResult::Success;
        }
        if !self.state(entity).is_some_and(|s| s.anims.contains_key(slot)) {
            a.warn(&format!("animation {slot} not loaded"));
            return CmdResult::Failed;
        }
        let st = self.state_mut(entity);
        st.playing = Some(PlayAnim { slot: slot.to_owned(), looping });
        st.anim_serial += 1;
        CmdResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Script, ScriptWorld};
    use std::sync::Arc;

    fn setup(class: &str, kind: EntityKind, src: &str) -> (ScriptWorld, StdHost, EntityId) {
        let mut w = ScriptWorld::new();
        let id = w.add_entity(kind, class, 1, Some(Arc::new(Script::new(src.as_bytes()))), None);
        (w, StdHost::new(), id)
    }

    #[test]
    fn door_loads_animations_and_plays_them_on_events() {
        let (mut w, mut h, id) = setup(
            "graph/obj3d/interactive/fix_inter/light_door/light_door",
            EntityKind::Fix,
            "on initend {\n loadanim action1 \"Door_close\"\n loadanim action2 \"Door_open\"\n accept\n}\non open {\n playanim action2\n collision off\n accept\n}",
        );
        w.send_init(&mut h, id);
        let st = h.state(id).unwrap();
        assert_eq!(st.anims["action2"], "graph/obj3d/anims/fix_inter/door_open.tea");
        assert!(st.playing.is_none());
        w.send_event(&mut h, None, id, "open", vec![]);
        let st = h.state(id).unwrap();
        assert_eq!(st.playing.as_ref().unwrap().slot, "action2");
        assert!(!st.playing.as_ref().unwrap().looping);
        assert_eq!(st.anim_serial, 1);
        assert!(!st.collision);
    }

    #[test]
    fn playanim_e_runs_the_rest_of_the_line_when_the_animation_ends() {
        let (mut w, mut h, id) = setup(
            "graph/obj3d/interactive/fix_inter/door/door",
            EntityKind::Fix,
            "on init {
 loadanim action2 \"open\"
 accept
}
on action {
 setinteractivity none
 playanim -e action2 setinteractivity yes
 accept
}",
        );
        h.set_anim_duration(Box::new(|_| Some(2500.0)));
        w.send_init(&mut h, id);
        w.send_event(&mut h, None, id, "action", vec![]);
        assert!(!h.state(id).unwrap().interactive);
        w.update(&mut h, 2000.0);
        assert!(!h.state(id).unwrap().interactive, "animation (2.5s) has not finished yet");
        w.update(&mut h, 600.0);
        assert!(h.state(id).unwrap().interactive);
        assert_eq!(w.timer_count(), 0);
    }

    #[test]
    fn play_queues_sound_requests_with_flags_and_variables() {
        // Local text variables use the Latin-1 pound sign, which is a single byte in script files.
        let src: Vec<u8> = "on init {\n set \u{a3}sfx \"Door_Wood_Open\"\n accept\n}\non open {\n play ~\u{a3}sfx~\n play -li hum\n play -s hum\n play -o click.wav\n accept\n}"
            .chars()
            .map(|c| c as u8)
            .collect();
        let mut w = ScriptWorld::new();
        let id = w.add_entity(EntityKind::Fix, "x/y/door", 1, Some(Arc::new(Script::new(&src))), None);
        let mut h = StdHost::new();
        w.send_init(&mut h, id);
        w.send_event(&mut h, None, id, "open", vec![]);
        let s = h.take_sounds();
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].name, "door_wood_open");
        assert!(s[0].positional && !s[0].looping);
        assert!(s[1].looping && s[1].unique && s[1].name == "hum");
        assert!(s[2].stop);
        assert!(!s[3].positional && s[3].name == "click");
        assert!(h.take_sounds().is_empty());
    }

    #[test]
    fn usemesh_scale_hide_and_groups() {
        let (mut w, mut h, id) = setup(
            "graph/obj3d/interactive/fix_inter/door/door",
            EntityKind::Fix,
            "on load {\n usemesh \"Door_L2\\Door_L2.teo\"\n accept\n}\non init {\n setscale 50\n setgroup door\n objecthide self on\n accept\n}",
        );
        w.send_event(&mut h, None, id, "load", vec![]);
        w.send_init(&mut h, id);
        let st = h.state(id).unwrap();
        assert_eq!(st.mesh.as_deref(), Some("graph/obj3d/interactive/fix_inter/door_l2/door_l2"));
        assert!((st.scale - 0.5).abs() < 1e-6);
        assert!(st.hidden);
        assert!(w.entity(id).groups.contains("door"));
    }

    #[test]
    fn playing_an_unloaded_animation_fails_without_effect() {
        let (mut w, mut h, id) = setup("x/y/z", EntityKind::Fix, "on init {\n playanim -l walk\n accept\n}");
        w.send_init(&mut h, id);
        assert!(h.state(id).is_none_or(|s| s.playing.is_none()));
        assert!(!w.stats.warnings.is_empty());
    }
}
