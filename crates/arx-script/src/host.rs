//! A [`Host`] that implements the commands which change how entities look and behave, keeping the
//! result as plain data per entity for the renderer to apply. Commands it does not know are skipped
//! by the interpreter (the rest of their line is ignored), so gameplay-only commands cost nothing.

use crate::interp::{Args, CmdResult, Host, has_flag};
use crate::player::{MAX_BAGS, PlayerState};
use crate::text::Script;
use crate::world::{EntityId, EntityKind, ScriptWorld, Timer};
use std::{collections::HashMap, sync::Arc};

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
    /// Items: how many are in this stack (1 for anything that cannot stack).
    pub count: u32,
    /// Items: the most the player can carry in one stack (`playerstacksize`).
    pub stack_size: u32,
    /// Items: hunger restored by eating it (`setfood`).
    pub food: f32,
    pub weight: f32,
    pub price: f32,
    /// Carried by the player (and so not in the world).
    pub in_inventory: bool,
    /// Containers: the background picture of their panel (`inventory skin`), e.g. `ingame_inventory_chest_metal`.
    pub inventory_skin: String,
    /// Turned by `rotate`: degrees added to the angles the level gave it (pitch, yaw, roll).
    pub rotation: [f32; 3],
    /// Moved by the game (dropped by the player), in Arx coordinates; the renderer places the entity here.
    pub moved_to: Option<[f32; 3]>,
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
            count: 1,
            stack_size: 1,
            food: 0.0,
            weight: 0.0,
            price: 0.0,
            in_inventory: false,
            inventory_skin: String::new(),
            rotation: [0.0; 3],
            moved_to: None,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mood {
    #[default]
    Neutral,
    Happy,
    Angry,
}

/// How a spoken line is delivered (`speak` flags).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpeechFlags {
    /// `-t`: audio only, no text.
    pub no_text: bool,
    /// `-u`: cannot be cut short.
    pub unbreakable: bool,
    /// `-o`: heard everywhere, not from the speaker's position.
    pub off_voice: bool,
    pub mood: Mood,
}

/// Something spoken, for the application to voice and show.
#[derive(Debug, Clone)]
pub struct SpeechRequest {
    /// Who speaks (the player with `speak -p`).
    pub speaker: EntityId,
    /// The entity whose script asked for the speech.
    pub script_entity: EntityId,
    /// Localisation key: text from the locale file, sound `speech/<language>/<key>[N].wav`.
    pub key: String,
    pub flags: SpeechFlags,
    /// A command to run when the speech ends (the rest of the `speak` line): script and position.
    pub on_end: Option<(Arc<Script>, usize)>,
}

#[derive(Debug, Clone)]
pub enum SpeechEvent {
    Say(SpeechRequest),
    /// `speak ""`: silence this entity.
    Clear(EntityId),
    /// `speak killall`
    KillAll,
    /// `playspeech name`: play a speech sample with no text.
    PlaySample { entity: EntityId, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// A scrap of paper.
    Note,
    /// A notice board or sign.
    Notice,
    Book,
}

/// Something for the player to read (`note`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRequest {
    pub kind: NoteKind,
    /// Localisation key (or literal text) of what it says.
    pub text: String,
}

/// Loads the script of an entity class (a virtual path without extension), for items that scripts create
/// (`inventory add`).
pub type ScriptLoader = Box<dyn Fn(&str) -> Option<Arc<Script>> + Send + Sync>;

/// What happened to an item given to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carry {
    /// Now in the inventory as its own entry.
    Added,
    /// Merged into the stack of the same kind the player already carries (its entity).
    Stacked(EntityId),
    /// Gold: added to the purse (this many coins) and the item is gone.
    Gold(u64),
    /// No room for it in the inventory; nothing changed.
    Full,
}

/// Size in pixels of an item's inventory icon, by class (the grid size is derived from it).
pub type IconSize = Box<dyn Fn(&str) -> Option<(u32, u32)> + Send + Sync>;

/// Is this item class gold coins (which go to the purse instead of the grid)?
pub fn is_gold_class(class: &str) -> bool {
    class.ends_with("/gold_coin/gold_coin")
}

/// Returns the length in milliseconds of the animation file at a virtual path.
pub type AnimDuration = Box<dyn Fn(&str) -> Option<f64> + Send + Sync>;

