//! Interpreter for Arx Fatalis entity scripts (`.asl`).
//!
//! The language is interpreted straight from text, like the original engine: a script is a
//! lowercased byte buffer, events are `on <name> { ... }` blocks found by searching for the header,
//! and every command reads its own arguments from the text as it executes. Game-facing commands
//! (animations, meshes, sounds, ...) are delegated to a [`Host`]; everything about control flow,
//! variables, events and timers lives here.

mod host;
mod interp;
mod player;
mod text;
mod vars;
mod world;

pub use host::{AnimDuration, Carry, IconSize, is_gold_class, NoteKind, NoteRequest, ScriptLoader, EntityState, Mood, PlayAnim, SoundRequest, SpeechEvent, SpeechFlags, SpeechRequest, StdHost};
pub use interp::{Args, CmdResult, Context, Host, ScriptResult, has_flag};
pub use player::{Attribute, Attributes, BAG_HEIGHT, BAG_WIDTH, MAX_BAGS, Misc, PlayerState, Pool, Skill, Skills, Slot, xp_for_level};
pub use text::Script;
pub use vars::{Value, Vars};
pub use world::{EntityId, EntityKind, QueuedEvent, ScriptEntity, ScriptWorld, Stats};
