//! The text interpreter: tokenizer, event runner, and the built-in language commands.

use crate::text::{Script, is_whitespace, latin1};
use crate::vars::{Value, is_local, parse_float};
use crate::world::{EntityId, ScriptWorld, Timer};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptResult {
    Accept,
    Refuse,
    Destructive,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdResult {
    Success,
    Failed,
    AbortAccept,
    AbortRefuse,
    AbortError,
    AbortDestructive,
    /// Execution continued somewhere else (`goto` / `gosub`).
    Jumped,
}

/// Implements the game-facing commands. `name` is the command with underscores removed.
/// Read the command's arguments through `args` (they must be consumed even if the command is
/// ignored, or the rest of the line is skipped for you when `None` is returned).
pub trait Host {
    fn command(&mut self, name: &str, args: &mut Args) -> Option<CmdResult>;
}

/// Execution state of one event handler.
pub struct Context {
    pub script: Arc<Script>,
    pub(crate) pos: usize,
    stack: Vec<usize>,
    pub entity: EntityId,
    pub sender: Option<EntityId>,
    pub event: String,
    pub params: Vec<String>,
}

/// Argument reader handed to commands: the script text cursor plus the world.
pub struct Args<'a> {
    pub ctx: &'a mut Context,
    pub world: &'a mut ScriptWorld,
}

/// Does `flags` (a `-abc` word) contain `letter`?
pub fn has_flag(flags: &str, letter: char) -> bool {
    flags.trim_start_matches('-').contains(letter)
}

