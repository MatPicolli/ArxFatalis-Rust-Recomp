//! A [`Host`] that implements the commands which change how entities look and behave, keeping the
//! result as plain data per entity for the renderer to apply. Commands it does not know are skipped
//! by the interpreter (the rest of their line is ignored), so gameplay-only commands cost nothing.

use crate::interp::{Args, CmdResult, Host, has_flag};
use crate::player::{EquipSlot, MAX_BAGS, PlayerState};
use crate::text::Script;
use crate::world::{EntityId, EntityKind, ScriptWorld, Timer};
use std::{collections::HashMap, sync::Arc};

/// What an item is (`setobjecttype`), as flags.
pub mod object_type {
    pub const WEAPON: u32 = 1 << 0;
    pub const DAGGER: u32 = 1 << 1;
    pub const ONE_HANDED: u32 = 1 << 2;
    pub const TWO_HANDED: u32 = 1 << 3;
    pub const BOW: u32 = 1 << 4;
    pub const SHIELD: u32 = 1 << 5;
    pub const RING: u32 = 1 << 6;
    pub const ARMOR: u32 = 1 << 7;
    pub const HELMET: u32 = 1 << 8;
    pub const LEGGINGS: u32 = 1 << 9;
    /// Weapons the hero holds in the hand.
    pub const HELD: u32 = DAGGER | ONE_HANDED | TWO_HANDED | BOW;
}

/// A number an item adds to the hero (`setequip`): an amount, or a percentage of the value it modifies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EquipValue {
    pub value: f32,
    pub percent: bool,
}

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
    /// `physical off`: the entity does not fall, walk or collide (a hanging corpse).
    pub physical_off: bool,
    /// `physical radius` / `physical height`: the collision cylinder the script asked for (before scaling).
    pub radius: Option<f32>,
    pub height: Option<f32>,
    /// Items: what it is (see [`object_type`]).
    pub type_flags: u32,
    /// Items: what it adds to whoever wears it (`setequip`), by modifier name.
    pub equip: HashMap<String, EquipValue>,
    /// Items: special effects (`setequip -s paralyse 500`).
    pub specials: Vec<(String, f32)>,
    /// What a weapon or a creature's attack is made of (`setweaponmaterial`), what armour or hide is (`setarmormaterial`)
    /// and what feet wear (`setstepmaterial`).
    pub weapon_material: String,
    pub armor_material: String,
    pub step_material: String,
    pub durability: f32,
    pub max_durability: f32,
    /// Worn or wielded by the hero (and so in neither the world nor the grid).
    pub equipped: bool,
    /// What the hero shouts when a weapon's blow is well aimed (`setstrikespeech`).
    pub strike_speech: String,
    /// What the entity looks at or follows (`settarget`); characters act on it, cameras look at it.
    pub target: TargetSpec,
    /// Cameras: focal length (`camerafocal`, 100..800; the engine's default view is 350), how slowly the view follows
    /// its target (`camerasmoothing`) and an offset added to the target (`cameratranslatetarget`, Arx coordinates).
    pub cam_focal: f32,
    pub cam_smoothing: f32,
    pub cam_translate: [f32; 3],
    /// Cannot be hurt (`invulnerability on`).
    pub invulnerable: bool,
    /// Casts a shadow on the floor (`setshadow off` removes it).
    pub shadow: bool,
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
            physical_off: false,
            radius: None,
            height: None,
            type_flags: 0,
            equip: HashMap::new(),
            specials: Vec::new(),
            weapon_material: String::new(),
            armor_material: String::new(),
            step_material: String::new(),
            durability: 0.0,
            max_durability: 0.0,
            equipped: false,
            strike_speech: String::new(),
            target: TargetSpec::None,
            cam_focal: 350.0,
            cam_smoothing: 0.0,
            cam_translate: [0.0; 3],
            invulnerable: false,
            shadow: true,
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

