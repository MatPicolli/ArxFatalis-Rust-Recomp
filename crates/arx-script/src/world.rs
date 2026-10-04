//! The set of script-bearing entities, their variables, the event queue and script timers.

use crate::interp::{Host, ScriptResult, run_event};
use crate::text::Script;
use crate::vars::{Value, Vars};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub type EntityId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Player,
    Npc,
    Item,
    Fix,
    Marker,
    Camera,
}

impl EntityKind {
    /// Kind of entity implied by its class path.
    pub fn from_class(class: &str) -> Self {
        if class.contains("/npc/") {
            EntityKind::Npc
        } else if class.contains("/items/") {
            EntityKind::Item
        } else if class.contains("/system/camera") {
            EntityKind::Camera
        } else if class.contains("/system/") || class.contains("marker") {
            EntityKind::Marker
        } else {
            EntityKind::Fix
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScriptEntity {
    pub id: EntityId,
    /// `<class name>_<instance>`, e.g. `goblin_base_0001`; how other scripts address it.
    pub id_string: String,
    pub class: String,
    pub instance: i32,
    pub kind: EntityKind,
    /// Class script.
    pub script: Option<Arc<Script>>,
    /// Per-instance script, run before the class script.
    pub over_script: Option<Arc<Script>>,
    /// Local (`§`, `@`, `£`) variables.
    pub vars: Vars,
    /// Values for `^name` system variables that belong to this entity (set by the game).
    pub props: HashMap<String, Value>,
    pub disabled_events: HashSet<String>,
    pub timer_slots: [Option<f64>; 4],
    pub main_event: Option<String>,
    pub groups: HashSet<String>,
    /// World position (Arx coordinates), for distance queries.
    pub pos: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct QueuedEvent {
    pub sender: Option<EntityId>,
    pub target: EntityId,
    pub event: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub entity: EntityId,
    pub name: String,
    pub script: Arc<Script>,
    pub pos: usize,
    pub interval_ms: f64,
    pub start_ms: f64,
    /// 0 = repeat forever.
    pub count: i64,
}

/// Diagnostics gathered while running scripts.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub events_run: u64,
    pub commands_run: u64,
    pub unknown_commands: BTreeMap<String, usize>,
    pub warnings: BTreeMap<String, usize>,
    pub aborted_runaway: usize,
}

pub struct ScriptWorld {
    pub entities: Vec<ScriptEntity>,
    /// Global (`#`, `&`, `$`) variables.
    pub globals: Vars,
    /// `^name` system variables not tied to an entity (player stats, time, ...).
    pub sys: HashMap<String, Value>,
    pub player: Option<EntityId>,
    pub now_ms: f64,
    pub stats: Stats,
    index: HashMap<String, EntityId>,
    queue: VecDeque<QueuedEvent>,
    pub(crate) timers: Vec<Timer>,
    rng: u64,
}

impl Default for ScriptWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptWorld {
    pub fn new() -> Self {
        ScriptWorld {
            entities: Vec::new(),
            globals: Vars::default(),
            sys: HashMap::new(),
            player: None,
            now_ms: 0.0,
            stats: Stats::default(),
            index: HashMap::new(),
            queue: VecDeque::new(),
            timers: Vec::new(),
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    pub fn add_entity(
        &mut self,
        kind: EntityKind,
        class: &str,
        instance: i32,
        script: Option<Arc<Script>>,
        over_script: Option<Arc<Script>>,
    ) -> EntityId {
        let id = self.entities.len() as EntityId;
        let stem = class.rsplit('/').next().unwrap_or(class).to_ascii_lowercase();
        let id_string = format!("{stem}_{instance:04}");
        self.index.insert(id_string.clone(), id);
        if kind == EntityKind::Player {
            self.player = Some(id);
        }
        self.entities.push(ScriptEntity {
            id,
            id_string,
            class: class.to_owned(),
            instance,
            kind,
            script,
            over_script,
            vars: Vars::default(),
            props: HashMap::new(),
            disabled_events: HashSet::new(),
            timer_slots: [None; 4],
            main_event: None,
            groups: HashSet::new(),
            pos: [0.0; 3],
        });
        id
    }

    pub fn entity(&self, id: EntityId) -> &ScriptEntity {
        &self.entities[id as usize]
    }

    pub fn entity_mut(&mut self, id: EntityId) -> &mut ScriptEntity {
        &mut self.entities[id as usize]
    }

    /// Resolve how scripts name entities: `self`/`me`, `player`, `none`, or `<class>_<nnnn>`.
    pub fn find(&self, name: &str, from: EntityId) -> Option<EntityId> {
        match name {
            "" | "none" => None,
            "self" | "me" => Some(from),
            "player" => self.player,
            _ => self.index.get(name).copied(),
        }
    }

    /// Deterministic pseudo-random number in `[0, 1)`.
    pub(crate) fn random(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u32 << 24) as f32
    }

    pub(crate) fn warn(&mut self, msg: String) {
        *self.stats.warnings.entry(msg).or_default() += 1;
    }

    /// Queue an event for the next [`ScriptWorld::update`] (what `sendevent` does).
    pub fn queue_event(&mut self, sender: Option<EntityId>, target: EntityId, event: &str, params: Vec<String>) {
        self.queue.push_back(QueuedEvent { sender, target, event: event.to_ascii_lowercase(), params });
    }

    /// Deliver an event now: to the instance script first, then (unless refused) the class script.
    pub fn send_event(
        &mut self,
        host: &mut dyn Host,
        sender: Option<EntityId>,
        target: EntityId,
        event: &str,
        params: Vec<String>,
    ) -> ScriptResult {
        let event = event.to_ascii_lowercase();
        let over = self.entity(target).over_script.clone();
        if let Some(s) = over {
            let r = run_event(self, host, &s, sender, target, &event, params.clone(), 0, false);
            if matches!(r, ScriptResult::Refuse | ScriptResult::Destructive) {
                return r;
            }
        }
        match self.entity(target).script.clone() {
            Some(s) => run_event(self, host, &s, sender, target, &event, params, 0, false),
            None => ScriptResult::Accept,
        }
    }

    /// Run an entity's `on init` then `on initend` handlers (class script, then instance script).
    pub fn send_init(&mut self, host: &mut dyn Host, id: EntityId) {
        for event in ["init", "initend"] {
            for s in [self.entity(id).script.clone(), self.entity(id).over_script.clone()].into_iter().flatten() {
                run_event(self, host, &s, None, id, event, Vec::new(), 0, false);
            }
        }
    }

    /// Advance the clock, fire due timers, and deliver queued events.
    pub fn update(&mut self, host: &mut dyn Host, dt_ms: f64) {
        self.now_ms += dt_ms;
        self.fire_timers(host);
        for _ in 0..4096 {
            let Some(q) = self.queue.pop_front() else { break };
            self.send_event(host, q.sender, q.target, &q.event, q.params);
        }
    }

    fn fire_timers(&mut self, host: &mut dyn Host) {
        let mut i = 0;
        while i < self.timers.len() {
            let t = &self.timers[i];
            if t.start_ms + t.interval_ms > self.now_ms {
                i += 1;
                continue;
            }
            let (script, entity, pos) = (t.script.clone(), t.entity, t.pos);
            if t.count == 1 {
                self.timers.remove(i);
            } else {
                let t = &mut self.timers[i];
                if t.count != 0 {
                    t.count -= 1;
                }
                t.start_ms = if t.interval_ms == 0.0 { self.now_ms } else { t.start_ms + t.interval_ms };
                i += 1;
            }
            run_event(self, host, &script, None, entity, "", Vec::new(), pos, true);
        }
    }

    /// Run the single command line at `pos` of `script` as `entity` (a speech's "when finished" command).
    pub fn run_line(&mut self, host: &mut dyn Host, entity: EntityId, script: &Arc<Script>, pos: usize) {
        run_event(self, host, script, None, entity, "", Vec::new(), pos, true);
    }

    pub fn timer_count(&self) -> usize {
        self.timers.len()
    }

    pub(crate) fn add_timer(&mut self, t: Timer) {
        // A timer re-using a name replaces the old one.
        self.timers.retain(|o| !(o.entity == t.entity && o.name == t.name));
        self.timers.push(t);
    }

    /// Drop every timer of an entity (it died).
    pub fn clear_timers_for(&mut self, entity: EntityId) {
        self.timers.retain(|t| t.entity != entity);
    }

    pub(crate) fn clear_timer(&mut self, entity: EntityId, name: &str) {
        self.timers.retain(|o| !(o.entity == entity && o.name == name));
    }

    pub(crate) fn default_timer_name(&self, entity: EntityId, prefix: &str) -> String {
        (1..)
            .map(|n| format!("{prefix}_{n}"))
            .find(|name| !self.timers.iter().any(|t| t.entity == entity && &t.name == name))
            .unwrap()
    }
}