impl Args<'_> {
    pub fn entity(&self) -> EntityId {
        self.ctx.entity
    }

    pub fn warn(&mut self, msg: &str) {
        let who = self.world.entity(self.ctx.entity).id_string.clone();
        self.world.warn(format!("{who}: {} {msg}", self.ctx.event));
    }

    fn data(&self) -> Arc<Script> {
        self.ctx.script.clone()
    }

    fn skip_whitespace(&mut self, skip_newlines: bool) {
        let s = self.data();
        let d = &s.data;
        while self.ctx.pos < d.len() && is_whitespace(d[self.ctx.pos]) {
            if d[self.ctx.pos] == b'\n' && !skip_newlines {
                return;
            }
            self.ctx.pos += 1;
        }
    }

    fn skip_line_comment(&mut self, d: &[u8]) {
        self.ctx.pos = d[self.ctx.pos..].iter().position(|&c| c == b'\n').map_or(d.len(), |p| self.ctx.pos + p);
    }

    /// Read a command name (no `~var~` substitution).
    pub fn get_command(&mut self, skip_newlines: bool) -> String {
        let s = self.data();
        let d = &s.data;
        self.skip_whitespace(skip_newlines);
        let mut word = Vec::new();
        while self.ctx.pos < d.len() && !is_whitespace(d[self.ctx.pos]) {
            let c = d[self.ctx.pos];
            if c == b'/' && d.get(self.ctx.pos + 1) == Some(&b'/') {
                self.skip_line_comment(d);
                if !word.is_empty() {
                    break;
                }
                self.skip_whitespace(skip_newlines);
                continue;
            }
            word.push(c);
            self.ctx.pos += 1;
        }
        latin1(&word)
    }

    /// Read an argument word: bare or `"quoted"`, with `~variable~` substitution.
    pub fn get_word(&mut self) -> String {
        let s = self.data();
        let d = &s.data;
        self.skip_whitespace(false);
        if self.ctx.pos >= d.len() {
            return String::new();
        }
        let mut word = String::new();
        let mut var = Vec::new();
        let mut in_var = false;
        let push = |this: &mut Self, c: u8, in_var: &mut bool, word: &mut String, var: &mut Vec<u8>| {
            if c == b'~' {
                if *in_var {
                    let name = latin1(var);
                    word.push_str(&this.string_var(&name));
                    var.clear();
                }
                *in_var = !*in_var;
            } else if *in_var {
                var.push(c);
            } else {
                word.push(c as char);
            }
        };
        if d[self.ctx.pos] == b'"' {
            self.ctx.pos += 1;
            while self.ctx.pos < d.len() && d[self.ctx.pos] != b'"' {
                let c = d[self.ctx.pos];
                if c == b'\n' {
                    return word;
                }
                push(self, c, &mut in_var, &mut word, &mut var);
                self.ctx.pos += 1;
            }
            if self.ctx.pos < d.len() {
                self.ctx.pos += 1;
            }
        } else {
            while self.ctx.pos < d.len() && !is_whitespace(d[self.ctx.pos]) {
                let c = d[self.ctx.pos];
                if c == b'/' && d.get(self.ctx.pos + 1) == Some(&b'/') && !in_var {
                    self.skip_line_comment(d);
                    break;
                }
                push(self, c, &mut in_var, &mut word, &mut var);
                self.ctx.pos += 1;
            }
        }
        word
    }

    pub fn skip_word(&mut self) {
        let s = self.data();
        let d = &s.data;
        self.skip_whitespace(false);
        if self.ctx.pos < d.len() && d[self.ctx.pos] == b'"' {
            self.ctx.pos += 1;
            while self.ctx.pos < d.len() && d[self.ctx.pos] != b'"' {
                if d[self.ctx.pos] == b'\n' {
                    return;
                }
                self.ctx.pos += 1;
            }
            if self.ctx.pos < d.len() {
                self.ctx.pos += 1;
            }
        } else {
            while self.ctx.pos < d.len() && !is_whitespace(d[self.ctx.pos]) {
                if d[self.ctx.pos] == b'/' && d.get(self.ctx.pos + 1) == Some(&b'/') {
                    self.skip_line_comment(d);
                    break;
                }
                self.ctx.pos += 1;
            }
        }
    }

    /// A `-flags` word if the next argument starts with `-`, otherwise empty.
    pub fn get_flags(&mut self) -> String {
        self.skip_whitespace(false);
        if self.data().data.get(self.ctx.pos) == Some(&b'-') { self.get_word() } else { String::new() }
    }

    pub fn get_float(&mut self) -> f32 {
        let w = self.get_word();
        self.float_var(&w)
    }

    pub fn get_bool(&mut self) -> bool {
        matches!(self.get_word().as_str(), "on" | "yes")
    }

    /// Skip the rest of the line; returns where it started (None if the line was empty/comment).
    pub fn skip_command(&mut self) -> Option<usize> {
        self.skip_whitespace(false);
        let s = self.data();
        let d = &s.data;
        if self.ctx.pos >= d.len() || d[self.ctx.pos] == b'\n' {
            return None;
        }
        let mut start = Some(self.ctx.pos);
        if d[self.ctx.pos] == b'/' && d.get(self.ctx.pos + 1) == Some(&b'/') {
            start = None;
            self.ctx.pos += 2;
        }
        self.skip_line_comment(d);
        start
    }

    /// Skip the next statement: a `{ ... }` block or a single line. A following `else` is consumed.
    pub fn skip_block(&mut self) {
        let word = self.get_command(true);
        let s = self.data();
        if self.ctx.pos >= s.data.len() {
            self.warn("missing statement before end of script");
            return;
        }
        if word == "{" {
            let mut depth = 1;
            while depth > 0 {
                self.skip_whitespace(true);
                let w = self.get_word();
                if self.ctx.pos >= s.data.len() {
                    self.warn("missing '}' before end of script");
                    return;
                }
                match w.as_str() {
                    "{" => depth += 1,
                    "}" => depth -= 1,
                    _ => {}
                }
            }
        } else {
            self.skip_command();
        }
        self.skip_whitespace(true);
        let old = self.ctx.pos;
        if self.get_command(true) != "else" {
            self.ctx.pos = old;
        }
    }

    fn jump_to_label(&mut self, label: &str, sub: bool) -> bool {
        if sub {
            self.ctx.stack.push(self.ctx.pos);
        }
        match self.ctx.script.find_pos(&format!(">>{label}")) {
            Some(p) => {
                self.ctx.pos = p;
                true
            }
            None => false,
        }
    }

    fn return_to_caller(&mut self) -> bool {
        match self.ctx.stack.pop() {
            Some(p) => {
                self.ctx.pos = p;
                true
            }
            None => false,
        }
    }

    // ---- variables ---------------------------------------------------------------------------

    fn vars_for(&mut self, name: &str) -> &mut crate::vars::Vars {
        if is_local(name) {
            let id = self.ctx.entity;
            &mut self.world.entity_mut(id).vars
        } else {
            &mut self.world.globals
        }
    }

    /// Value of a `^system` variable.
    pub fn sys_var(&mut self, name: &str) -> Value {
        let me = self.ctx.entity;
        let rest = &name[1..];
        let param = |kind: char| -> Option<usize> { rest.strip_prefix(kind).and_then(|r| r.strip_prefix("param")).and_then(|n| n.parse().ok()) };
        if let Some(i) = param('$') {
            return Value::Text(self.ctx.params.get(i.wrapping_sub(1)).cloned().unwrap_or_default());
        }
        if let Some(i) = param('&') {
            return Value::Float(self.ctx.params.get(i.wrapping_sub(1)).map_or(0.0, |p| parse_float(p)));
        }
        if let Some(i) = param('#') {
            return Value::Int(self.ctx.params.get(i.wrapping_sub(1)).map_or(0.0, |p| parse_float(p)) as i64);
        }
        let dist_to = |w: &ScriptWorld, other: EntityId| -> f32 {
            let (a, b) = (w.entity(me).pos, w.entity(other).pos);
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        match rest {
            "me" => return Value::Text(self.world.entity(me).id_string.clone()),
            "sender" => {
                let s = self.ctx.sender.map_or("none".to_owned(), |s| self.world.entity(s).id_string.clone());
                return Value::Text(s);
            }
            "&playerdist" | "#playerdist" => {
                let d = self.world.player.map_or(99_999_999.0, |p| dist_to(self.world, p));
                return if rest.starts_with('#') { Value::Int(d as i64) } else { Value::Float(d) };
            }
            "#timer1" | "#timer2" | "#timer3" | "#timer4" => {
                let slot = (rest.as_bytes()[6] - b'1') as usize;
                let v = self.world.entity(me).timer_slots[slot].map_or(0.0, |t| self.world.now_ms - t);
                return Value::Int(v as i64);
            }
            _ => {}
        }
        if let Some(max) = rest.strip_prefix("rnd_") {
            return Value::Float(self.world.random() * parse_float(max));
        }
        if let Some(target) = rest.strip_prefix("dist_") {
            let d = self.world.find(target, me).map_or(99_999_999_999.0, |o| dist_to(self.world, o));
            return Value::Float(d);
        }
        if let Some(v) = self.world.entity(me).props.get(name).or_else(|| self.world.sys.get(name)) {
            return v.clone();
        }
        // What the game keeps per entity, before it has said anything about it: nobody speaks, nothing is targeted.
        match rest {
            "gameseconds" => Value::Int((self.world.now_ms / 1000.0) as i64),
            "target" => Value::Text("none".to_owned()),
            "speaking" | "life" | "mana" | "fighting" | "playercasting" | "inplayerinventory" | "poisoned" | "gore" | "demo" | "price" => Value::Int(0),
            _ if rest.starts_with("playerspell_") || rest.starts_with("myspell_") || rest.starts_with("possess_") => Value::Int(0),
            _ => Value::Text(String::new()),
        }
    }

    /// Value of `name` as text: variables are looked up, anything else is a literal.
    pub fn string_var(&mut self, name: &str) -> String {
        match name.chars().next() {
            None => String::new(),
            Some('^') => self.sys_var(name).to_text(),
            Some('#' | '\u{a7}') => self.vars_for(name).get_int(name).to_string(),
            Some('&' | '@') => crate::vars::format_float(self.vars_for(name).get_float(name)),
            Some('$' | '\u{a3}') => self.vars_for(name).get_text(name).unwrap_or("void").to_owned(),
            _ => name.to_owned(),
        }
    }

    /// Numeric value of `name`: variables are looked up, anything else is parsed.
    pub fn float_var(&mut self, name: &str) -> f32 {
        match name.chars().next() {
            None => 0.0,
            Some('^') => self.sys_var(name).as_float(),
            Some('#' | '\u{a7}') => self.vars_for(name).get_int(name) as f32,
            Some('&' | '@') => self.vars_for(name).get_float(name),
            _ => parse_float(name),
        }
    }
}