/// What `settarget` points an entity at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSpec {
    None,
    /// The entity's patrol path (`settarget path`).
    Path,
    Entity(EntityId),
}

/// Commands that steer or configure a character, kept in the order scripts gave them for the NPC runtime
/// (`arx_level::npc`) to carry out; this crate only reads them.
#[derive(Debug, Clone, PartialEq)]
pub enum NpcRequest {
    /// `behavior [-lsdmfa] <command> [param]`: `flags` is the letters given, `command` the word after them.
    Behavior { entity: EntityId, flags: String, command: String, param: f32 },
    /// `settarget [-san] <target>`
    SetTarget { entity: EntityId, flags: String, target: TargetSpec },
    /// `setmovemode walk|run|sneak|none`
    MoveMode { entity: EntityId, mode: String },
    /// `setnpcstat <name> <value>`
    Stat { entity: EntityId, name: String, value: f32 },
    /// `setdetect <0..100|off>` (-1 for off)
    Detect { entity: EntityId, value: i32 },
    Speed { entity: EntityId, value: f32 },
    XpValue { entity: EntityId, value: f32 },
    Life { entity: EntityId, value: f32 },
    Revive { entity: EntityId, init: bool },
    ForceDeath { target: EntityId, killer: EntityId },
    /// `pathfind <target>`
    Pathfind { entity: EntityId, target: Option<EntityId> },
}

/// A fade of the whole picture (`worldfade`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    /// To the colour (`out`) or back from it (`in`).
    pub out: bool,
    pub duration_ms: f32,
    pub color: [f32; 3],
    /// Script time when it began.
    pub started_ms: f64,
}

/// Something a script asked of the stage: moving along the level's paths, jumping somewhere, turning the hero's head.
#[derive(Debug, Clone, PartialEq)]
pub enum StageRequest {
    /// `setpath [-wf] <name>|none`: follow a path of the level from its beginning (or stop following).
    SetPath { entity: EntityId, name: Option<String>, worm: bool, follow_direction: bool },
    /// `usepath f|b|p`: go forward, backward, or pause on the path.
    UsePath { entity: EntityId, mode: char },
    /// `teleport <target>`: the entity jumps to where `to` is. `teleport -i` sends it back where it started.
    Teleport { entity: EntityId, to: Option<EntityId> },
    /// `teleport -p <target>`: the hero jumps there; `yaw` (engine degrees) if `-a` gave one.
    TeleportPlayer { to: EntityId, yaw: Option<f32> },
    /// `teleport -l <level> <target>`: go to another level.
    ChangeLevel { level: u32, target: String, yaw: Option<f32> },
    /// `playerlookat <entity>`
    LookAt { entity: EntityId },
    /// `cine <name>`: a 2D cinematic was asked for by `entity`.
    Cinematic { entity: EntityId, name: String },
}

/// What scripts have done to the way the game is shown and played: the cutscene state.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    /// The player may move and look (`setplayercontrols`).
    pub controls: bool,
    /// Black bars above and below (`cinemascope`).
    pub cinemascope: bool,
    /// The HUD is hidden (`playerinterface hide`).
    pub interface_hidden: bool,
    pub fade: Option<Fade>,
    /// The camera entity the scene is seen through (`cameraactivate`), instead of the hero's eyes.
    pub camera: Option<EntityId>,
    /// The hero cannot be hurt (`invulnerability -p on`).
    pub player_invulnerable: bool,
}

impl Default for Stage {
    fn default() -> Self {
        Stage { controls: true, cinemascope: false, interface_hidden: false, fade: None, camera: None, player_invulnerable: false }
    }
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
    npc_requests: Vec<NpcRequest>,
    /// The weapon item each character wields (`setweapon`).
    pub npc_weapons: HashMap<EntityId, EntityId>,
    /// Which entity watches each zone (`setcontrolledzone`), by lowercased zone name.
    pub controlled_zones: HashMap<String, EntityId>,
    /// Cutscene state: camera, fade, bars, whether the player has control.
    pub stage: Stage,
    stage_requests: Vec<StageRequest>,
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