#[derive(Default)]
pub struct StdHost {
    states: Vec<EntityState>,
    anim_duration: Option<AnimDuration>,
    sounds: Vec<SoundRequest>,
    speech: Vec<SpeechEvent>,
    messages: Vec<String>,
    /// The player's life, mana, hunger and inventory.
    pub player: PlayerState,
    script_loader: Option<ScriptLoader>,
    icon_size: Option<IconSize>,
    /// What chests, corpses and characters hold (`inventory add`): item entities not in the world.
    pub containers: HashMap<EntityId, Vec<EntityId>>,
    /// The container the player is looking into (`inventory open`).
    pub open_container: Option<EntityId>,
    notes: Vec<NoteRequest>,
    /// Items the player put back into the world, for the renderer to give a model (see `take_dropped`).
    dropped: Vec<EntityId>,
}

impl StdHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tell the host how to load item scripts, so `inventory add` can create items.
    pub fn set_script_loader(&mut self, f: ScriptLoader) {
        self.script_loader = Some(f);
    }

    /// Tell the host how big item icons are, so the inventory grid can give each item its room.
    pub fn set_icon_size(&mut self, f: IconSize) {
        self.icon_size = Some(f);
    }

    /// Slots an item of this class takes in a bag: its icon size in 32-pixel cells, 1 to 3 each way.
    pub fn item_slots(&self, class: &str) -> (u8, u8) {
        let (w, h) = self.icon_size.as_ref().and_then(|f| f(class)).unwrap_or((32, 32));
        let cells = |px: u32| ((px + 31) / 32).clamp(1, 3) as u8;
        (cells(w), cells(h))
    }

    /// Put `item` into the player's inventory: gold goes to the purse; anything else joins a carried stack of
    /// the same kind that has room (up to its `playerstacksize`), or takes a free place in the grid. The item
    /// leaves the world. `Full` if there is no room.
    pub fn carry(&mut self, world: &ScriptWorld, item: EntityId) -> Carry {
        let st = self.state(item).cloned().unwrap_or_default();
        let class = &world.entity(item).class;
        if is_gold_class(class) {
            let coins = u64::from(st.price.max(1.0) as u32) * u64::from(st.count.max(1));
            self.player.gold += coins;
            self.modify(item, |s| {
                s.count = 0;
                s.destroyed = true;
                s.collision = false;
            });
            return Carry::Gold(coins);
        }
        let target = self.player.inventory.iter().copied().find(|&i| {
            &world.entity(i).class == class && self.state(i).is_some_and(|t| t.stack_size > 1 && t.count < t.stack_size)
        });
        let mut remaining = st.count;
        if let Some(target) = target {
            let t = self.state(target).expect("carried items have a state");
            let moved = remaining.min(t.stack_size - t.count);
            self.modify(target, |t| t.count += moved);
            remaining -= moved;
            if remaining == 0 {
                self.modify(item, |s| {
                    s.count = 0;
                    s.destroyed = true;
                    s.collision = false;
                });
                return Carry::Stacked(target);
            }
        }
        let (w, h) = self.item_slots(class);
        let Some(slot) = self.player.find_free(w, h) else {
            // The stack was merged as far as it went; what is left stays where it was.
            if remaining != st.count {
                self.modify(item, |s| s.count = remaining);
            }
            return Carry::Full;
        };
        self.modify(item, |s| {
            s.count = remaining;
            s.in_inventory = true;
            s.hidden = true;
            s.collision = false;
            s.moved_to = None;
        });
        self.player.inventory.push(item);
        self.player.slots.insert(item, slot);
        Carry::Added
    }

    /// Forget what `owner` holds (its items are destroyed) and close it if it was open.
    fn destroy_container(&mut self, owner: EntityId) {
        for item in self.containers.remove(&owner).unwrap_or_default() {
            self.modify(item, |s| s.destroyed = true);
        }
        if self.open_container == Some(owner) {
            self.open_container = None;
        }
    }

    /// Create a new item of class `name` (a path below `graph/obj3d/interactive/items`), run its start-up, and
    /// leave it out of the world. `None` if its script cannot be found.
    fn spawn_item(&mut self, a: &mut Args, name: &str) -> Option<EntityId> {
        // Scripts write paths with single or doubled backslashes.
        let name = name.trim().trim_matches('"').to_ascii_lowercase();
        let parts: Vec<&str> = name.split(['\\', '/']).filter(|part| !part.is_empty()).collect();
        let class = format!("graph/obj3d/interactive/items/{}", parts.join("/"));
        let Some(script) = self.script_loader.as_ref().and_then(|load| load(&class)) else {
            a.warn(&format!("could not add item {class}"));
            return None;
        };
        let instance = a.world.entities.iter().filter(|e| e.class == class).map(|e| e.instance).max().unwrap_or(0) + 1;
        let id = a.world.add_entity(EntityKind::Item, &class, instance, Some(script), None);
        self.modify(id, |s| {
            s.in_inventory = true;
            s.hidden = true;
            s.collision = false;
        });
        a.world.send_event(self, None, id, "load", Vec::new());
        a.world.send_init(self, id);
        Some(id)
    }

    /// The `inventory` command: containers and giving items to the player.
    fn inventory_command(&mut self, a: &mut Args) -> CmdResult {
        let me = a.entity();
        let sub: String = a.get_word().chars().filter(|c| *c != '_').collect();
        let has_container = |h: &Self, w: &ScriptWorld| h.containers.contains_key(&me) || w.entity(me).kind == EntityKind::Npc;
        match sub.as_str() {
            "create" => {
                self.destroy_container(me);
                self.containers.insert(me, Vec::new());
            }
            "skin" => {
                let w = a.get_word();
                let skin = a.string_var(&w).to_ascii_lowercase();
                self.state_mut(me).inventory_skin = skin;
            }
            "destroy" => self.destroy_container(me),
            "open" => {
                if has_container(self, a.world) {
                    self.open_container = Some(me);
                }
            }
            "close" => self.open_container = None,
            "add" | "addmulti" | "playeradd" | "playeraddmulti" => {
                let multi = sub.ends_with("multi");
                let to_player = sub.starts_with("player");
                let w = a.get_word();
                let name = a.string_var(&w);
                let count = multi.then(|| a.get_float());
                if !to_player && !has_container(self, a.world) {
                    return CmdResult::Failed;
                }
                if count == Some(0.0) {
                    return CmdResult::Success;
                }
                let Some(item) = self.spawn_item(a, &name) else { return CmdResult::Failed };
                if let Some(n) = count {
                    self.modify(item, |s| {
                        s.stack_size = 9999;
                        s.count = (n as u32).max(1);
                    });
                }
                if to_player {
                    let player = a.world.player;
                    if self.carry(a.world, item) == Carry::Added {
                        a.world.send_event(self, player, item, "inventoryin", Vec::new());
                    }
                } else {
                    self.containers.entry(me).or_default().push(item);
                }
            }
            "addfromscene" | "playeraddfromscene" => {
                let w = a.get_word();
                let target = a.string_var(&w);
                let Some(item) = a.world.find(&target, me) else {
                    a.warn(&format!("unknown target: {target}"));
                    return CmdResult::Failed;
                };
                if sub.starts_with("player") {
                    let player = a.world.player;
                    if self.carry(a.world, item) == Carry::Added {
                        a.world.send_event(self, player, item, "inventoryin", Vec::new());
                    }
                } else if has_container(self, a.world) {
                    self.modify(item, |s| {
                        s.in_inventory = true;
                        s.hidden = true;
                        s.collision = false;
                    });
                    self.containers.entry(me).or_default().push(item);
                }
            }
            other => {
                a.warn(&format!("unknown inventory command: {other}"));
                return CmdResult::Failed;
            }
        }
        CmdResult::Success
    }

    /// Tell the host how to measure animations (needed for `playanim -e`, which runs a command when
    /// the animation ends).
    pub fn set_anim_duration(&mut self, f: AnimDuration) {
        self.anim_duration = Some(f);
    }

    /// Speech requests made since the last call.
    pub fn take_speech(&mut self) -> Vec<SpeechEvent> {
        std::mem::take(&mut self.speech)
    }

    /// Remember that the player put `item` into the world.
    pub fn note_dropped(&mut self, item: EntityId) {
        self.dropped.push(item);
    }

    /// Items put into the world since the last call.
    pub fn take_dropped(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.dropped)
    }

    /// Things scripts asked the player to read since the last call.
    pub fn take_notes(&mut self) -> Vec<NoteRequest> {
        std::mem::take(&mut self.notes)
    }

    /// Let scripts read the player's stats (`^player_life`, `^player_skill_mecanism`, ...).
    pub fn publish_player(&self, world: &mut ScriptWorld) {
        for (name, v) in self.player.script_vars() {
            world.sys.insert(name, crate::Value::Float(v));
        }
    }

    /// Queue a speech event as if a script had asked for it (test aid).
    pub fn push_speech(&mut self, ev: SpeechEvent) {
        self.speech.push(ev);
    }

    /// Show a notification, as `herosay` does.
    pub fn push_message(&mut self, text: String) {
        self.messages.push(text);
    }

    /// `herosay` messages (localisation keys or literal text) since the last call.
    pub fn take_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    /// Sound requests made since the last call.
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.sounds)
    }

    pub fn state(&self, id: EntityId) -> Option<&EntityState> {
        self.states.get(id as usize)
    }

    /// Change an entity's state from outside a script (the game picking something up, ...).
    pub fn modify(&mut self, id: EntityId, f: impl FnOnce(&mut EntityState)) {
        f(self.state_mut(id));
    }

    /// Forget inventory entries that have been destroyed (eaten, ...).
    pub fn prune_inventory(&mut self) {
        let states = &self.states;
        let gone: Vec<EntityId> =
            self.player.inventory.iter().copied().filter(|&i| states.get(i as usize).is_some_and(|s| s.destroyed)).collect();
        for id in gone {
            self.player.remove_item(id);
        }
    }

    /// What `destroy` / `eatme` do to an entity: one item leaves a stack, anything else is destroyed. Returns
    /// whether the entity itself is gone.
    fn destroy_delayed(&mut self, id: EntityId, kind: EntityKind) -> bool {
        let st = self.state_mut(id);
        if kind == EntityKind::Item && st.count > 1 {
            st.count -= 1;
            return false;
        }
        st.destroyed = true;
        true
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
            "herosay" => {
                let flags = a.get_flags();
                if has_flag(&flags, 'd') {
                    a.skip_word(); // debug text, never shown
                    return Some(CmdResult::Success);
                }
                let w = a.get_word();
                let text = a.string_var(&w);
                if !text.is_empty() {
                    self.messages.push(text);
                }
                CmdResult::Success
            }
            "playspeech" => {
                let w = a.get_word();
                let name = a.string_var(&w).to_ascii_lowercase().replace('\\', "/");
                self.speech.push(SpeechEvent::PlaySample { entity: me, name });
                CmdResult::Success
            }
            "speak" => {
                let flags = a.get_flags();
                let mut f = SpeechFlags {
                    no_text: has_flag(&flags, 't'),
                    unbreakable: has_flag(&flags, 'u'),
                    off_voice: has_flag(&flags, 'o'),
                    mood: if has_flag(&flags, 'h') {
                        Mood::Happy
                    } else if has_flag(&flags, 'a') {
                        Mood::Angry
                    } else {
                        Mood::Neutral
                    },
                };
                let speaker = if has_flag(&flags, 'p') { a.world.player.unwrap_or(me) } else { me };
                if has_flag(&flags, 'c') {
                    skip_cinematic_arguments(a);
                    f.no_text = false;
                }
                let word = a.get_word();
                if word == "killall" {
                    self.speech.push(SpeechEvent::KillAll);
                    return Some(CmdResult::Success);
                }
                let key = speech_key(&a.string_var(&word));
                if key.is_empty() {
                    self.speech.push(SpeechEvent::Clear(me));
                    return Some(CmdResult::Success);
                }
                // Whatever else is on the line runs once the speech has ended.
                let on_end = a.skip_command().map(|pos| (a.ctx.script.clone(), pos));
                self.speech.push(SpeechEvent::Say(SpeechRequest { speaker, script_entity: me, key, flags: f, on_end }));
                CmdResult::Success
            }
            "inventory" => self.inventory_command(a),
            "addxp" => {
                let points = a.get_float() as i64;
                let gained = self.player.add_xp(points);
                if let Some(player) = a.world.player {
                    for _ in 0..gained {
                        a.world.send_event(self, None, player, "level_up", Vec::new());
                    }
                }
                CmdResult::Success
            }
            "quest" => {
                let w = a.get_word();
                let key = speech_key(&a.string_var(&w));
                self.player.quests.push(key);
                self.messages.push("Quest book updated".to_owned());
                CmdResult::Success
            }
            "addbag" => {
                self.player.bags = (self.player.bags + 1).min(MAX_BAGS);
                CmdResult::Success
            }
            "rotate" => {
                let delta = [a.get_float(), a.get_float(), a.get_float()];
                let st = self.state_mut(me);
                for (r, d) in st.rotation.iter_mut().zip(delta) {
                    *r += d;
                }
                CmdResult::Success
            }
            "note" => {
                let kind = match a.get_word().as_str() {
                    "notice" => NoteKind::Notice,
                    "book" => NoteKind::Book,
                    "note" => NoteKind::Note,
                    other => {
                        a.warn(&format!("unexpected note type: {other}"));
                        NoteKind::Note
                    }
                };
                let w = a.get_word();
                let text = a.string_var(&w);
                self.notes.push(NoteRequest { kind, text });
                CmdResult::Success
            }
            "playerstacksize" => {
                let n = a.get_float().max(1.0) as u32;
                self.state_mut(me).stack_size = n;
                CmdResult::Success
            }
            "setfood" => {
                let v = a.get_float();
                self.state_mut(me).food = v;
                CmdResult::Success
            }
            "setweight" => {
                let v = a.get_float();
                self.state_mut(me).weight = v;
                CmdResult::Success
            }
            "setprice" => {
                let v = a.get_float();
                self.state_mut(me).price = v;
                CmdResult::Success
            }
            "eatme" => {
                let kind = a.world.entity(me).kind;
                if kind == EntityKind::Item {
                    let food = self.state(me).map_or(0.0, |s| s.food);
                    self.player.eat(food);
                }
                self.destroy_delayed(me, kind);
                CmdResult::Success
            }
            "specialfx" => {
                match a.get_word().as_str() {
                    "heal" => {
                        let v = a.get_float();
                        self.player.life.add(v);
                    }
                    "mana" => {
                        let v = a.get_float();
                        self.player.mana.add(v);
                    }
                    "newspell" => a.skip_word(),
                    // Visual effects (torches, fire, ...): not implemented, ignore the rest of the line.
                    _ => {
                        a.skip_command();
                    }
                }
                CmdResult::Success
            }
            "destroy" => {
                let w = a.get_word();
                let target = a.string_var(&w);
                match a.world.find(&target, me) {
                    Some(t) => {
                        let kind = a.world.entity(t).kind;
                        let gone = self.destroy_delayed(t, kind);
                        if t == me && gone { CmdResult::AbortAccept } else { CmdResult::Success }
                    }
                    None => CmdResult::Success,
                }
            }
            _ => return None,
        })
    }
}