// ---- event runner -----------------------------------------------------------------------------

const MAX_COMMANDS_PER_EVENT: usize = 200_000;

/// Run an event handler (or, with `exec_line`, the single command line at `position`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_event(
    world: &mut ScriptWorld,
    host: &mut dyn Host,
    script: &Arc<Script>,
    sender: Option<EntityId>,
    entity: EntityId,
    event: &str,
    params: Vec<String>,
    position: usize,
    exec_line: bool,
) -> ScriptResult {
    world.stats.events_run += 1;
    if !exec_line && world.entity(entity).disabled_events.contains(event) {
        return ScriptResult::Refuse;
    }
    let mut exec_line = exec_line;
    let pos = if exec_line {
        position
    } else {
        match script.find_pos(&format!("on {event}")) {
            Some(p) => p,
            None => return ScriptResult::Accept,
        }
    };
    let mut ctx = Context {
        script: script.clone(),
        pos,
        stack: Vec::new(),
        entity,
        sender,
        event: event.to_owned(),
        params,
    };

    if !exec_line {
        let first = Args { ctx: &mut ctx, world }.get_command(true);
        if first != "{" {
            let mut a = Args { ctx: &mut ctx, world };
            a.warn(&format!("missing bracket after event, got \"{first}\""));
            return ScriptResult::Accept;
        }
    }

    // Brackets are not tracked after a jump.
    let mut brackets: i64 = 1;
    for _ in 0..MAX_COMMANDS_PER_EVENT {
        let mut a = Args { ctx: &mut ctx, world };
        let word = a.get_command(!exec_line);
        if word.is_empty() {
            if exec_line && a.ctx.pos != a.ctx.script.data.len() {
                return ScriptResult::Accept; // end of the line
            }
            a.warn("reached script end without accept / refuse / return");
            return ScriptResult::Accept;
        }
        let name = word.replace('_', "");
        a.world.stats.commands_run += 1;

        let result = match builtin(&name, &mut a) {
            Some(r) => Some(r),
            None => host.command(&name, &mut a),
        };
        match result {
            Some(CmdResult::AbortAccept) => return ScriptResult::Accept,
            Some(CmdResult::AbortRefuse) => return ScriptResult::Refuse,
            Some(CmdResult::AbortError) => return ScriptResult::Error,
            Some(CmdResult::AbortDestructive) => return ScriptResult::Destructive,
            Some(CmdResult::Jumped) => {
                exec_line = false;
                brackets = -1;
            }
            Some(CmdResult::Success | CmdResult::Failed) => {}
            None => {
                if name.starts_with(">>") {
                    a.skip_command(); // label
                } else if let Some(timer_name) = name.strip_prefix("timer") {
                    timer_command(timer_name, &mut a);
                } else if name == "{" {
                    if brackets != -1 {
                        brackets += 1;
                    }
                } else if name == "}" {
                    if brackets != -1 {
                        brackets -= 1;
                        if brackets == 0 {
                            a.warn("event block ended without accept or refuse");
                            return ScriptResult::Accept;
                        }
                    }
                } else {
                    *a.world.stats.unknown_commands.entry(name).or_default() += 1;
                    a.skip_command();
                }
            }
        }
    }
    world.stats.aborted_runaway += 1;
    ScriptResult::Error
}