    /// How long, in milliseconds, the animation in an entity's slot lasts (if it is loaded and measurable).
    pub fn slot_duration_ms(&self, entity: EntityId, slot: &str) -> Option<f64> {
        let path = self.state(entity)?.anims.get(slot)?;
        self.anim_duration.as_ref()?(path)
    }

    /// Does the entity have an animation in this slot?
    pub fn has_slot(&self, entity: EntityId, slot: &str) -> bool {
        self.state(entity).is_some_and(|s| s.anims.contains_key(slot))
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

    /// Put `item` on the hero: it goes to the slot its type says, and whatever was there comes off and returns to the
    /// pack (or the floor). A two-handed weapon or a bow takes the shield off, and a shield a two-handed weapon.
    pub fn equip(&mut self, world: &mut ScriptWorld, item: EntityId) {
        use object_type as t;
        let Some(player) = world.player else { return };
        let flags = self.state(item).map_or(0, |s| s.type_flags);
        if flags & (t::HELD | t::SHIELD | t::RING | t::ARMOR | t::LEGGINGS | t::HELMET) == 0 || self.player.is_equipped(item) {
            return;
        }
        // It leaves the pack and the world.
        self.player.remove_item(item);
        self.modify(item, |s| {
            s.in_inventory = false;
            s.hidden = true;
            s.collision = false;
            s.equipped = true;
        });
        if flags & t::HELD != 0 {
            self.release_slot(world, EquipSlot::Weapon);
            self.player.equipped[EquipSlot::Weapon as usize] = Some(item);
            if flags & (t::TWO_HANDED | t::BOW) != 0 {
                self.release_slot(world, EquipSlot::Shield);
            }
        } else if flags & t::SHIELD != 0 {
            self.release_slot(world, EquipSlot::Shield);
            self.player.equipped[EquipSlot::Shield as usize] = Some(item);
            let weapon_flags = self.player.equipped_in(EquipSlot::Weapon).and_then(|w| self.state(w)).map_or(0, |s| s.type_flags);
            if weapon_flags & (t::TWO_HANDED | t::BOW) != 0 {
                self.release_slot(world, EquipSlot::Weapon);
            }
        } else if flags & t::RING != 0 {
            let slot = match (self.player.equipped_in(EquipSlot::RingLeft), self.player.equipped_in(EquipSlot::RingRight)) {
                (None, _) => EquipSlot::RingLeft,
                (_, None) => EquipSlot::RingRight,
                _ => {
                    self.release_slot(world, EquipSlot::RingLeft);
                    EquipSlot::RingLeft
                }
            };
            self.player.equipped[slot as usize] = Some(item);
        } else {
            let slot = if flags & t::ARMOR != 0 {
                EquipSlot::Armor
            } else if flags & t::LEGGINGS != 0 {
                EquipSlot::Leggings
            } else {
                EquipSlot::Helmet
            };
            self.release_slot(world, slot);
            self.player.equipped[slot as usize] = Some(item);
        }
        self.recompute_equipment();
        let _ = player;
    }

    /// Take `item` off the hero. It returns to the pack, or to the floor at the hero's feet if there is no room
    /// (`destroyed` items are just gone). Both it and the hero's script hear `equipout`.
    pub fn unequip(&mut self, world: &mut ScriptWorld, item: EntityId, destroyed: bool) {
        let Some(player) = world.player else { return };
        let Some(i) = self.player.equipped.iter().position(|&e| e == Some(item)) else { return };
        self.player.equipped[i] = None;
        self.modify(item, |s| s.equipped = false);
        if destroyed {
            self.modify(item, |s| s.destroyed = true);
        } else if self.carry(world, item) == Carry::Full {
            let at = world.entity(player).pos;
            self.modify(item, |s| {
                s.hidden = false;
                s.in_inventory = false;
                s.collision = true;
                s.moved_to = Some(at);
            });
            world.entity_mut(item).pos = at;
            self.note_dropped(item);
            self.push_message("Your inventory is full".to_owned());
        }
        world.queue_event(Some(player), item, "equipout", Vec::new());
        world.queue_event(Some(item), player, "equipout", Vec::new());
        self.recompute_equipment();
    }

    fn release_slot(&mut self, world: &mut ScriptWorld, slot: EquipSlot) {
        if let Some(old) = self.player.equipped_in(slot) {
            self.unequip(world, old, false);
        }
    }

    /// Add up what the worn items give (their `setequip` values) into the hero's modifiers, then bring life and mana
    /// maximums in line.
    pub fn recompute_equipment(&mut self) {
        let mut mods = crate::player::EquipMods::default();
        for slot in EquipSlot::ALL {
            let Some(item) = self.player.equipped_in(slot) else { continue };
            let Some(st) = self.state(item) else { continue };
            for (name, v) in &st.equip {
                if v.percent {
                    *mods.rel.entry(name.clone()).or_default() += v.value * 0.01;
                } else {
                    *mods.abs.entry(name.clone()).or_default() += v.value;
                }
            }
        }
        self.player.mods = mods;
        self.player.recompute();
    }

    /// The weapon the hero holds, if any.
    pub fn player_weapon(&self) -> Option<EntityId> {
        self.player.equipped_in(EquipSlot::Weapon)
    }

    /// Paths, teleports and such that scripts asked for since the last call, in order.
    pub fn take_stage_requests(&mut self) -> Vec<StageRequest> {
        std::mem::take(&mut self.stage_requests)
    }

    /// Character commands scripts gave since the last call, in order.
    pub fn take_npc_requests(&mut self) -> Vec<NpcRequest> {
        std::mem::take(&mut self.npc_requests)
    }

    /// Queue a character command as a script would (used by the game itself, and in tests).
    pub fn push_npc_request(&mut self, r: NpcRequest) {
        self.npc_requests.push(r);
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
                let positional = !has_flag(&flags, 'o') && a.ctx.event != "inventoryuse" && a.ctx.event != "equipin";
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
            "cameraactivate" => {
                let w = a.get_word();
                if w == "none" {
                    self.stage.camera = None;
                    return Some(CmdResult::Success);
                }
                match a.world.find(&w, me).filter(|&t| a.world.entity(t).kind == EntityKind::Camera) {
                    Some(t) => {
                        self.stage.camera = Some(t);
                        CmdResult::Success
                    }
                    None => CmdResult::Failed,
                }
            }
            "camerasmoothing" => {
                let v = a.get_float();
                self.state_mut(me).cam_smoothing = v;
                CmdResult::Success
            }
            "camerafocal" => {
                let v = a.get_float().clamp(100.0, 800.0);
                self.state_mut(me).cam_focal = v;
                CmdResult::Success
            }
            "cameratranslatetarget" => {
                let v = [a.get_float(), a.get_float(), a.get_float()];
                self.state_mut(me).cam_translate = v;
                CmdResult::Success
            }
            "cinemascope" => {
                let _flags = a.get_flags();
                self.stage.cinemascope = a.get_bool();
                CmdResult::Success
            }
            "worldfade" => {
                let dir = a.get_word();
                let duration_ms = a.get_float();
                let started_ms = a.world.now_ms;
                match dir.as_str() {
                    "out" => {
                        let color = [a.get_float(), a.get_float(), a.get_float()];
                        self.stage.fade = Some(Fade { out: true, duration_ms, color, started_ms });
                    }
                    "in" => {
                        let color = self.stage.fade.map_or([0.0; 3], |f| f.color);
                        self.stage.fade = Some(Fade { out: false, duration_ms, color, started_ms });
                    }
                    other => {
                        a.warn(&format!("unexpected fade direction: {other}"));
                        return Some(CmdResult::Failed);
                    }
                }
                CmdResult::Success
            }
            "setplayercontrols" => {
                let on = a.get_bool();
                if on != self.stage.controls {
                    // Every character hears that the player can (or cannot) act.
                    let event = if on { "controls_on" } else { "controls_off" };
                    let npcs: Vec<EntityId> = a.world.entities.iter().filter(|e| e.kind == EntityKind::Npc).map(|e| e.id).collect();
                    for n in npcs {
                        a.world.queue_event(Some(me), n, event, Vec::new());
                    }
                }
                self.stage.controls = on;
                if !on {
                    self.player.fighting = false;
                }
                CmdResult::Success
            }
            "playerinterface" => {
                let _flags = a.get_flags();
                match a.get_word().as_str() {
                    "hide" => self.stage.interface_hidden = true,
                    "show" => self.stage.interface_hidden = false,
                    other => {
                        a.warn(&format!("unknown command: {other}"));
                        return Some(CmdResult::Failed);
                    }
                }
                CmdResult::Success
            }
            "playerlookat" => {
                let w = a.get_word();
                match a.world.find(&w, me) {
                    Some(entity) => {
                        self.stage_requests.push(StageRequest::LookAt { entity });
                        CmdResult::Success
                    }
                    None => CmdResult::Failed,
                }
            }
            "invulnerability" => {
                let flags = a.get_flags();
                let on = a.get_bool();
                if has_flag(&flags, 'p') {
                    self.stage.player_invulnerable = on;
                } else {
                    self.state_mut(me).invulnerable = on;
                }
                CmdResult::Success
            }
            "setpath" => {
                let flags = a.get_flags();
                let name = a.get_word();
                let name = (name != "none").then_some(name);
                self.stage_requests.push(StageRequest::SetPath { entity: me, name, worm: has_flag(&flags, 'w'), follow_direction: has_flag(&flags, 'f') });
                CmdResult::Success
            }
            "usepath" => {
                let mode = a.get_word().chars().next().unwrap_or('f');
                self.stage_requests.push(StageRequest::UsePath { entity: me, mode });
                CmdResult::Success
            }
            "teleport" => {
                let flags = a.get_flags();
                let yaw = has_flag(&flags, 'a').then(|| a.get_float());
                if has_flag(&flags, 'l') {
                    let level = a.get_float().max(0.0) as u32;
                    let target = a.get_word();
                    self.stage_requests.push(StageRequest::ChangeLevel { level, target, yaw });
                    return Some(CmdResult::Success);
                }
                if has_flag(&flags, 'i') {
                    self.stage_requests.push(StageRequest::Teleport { entity: me, to: None });
                    return Some(CmdResult::Success);
                }
                let w = a.get_word();
                if w == "behind" {
                    return Some(CmdResult::Success);
                }
                let Some(to) = a.world.find(&w, me) else {
                    a.warn(&format!("unknown target: {w}"));
                    return Some(CmdResult::Failed);
                };
                if has_flag(&flags, 'p') {
                    self.stage_requests.push(StageRequest::TeleportPlayer { to, yaw });
                } else {
                    self.stage_requests.push(StageRequest::Teleport { entity: me, to: Some(to) });
                }
                CmdResult::Success
            }
            "cine" => {
                let _flags = a.get_flags();
                let name = a.get_word();
                if !matches!(name.as_str(), "kill" | "preload") {
                    self.stage_requests.push(StageRequest::Cinematic { entity: me, name });
                } else if name == "preload" {
                    a.skip_word();
                }
                CmdResult::Success
            }
            "setshadow" => {
                let on = a.get_bool();
                self.state_mut(me).shadow = on;
                CmdResult::Success
            }
            "setcontrolledzone" => {
                let w = a.get_word();
                let zone = a.string_var(&w).to_ascii_lowercase();
                self.controlled_zones.insert(zone, me);
                CmdResult::Success
            }
            "unsetcontrolledzone" => {
                let w = a.get_word();
                let zone = a.string_var(&w).to_ascii_lowercase();
                self.controlled_zones.remove(&zone);
                CmdResult::Success
            }
            "setobjecttype" => {
                let flags = a.get_flags();
                let name = a.get_word();
                let flag = match name.chars().next() {
                    Some('w') => object_type::WEAPON,
                    Some('d') => object_type::DAGGER,
                    Some('1') => object_type::ONE_HANDED,
                    Some('2') => object_type::TWO_HANDED,
                    Some('b') => object_type::BOW,
                    Some('s') => object_type::SHIELD,
                    Some('r') => object_type::RING,
                    Some('a') => object_type::ARMOR,
                    Some('h') => object_type::HELMET,
                    Some('l') => object_type::LEGGINGS,
                    _ => {
                        a.warn(&format!("unknown object type: {name}"));
                        return Some(CmdResult::Failed);
                    }
                };
                let st = self.state_mut(me);
                if has_flag(&flags, 'r') {
                    st.type_flags &= !flag;
                } else {
                    st.type_flags |= flag;
                }
                CmdResult::Success
            }
            "setequip" => {
                let flags = a.get_flags();
                if has_flag(&flags, 'r') {
                    self.state_mut(me).specials.clear();
                }
                let name = a.get_word();
                let value = a.get_word();
                let percent = value.ends_with('%');
                let number = a.float_var(value.trim_end_matches('%'));
                let st = self.state_mut(me);
                if has_flag(&flags, 's') {
                    if st.specials.len() < 4 {
                        st.specials.push((name, number));
                    }
                } else {
                    st.equip.insert(name, EquipValue { value: number, percent });
                }
                CmdResult::Success
            }
            "setdurability" => {
                let flags = a.get_flags();
                let v = a.get_float();
                let st = self.state_mut(me);
                st.durability = v;
                if !has_flag(&flags, 'c') {
                    st.max_durability = v;
                }
                let (d, m) = (st.durability, st.max_durability);
                let e = a.world.entity_mut(me);
                e.props.insert("^durability".to_owned(), crate::Value::Float(d));
                e.props.insert("^maxdurability".to_owned(), crate::Value::Float(m));
                CmdResult::Success
            }
            "setstrikespeech" => {
                let w = a.get_word();
                self.state_mut(me).strike_speech = w;
                CmdResult::Success
            }
            "setweaponmaterial" => {
                let w = a.get_word();
                self.state_mut(me).weapon_material = w;
                CmdResult::Success
            }
            "setarmormaterial" => {
                let w = a.get_word();
                self.state_mut(me).armor_material = w;
                CmdResult::Success
            }
            "setstepmaterial" => {
                let w = a.get_word();
                self.state_mut(me).step_material = w;
                CmdResult::Success
            }
            "equip" => {
                let flags = a.get_flags();
                let w = a.get_word();
                let Some(target) = a.world.find(&w, me) else {
                    a.warn(&format!("unknown target: {w}"));
                    return Some(CmdResult::Failed);
                };
                if Some(target) != a.world.player {
                    return Some(CmdResult::Success);
                }
                if has_flag(&flags, 'r') {
                    self.unequip(a.world, me, false);
                } else {
                    a.world.queue_event(Some(target), me, "equipin", Vec::new());
                    self.equip(a.world, me);
                }
                CmdResult::Success
            }
            "setweapon" => {
                let _flags = a.get_flags();
                let w = a.get_word();
                let name = a.string_var(&w);
                if let Some(item) = self.spawn_item(a, &name) {
                    self.npc_weapons.insert(me, item);
                    CmdResult::Success
                } else {
                    CmdResult::Failed
                }
            }
            "physical" => {
                let kind = a.get_word();
                match kind.as_str() {
                    "on" => self.state_mut(me).physical_off = false,
                    "off" => self.state_mut(me).physical_off = true,
                    "height" => {
                        let v = a.get_float();
                        self.state_mut(me).height = Some(v.clamp(30.0, 165.0));
                    }
                    "radius" => {
                        let v = a.get_float();
                        self.state_mut(me).radius = Some(v.clamp(10.0, 40.0));
                    }
                    other => {
                        a.warn(&format!("unknown physical command: {other}"));
                        return Some(CmdResult::Failed);
                    }
                }
                CmdResult::Success
            }
            "behavior" => {
                let flags = a.get_flags();
                let command = a.get_word();
                let param = if matches!(command.as_str(), "flee" | "look_for" | "hide" | "wander_around") { a.get_float() } else { 0.0 };
                self.npc_requests.push(NpcRequest::Behavior { entity: me, flags, command, param });
                CmdResult::Success
            }
            "settarget" => {
                let flags = a.get_flags();
                let mut word = a.get_word();
                if word == "object" {
                    word = a.get_word();
                }
                let word = a.string_var(&word);
                let target = match word.as_str() {
                    "none" => TargetSpec::None,
                    "path" => TargetSpec::Path,
                    other => match a.world.find(other, me) {
                        Some(t) => TargetSpec::Entity(t),
                        None => TargetSpec::None,
                    },
                };
                self.state_mut(me).target = target;
                self.npc_requests.push(NpcRequest::SetTarget { entity: me, flags, target });
                CmdResult::Success
            }
            "setmovemode" => {
                let mode = a.get_word();
                self.npc_requests.push(NpcRequest::MoveMode { entity: me, mode });
                CmdResult::Success
            }
            "setnpcstat" => {
                let name = a.get_word();
                let value = a.get_float();
                self.npc_requests.push(NpcRequest::Stat { entity: me, name, value });
                CmdResult::Success
            }
            "setdetect" => {
                let w = a.get_word();
                let value = if w == "off" { -1 } else { (a.float_var(&w) as i32).clamp(-1, 100) };
                self.npc_requests.push(NpcRequest::Detect { entity: me, value });
                CmdResult::Success
            }
            "setspeed" => {
                let value = a.get_float().clamp(0.0, 10.0);
                self.npc_requests.push(NpcRequest::Speed { entity: me, value });
                CmdResult::Success
            }
            "setxpvalue" => {
                let value = a.get_float().max(0.0);
                self.npc_requests.push(NpcRequest::XpValue { entity: me, value });
                CmdResult::Success
            }
            "setlife" => {
                let value = a.get_float();
                self.npc_requests.push(NpcRequest::Life { entity: me, value });
                CmdResult::Success
            }
            "revive" => {
                let flags = a.get_flags();
                self.npc_requests.push(NpcRequest::Revive { entity: me, init: has_flag(&flags, 'i') });
                CmdResult::Success
            }
            "forcedeath" => {
                let w = a.get_word();
                match a.world.find(&w, me) {
                    Some(target) => {
                        self.npc_requests.push(NpcRequest::ForceDeath { target, killer: me });
                        CmdResult::Success
                    }
                    None => CmdResult::Failed,
                }
            }
            "pathfind" => {
                let w = a.get_word();
                let target = a.world.find(&w, me);
                self.npc_requests.push(NpcRequest::Pathfind { entity: me, target });
                CmdResult::Success
            }
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
                a.world.entity_mut(me).props.insert("^price".to_owned(), crate::Value::Float(v));
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
    fn equipping_fills_slots_swaps_what_was_there_and_changes_the_hero_numbers() {
        let latin = |src: &str| Arc::new(Script::new(&src.chars().map(|c| c as u8).collect::<Vec<u8>>()));
        let mut w = ScriptWorld::new();
        let mut h = StdHost::new();
        let player = w.add_entity(EntityKind::Player, "x/npc/player/player", 1, None, None);
        let item = |w: &mut ScriptWorld, name: &str, setup: &str| {
            let src = format!("on init {{\n {setup}\n accept\n}}\non inventoryuse {{\n equip player\n accept\n}}\non equipin {{\n set §worn 1\n accept\n}}\non equipout {{\n set §worn 0\n accept\n}}");
            w.add_entity(EntityKind::Item, &format!("graph/obj3d/interactive/items/{name}"), 1, Some(latin(&src)), None)
        };
        let sword = item(&mut w, "weapons/sword/sword", "setobjecttype weapon\n setobjecttype 1h\n setequip damages 4\n setequip aim_time 700");
        let dagger = item(&mut w, "weapons/dagger/dagger", "setobjecttype weapon\n setobjecttype dagger\n setequip damages 2");
        let axe = item(&mut w, "weapons/axe2/axe2", "setobjecttype 2h");
        let shield = item(&mut w, "armor/shield/shield", "setobjecttype shield\n setequip armor_class 3");
        let mail = item(&mut w, "armor/mail/mail", "setobjecttype armor\n setequip armor_class 50%\n setequip strength 2");
        for &i in &[sword, dagger, axe, shield, mail] {
            w.send_init(&mut h, i);
            assert_eq!(h.carry(&w, i), Carry::Added);
        }
        let base_damage = h.player.misc().damages;
        // Use the sword: it leaves the pack and is wielded; its damage adds to the hero's.
        w.send_event(&mut h, Some(player), sword, "inventoryuse", vec![]);
        w.update(&mut h, 0.0);
        assert_eq!(h.player_weapon(), Some(sword));
        assert!(!h.player.inventory.contains(&sword) && h.state(sword).unwrap().equipped);
        assert_eq!(w.entity(sword).vars.get_int("§worn"), 1, "equipin was sent");
        assert!((h.player.misc().damages - (base_damage + 4.0)).abs() < 1e-4);
        assert_eq!(h.player.aim_time_ms(), 1500.0, "never faster than a second and a half");
        // The dagger replaces it; the sword goes back to the pack and hears equipout.
        w.send_event(&mut h, Some(player), dagger, "inventoryuse", vec![]);
        w.update(&mut h, 0.0);
        assert_eq!(h.player_weapon(), Some(dagger));
        assert!(h.player.inventory.contains(&sword) && !h.state(sword).unwrap().equipped);
        assert_eq!(w.entity(sword).vars.get_int("§worn"), 0);
        assert!((h.player.misc().damages - (base_damage + 2.0)).abs() < 1e-4);
        // A shield, then a two-handed axe, which takes the shield off.
        w.send_event(&mut h, Some(player), shield, "inventoryuse", vec![]);
        assert_eq!(h.player.equipped_in(EquipSlot::Shield), Some(shield));
        assert!(h.player.misc().armor_class >= 4.0);
        w.send_event(&mut h, Some(player), axe, "inventoryuse", vec![]);
        w.update(&mut h, 0.0);
        assert_eq!(h.player_weapon(), Some(axe));
        assert_eq!(h.player.equipped_in(EquipSlot::Shield), None, "two hands on the axe");
        assert!(h.player.inventory.contains(&shield) && h.player.inventory.contains(&dagger));
        // Armour with a percentage and an attribute bonus.
        let before = h.player.misc().armor_class;
        w.send_event(&mut h, Some(player), mail, "inventoryuse", vec![]);
        assert_eq!(h.player.attributes_full().strength, 8.0);
        assert!(h.player.misc().armor_class > before, "50% more armour class: {} -> {}", before, h.player.misc().armor_class);
        // Taking it off restores the numbers.
        h.unequip(&mut w, mail, false);
        assert_eq!(h.player.attributes_full().strength, 6.0);
        assert!(h.player.inventory.contains(&mail));
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