/// `[key]` -> `key`, lowercased.
fn speech_key(text: &str) -> String {
    let t = text.trim();
    t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(t).to_ascii_lowercase()
}

/// `speak -c <kind> ...`: cinematic camera arguments. Cameras are not implemented, but the words must be
/// consumed so the rest of the line parses.
fn skip_cinematic_arguments(a: &mut Args) {
    let words = match a.get_word().as_str() {
        "zoom" | "side" | "side_l" | "side_r" => 6,
        "ccctalker_l" | "ccctalker_r" | "ccclistener_l" | "ccclistener_r" => 3,
        _ => 0, // "keep" and anything unknown
    };
    for _ in 0..words {
        a.skip_word();
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
    fn speak_and_herosay_queue_speech_and_messages() {
        let (mut w, mut h, id) = setup(
            "x/npc/guard/guard",
            EntityKind::Npc,
            "on chat {\n speak [Goblin_Hail] herosay done\n speak -p player_wow\n speak -to whisper\n herosay [description_door]\n herosay -d debug\n speak killall\n speak \"\"\n accept\n}",
        );
        let player = w.add_entity(EntityKind::Player, "x/npc/player/player", 1, None, None);
        w.player = Some(player);
        w.send_event(&mut h, None, id, "chat", vec![]);
        let ev = h.take_speech();
        assert_eq!(ev.len(), 5, "{ev:?}");
        let SpeechEvent::Say(first) = &ev[0] else { panic!("{:?}", ev[0]) };
        assert_eq!(first.key, "goblin_hail");
        assert_eq!(first.speaker, id);
        let (script, pos) = first.on_end.as_ref().expect("the rest of the line runs when the speech ends");
        assert!(String::from_utf8_lossy(&script.data[*pos..]).trim_start().starts_with("herosay"));
        let SpeechEvent::Say(second) = &ev[1] else { panic!() };
        assert_eq!(second.speaker, player, "-p makes the player speak");
        assert!(second.on_end.is_none());
        let SpeechEvent::Say(third) = &ev[2] else { panic!() };
        assert!(third.flags.no_text && third.flags.off_voice);
        assert!(matches!(ev[3], SpeechEvent::KillAll));
        assert!(matches!(ev[4], SpeechEvent::Clear(e) if e == id));
        assert_eq!(h.take_messages(), ["[description_door]"], "the debug herosay is dropped");
        // When the speech ends the application runs the rest of the line.
        let (script, pos) = first.on_end.clone().unwrap();
        w.run_line(&mut h, id, &script, pos);
        assert_eq!(h.take_messages(), ["done"]);
    }

    #[test]
    fn eating_heals_feeds_and_uses_up_one_of_a_stack() {
        let (mut w, mut h, id) = setup(
            "graph/obj3d/interactive/items/provisions/applepie/applepie",
            EntityKind::Item,
            "on init {
 playerstacksize 10
 setfood 14
 setweight 1
 setprice 40
 accept
}
on inventoryuse {
 specialfx heal 4
 specialfx fiery
 specialfx mana 2
 eatme
 accept
}",
        );
        w.send_init(&mut h, id);
        let st = h.state(id).unwrap();
        assert_eq!((st.stack_size, st.food, st.weight, st.price), (10, 14.0, 1.0, 40.0));
        h.modify(id, |s| s.count = 2);
        h.player.life.current = 3.0;
        h.player.hunger = 20.0;
        w.send_event(&mut h, None, id, "inventoryuse", vec![]);
        assert_eq!(h.player.life.current, 7.0);
        assert_eq!(h.player.mana.current, 6.0, "already full");
        assert_eq!(h.player.hunger, 76.0);
        assert_eq!(h.state(id).unwrap().count, 1);
        assert!(!h.state(id).unwrap().destroyed, "one is left");
        w.send_event(&mut h, None, id, "inventoryuse", vec![]);
        assert!(h.state(id).unwrap().destroyed, "the last one is gone");
        h.player.inventory.push(id);
        h.prune_inventory();
        assert!(h.player.inventory.is_empty());
    }

    #[test]
    fn rotate_accumulates_angles() {
        let (mut w, mut h, id) = setup("x/fix_inter/puzzle_wheel/puzzle_wheel", EntityKind::Fix, "on action {\n rotate 0 90 0\n rotate 0 -30 5\n accept\n}");
        w.send_event(&mut h, None, id, "action", vec![]);
        w.send_event(&mut h, None, id, "action", vec![]);
        assert_eq!(h.state(id).unwrap().rotation, [0.0, 120.0, 10.0]);
    }

    #[test]
    fn note_queues_something_to_read() {
        let (mut w, mut h, id) = setup(
            "graph/obj3d/interactive/fix_inter/public_notice/public_notice",
            EntityKind::Fix,
            "on action {\n note notice [public_notice_a]\n note book \"some text\"\n note scrap oops\n accept\n}",
        );
        w.send_event(&mut h, None, id, "action", vec![]);
        let n = h.take_notes();
        assert_eq!(n.len(), 3);
        assert_eq!((n[0].kind, n[0].text.as_str()), (NoteKind::Notice, "[public_notice_a]"));
        assert_eq!((n[1].kind, n[1].text.as_str()), (NoteKind::Book, "some text"));
        assert_eq!(n[2].kind, NoteKind::Note, "unknown types fall back to a note");
        assert!(h.take_notes().is_empty());
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