fn timer_command(name: &str, a: &mut Args) {
    let flags = a.get_flags();
    let millis = has_flag(&flags, 'm');
    let command = a.get_word();
    let entity = a.ctx.entity;
    if command == "kill_local" {
        a.world.timers.retain(|t| t.entity != entity || !t.name.starts_with("timer_"));
        return;
    }
    if !name.is_empty() {
        a.world.clear_timer(entity, name);
    }
    if command == "off" {
        return;
    }
    let count = a.float_var(&command) as i64;
    let interval = a.get_float();
    if count < 0 || interval < 0.0 {
        a.warn("timer count and interval must not be negative");
        return;
    }
    let timer_name = if name.is_empty() { a.world.default_timer_name(entity, "timer") } else { name.to_owned() };
    let Some(pos) = a.skip_command() else { return };
    let timer = Timer {
        entity,
        name: timer_name,
        script: a.ctx.script.clone(),
        pos,
        interval_ms: if millis { interval as f64 } else { interval as f64 * 1000.0 },
        start_ms: a.world.now_ms,
        count,
    };
    a.world.add_timer(timer);
}

// ---- built-in commands ------------------------------------------------------------------------

/// Commands that are part of the language itself rather than the game.
fn builtin(name: &str, a: &mut Args) -> Option<CmdResult> {
    use CmdResult::*;
    Some(match name {
        "nop" => Success,
        "goto" | "gosub" => {
            let sub = name == "gosub";
            let label = a.get_word();
            if !sub {
                a.skip_command();
            }
            if !a.jump_to_label(&label, sub) {
                a.warn(&format!("unknown label \"{label}\""));
                return Some(AbortError);
            }
            Jumped
        }
        "accept" => AbortAccept,
        "refuse" => AbortRefuse,
        "return" => {
            if !a.return_to_caller() {
                a.warn("return failed");
                return Some(AbortError);
            }
            Success
        }
        "random" => {
            let chance = a.get_float().clamp(0.0, 100.0);
            if chance < a.world.random() * 100.0 {
                a.skip_block();
            }
            Success
        }
        "setstatus" | "setmainevent" => {
            let ev = a.get_word();
            let id = a.entity();
            a.world.entity_mut(id).main_event = Some(ev);
            Success
        }
        "starttimer" | "stoptimer" => {
            let t = a.get_word();
            let slot = match t.as_str() {
                "timer1" => 0,
                "timer2" => 1,
                "timer3" => 2,
                "timer4" => 3,
                _ => {
                    a.warn(&format!("invalid timer: {t}"));
                    return Some(Failed);
                }
            };
            let now = a.world.now_ms;
            let id = a.entity();
            a.world.entity_mut(id).timer_slots[slot] = (name == "starttimer").then_some(now + 0.001);
            Success
        }
        "sendevent" => send_event_command(a),
        "setevent" => {
            let ev = a.get_word();
            let enable = a.get_bool();
            let id = a.entity();
            let disabled = &mut a.world.entity_mut(id).disabled_events;
            if enable {
                disabled.remove(&ev);
            } else {
                disabled.insert(ev);
            }
            Success
        }
        "if" => if_command(a),
        "else" => {
            a.skip_block();
            Success
        }
        "set" => {
            let var = a.get_word();
            let val = a.get_word();
            if var.is_empty() {
                a.warn("missing variable name");
                return Some(Failed);
            }
            let v = match var.chars().next().unwrap() {
                '$' | '\u{a3}' => Value::Text(a.string_var(&val)),
                '#' | '\u{a7}' => Value::Int(a.float_var(&val) as i64),
                '&' | '@' => Value::Float(a.float_var(&val)),
                _ => {
                    a.warn(&format!("unknown variable type: {var}"));
                    return Some(Failed);
                }
            };
            a.vars_for(&var).set(&var, v);
            Success
        }
        "inc" | "dec" | "mul" | "div" => {
            let var = a.get_word();
            let val = a.get_float();
            arithmetic(a, &var, |old| match name {
                "inc" => old + val,
                "dec" => old - val,
                "mul" => old * val,
                _ => if val == 0.0 { 0.0 } else { old / val },
            })
        }
        "++" | "--" => {
            let var = a.get_word();
            let d = if name == "++" { 1.0 } else { -1.0 };
            arithmetic(a, &var, |old| old + d)
        }
        "unset" => {
            let var = a.get_word();
            if var.is_empty() {
                a.warn("missing variable name");
                return Some(Failed);
            }
            a.vars_for(&var).unset(&var);
            Success
        }
        _ => return None,
    })
}

fn arithmetic(a: &mut Args, var: &str, f: impl Fn(f32) -> f32) -> CmdResult {
    let Some(c) = var.chars().next() else {
        a.warn("missing variable name");
        return CmdResult::Failed;
    };
    match c {
        '#' | '\u{a7}' => {
            let old = a.vars_for(var).get_int(var) as f32;
            a.vars_for(var).set(var, Value::Int(f(old) as i64));
        }
        '&' | '@' => {
            let old = a.vars_for(var).get_float(var);
            a.vars_for(var).set(var, Value::Float(f(old)));
        }
        _ => {
            a.warn(&format!("cannot calculate with variable {var}"));
            return CmdResult::Failed;
        }
    }
    CmdResult::Success
}

enum Operand {
    Text(String),
    Num(f32),
}

fn operand(a: &mut Args, var: &str, default_text: bool) -> Operand {
    match var.chars().next() {
        Some('^') => match a.sys_var(var) {
            Value::Text(t) => Operand::Text(t),
            v => Operand::Num(v.as_float()),
        },
        Some('#' | '\u{a7}' | '&' | '@') => Operand::Num(a.float_var(var)),
        Some('$' | '\u{a3}') => Operand::Text(a.vars_for(var).get_text(var).unwrap_or("").to_owned()),
        _ if default_text => Operand::Text(var.to_owned()),
        _ => Operand::Num(parse_float(var)),
    }
}

fn if_command(a: &mut Args) -> CmdResult {
    let left = a.get_word();
    let op = a.get_word();
    let right = a.get_word();
    let text_op = matches!(op.as_str(), "iselement" | "isclass" | "isgroup" | "!isgroup" | "istype" | "isin");
    if !text_op && !matches!(op.as_str(), "==" | "!=" | "<=" | "<" | ">=" | ">") {
        a.warn(&format!("unknown operator: {op}"));
        return CmdResult::Failed;
    }
    let l = operand(a, &left, text_op);
    let r = operand(a, &right, matches!(l, Operand::Text(_)));
    let cond = match (l, r) {
        (Operand::Text(x), Operand::Text(y)) => match op.as_str() {
            "==" => x == y,
            "!=" => x != y,
            "iselement" => y.split(' ').any(|w| w == x),
            "isclass" => x.contains(&y) || y.contains(&x),
            "isin" => y.contains(&x),
            "isgroup" | "!isgroup" => {
                let me = a.ctx.entity;
                let in_group = a.world.find(&x, me).is_some_and(|e| a.world.entity(e).groups.contains(&y));
                let exists = a.world.find(&x, me).is_some();
                if op == "isgroup" { in_group } else { exists && !in_group }
            }
            _ => false,
        },
        (Operand::Num(x), Operand::Num(y)) => match op.as_str() {
            "==" => x == y,
            "!=" => x != y,
            "<=" => x <= y,
            "<" => x < y,
            ">=" => x >= y,
            ">" => x > y,
            _ => true,
        },
        _ => {
            a.warn(&format!("incompatible types: \"{left}\" {op} \"{right}\""));
            a.skip_block();
            return CmdResult::Failed;
        }
    };
    if !cond {
        a.skip_block();
    }
    CmdResult::Success
}

fn send_event_command(a: &mut Args) -> CmdResult {
    let flags = a.get_flags();
    let (group, radius, zone) = (has_flag(&flags, 'g'), has_flag(&flags, 'r'), has_flag(&flags, 'z'));
    let group_name = if group {
        let w = a.get_word();
        a.string_var(&w)
    } else {
        String::new()
    };
    let mut event = a.get_word();
    let zone_name = if zone {
        let w = a.get_word();
        a.string_var(&w)
    } else {
        String::new()
    };
    let rad = if radius { a.get_float() } else { 0.0 };
    let mut target = String::new();
    if !group && !zone && !radius {
        let w = a.get_word();
        target = a.string_var(&w);
        // Some scripts give the target first and the event second.
        if a.world.find(&event, a.ctx.entity).is_some() && a.world.find(&target, a.ctx.entity).is_none() {
            std::mem::swap(&mut target, &mut event);
        }
    }
    let params_word = a.get_word();
    let params: Vec<String> = if params_word.is_empty() { Vec::new() } else { params_word.split(' ').map(str::to_owned).collect() };

    let me = a.ctx.entity;
    let _ = zone_name; // zones need level data the interpreter does not have
    let targets: Vec<EntityId> = if radius || group {
        let my_pos = a.world.entity(me).pos;
        a.world
            .entities
            .iter()
            .filter(|e| e.id != me)
            .filter(|e| !group || e.groups.contains(&group_name))
            .filter(|e| {
                !radius || {
                    let d = ((e.pos[0] - my_pos[0]).powi(2) + (e.pos[1] - my_pos[1]).powi(2) + (e.pos[2] - my_pos[2]).powi(2)).sqrt();
                    d <= rad
                }
            })
            .map(|e| e.id)
            .collect()
    } else if zone {
        Vec::new()
    } else {
        match a.world.find(&target, me) {
            Some(t) => vec![t],
            None => return CmdResult::Failed,
        }
    };
    for t in targets {
        a.world.queue_event(Some(me), t, &event, params.clone());
    }
    CmdResult::Success
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::EntityKind;

    /// Host that records the game commands it sees.
    #[derive(Default)]
    struct Rec(Vec<String>);
    impl Host for Rec {
        fn command(&mut self, name: &str, a: &mut Args) -> Option<CmdResult> {
            match name {
                "say" => {
                    let w = a.get_word();
                    // Commands resolve plain variable names themselves.
                    let w = a.string_var(&w);
                    self.0.push(w);
                    Some(CmdResult::Success)
                }
                _ => None,
            }
        }
    }

    /// Script text as the game stores it: one byte per character (Latin-1).
    fn script(src: &str) -> Option<Arc<Script>> {
        let bytes: Vec<u8> = src.chars().map(|c| c as u8).collect();
        Some(Arc::new(Script::new(&bytes)))
    }

    fn world_with(src: &str) -> (ScriptWorld, EntityId) {
        let mut w = ScriptWorld::new();
        let id = w.add_entity(EntityKind::Fix, "graph/obj3d/interactive/fix_inter/test/test", 1, script(src), None);
        (w, id)
    }

    fn run(src: &str, event: &str) -> (Vec<String>, ScriptWorld, EntityId) {
        let (mut w, id) = world_with(src);
        let mut h = Rec::default();
        w.send_event(&mut h, None, id, event, vec![]);
        w.update(&mut h, 0.0);
        (h.0, w, id)
    }

    #[test]
    fn variables_arithmetic_and_substitution() {
        let (out, w, id) = run(
            "on init {\n set #a 5\n inc #a 3\n mul #a 2\n set $t \"x~#a~y\"\n say $t\n set \u{a7}loc 7\n set &f 1.5\n inc &f 1\n say ~&f~\n accept\n}",
            "init",
        );
        assert_eq!(out, ["x16y", "2.5"]);
        assert_eq!(w.globals.get_int("#a"), 16);
        assert_eq!(w.entity(id).vars.get_int("\u{a7}loc"), 7);
    }

    #[test]
    fn if_else_blocks_and_comparisons() {
        let src = "on init {\n set #n 3\n if (#n > 2) {\n say big\n } else {\n say small\n }\n if #n == 5 {\n say five\n } else say not5\n if \"abc\" isin \"xabcx\" say in\n accept\n}";
        // `if (` with parens is not valid ASL; use the real syntax below.
        let _ = src;
        let (out, ..) = run(
            "on init {\n set #n 3\n if #n > 2 {\n say big\n } else {\n say small\n }\n if #n == 5 {\n say five\n } else say not5\n if abc isin xabcx say in\n if hello iselement \"say hello world\" say elem\n accept\n}",
            "init",
        );
        assert_eq!(out, ["big", "not5", "in", "elem"]);
    }

    #[test]
    fn parentheses_are_whitespace() {
        let (out, ..) = run(
            "on init {
 set #n 3
 if ( #n == 3 ) say yes
 if (#n == 4) say no
 if (#n > 1) {
 say block
 }
 accept
}",
            "init",
        );
        assert_eq!(out, ["yes", "block"]);
    }

    #[test]
    fn goto_gosub_and_return() {
        let (out, ..) = run(
            "on init {\n gosub sub\n say after\n goto end\n say skipped\n >>sub\n say insub\n return\n >>end\n say done\n accept\n}",
            "init",
        );
        assert_eq!(out, ["insub", "after", "done"]);
    }

    #[test]
    fn accept_refuse_and_event_results() {
        let (mut w, id) = world_with("on a {\n refuse\n}\non b {\n accept\n}\non c {\n say x\n}");
        let mut h = Rec::default();
        assert_eq!(w.send_event(&mut h, None, id, "a", vec![]), ScriptResult::Refuse);
        assert_eq!(w.send_event(&mut h, None, id, "b", vec![]), ScriptResult::Accept);
        // Falling off the end of a handler is an implicit accept (with a warning).
        assert_eq!(w.send_event(&mut h, None, id, "c", vec![]), ScriptResult::Accept);
        assert_eq!(w.send_event(&mut h, None, id, "missing", vec![]), ScriptResult::Accept);
    }

    #[test]
    fn sendevent_is_queued_and_carries_parameters() {
        let mut w = ScriptWorld::new();
        let s = |t: &str| Some(Arc::new(Script::new(t.as_bytes())));
        let a = w.add_entity(EntityKind::Fix, "x/lever", 1, s("on action {\n sendevent open door_0001 \"fast now\"\n accept\n}"), None);
        let b = w.add_entity(EntityKind::Fix, "x/door", 1, s("on open {\n say ~^$param1~-~^$param2~-~^sender~\n accept\n}"), None);
        let _ = b;
        let mut h = Rec::default();
        w.send_event(&mut h, None, a, "action", vec![]);
        assert!(h.0.is_empty(), "delivery is deferred until update()");
        w.update(&mut h, 16.0);
        assert_eq!(h.0, ["fast-now-lever_0001"]);
    }

    #[test]
    fn instance_script_runs_first_and_can_refuse() {
        let mut w = ScriptWorld::new();
        let s = |t: &str| Some(Arc::new(Script::new(t.as_bytes())));
        let id = w.add_entity(EntityKind::Fix, "x/thing", 1, s("on go {\n say class\n accept\n}"), s("on go {\n say instance\n refuse\n}"));
        let mut h = Rec::default();
        assert_eq!(w.send_event(&mut h, None, id, "go", vec![]), ScriptResult::Refuse);
        assert_eq!(h.0, ["instance"]);
    }

    #[test]
    fn timers_fire_repeat_and_stop() {
        let (mut w, id) = world_with("on init {\n timerbeep 3 0.5 say tick\n accept\n}");
        let mut h = Rec::default();
        w.send_event(&mut h, None, id, "init", vec![]);
        for _ in 0..30 {
            w.update(&mut h, 100.0);
        }
        assert_eq!(h.0, ["tick", "tick", "tick"]);
        assert_eq!(w.timer_count(), 0);
    }

    #[test]
    fn setevent_disables_events() {
        let (mut w, id) = world_with("on init {\n setevent chat off\n accept\n}\non chat {\n say hi\n accept\n}");
        let mut h = Rec::default();
        w.send_event(&mut h, None, id, "init", vec![]);
        assert_eq!(w.send_event(&mut h, None, id, "chat", vec![]), ScriptResult::Refuse);
        assert!(h.0.is_empty());
    }

    #[test]
    fn unknown_commands_are_skipped_and_counted() {
        let (out, w, _) = run("on init {\n frobnicate 1 2 3\n say ok\n accept\n}", "init");
        assert_eq!(out, ["ok"]);
        assert_eq!(w.stats.unknown_commands.get("frobnicate"), Some(&1));
    }

    #[test]
    fn runaway_loops_are_aborted() {
        let (mut w, id) = world_with("on init {\n >>top\n goto top\n}");
        let mut h = Rec::default();
        assert_eq!(w.send_event(&mut h, None, id, "init", vec![]), ScriptResult::Error);
        assert_eq!(w.stats.aborted_runaway, 1);
    }
}
