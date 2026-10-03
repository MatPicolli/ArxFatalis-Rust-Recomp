//! Script variables. The first character of a name encodes its type and scope:
//! `#` global int, `&` global float, `$` global text, and `§` / `@` / `£` (Latin-1 0xA7 / `@` /
//! 0xA3) for the per-entity equivalents.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f32),
    Text(String),
}

impl Value {
    pub fn as_float(&self) -> f32 {
        match self {
            Value::Int(i) => *i as f32,
            Value::Float(f) => *f,
            Value::Text(t) => parse_float(t),
        }
    }

    pub fn to_text(&self) -> String {
        match self {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Text(t) => t.clone(),
        }
    }
}

/// Shortest representation that round-trips, like C++ iostream output for typical script values.
pub fn format_float(f: f32) -> String {
    format!("{f}")
}

/// Parse a leading number, ignoring trailing garbage; 0 if there is none.
pub fn parse_float(s: &str) -> f32 {
    let s = s.trim();
    let end = s
        .char_indices()
        .take_while(|&(i, c)| c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    s[..end].parse().unwrap_or(0.0)
}

/// Is `name` a per-entity variable (as opposed to a global one)?
pub fn is_local(name: &str) -> bool {
    matches!(name.chars().next(), Some('\u{a7}' | '@' | '\u{a3}'))
}

#[derive(Debug, Clone, Default)]
pub struct Vars(HashMap<String, Value>);

impl Vars {
    pub fn get_int(&self, name: &str) -> i64 {
        match self.0.get(name) {
            Some(Value::Int(i)) => *i,
            Some(Value::Float(f)) => *f as i64,
            _ => 0,
        }
    }
    pub fn get_float(&self, name: &str) -> f32 {
        self.0.get(name).map_or(0.0, Value::as_float)
    }
    pub fn get_text(&self, name: &str) -> Option<&str> {
        match self.0.get(name) {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        }
    }
    pub fn set(&mut self, name: &str, v: Value) {
        self.0.insert(name.to_owned(), v);
    }
    pub fn unset(&mut self, name: &str) {
        self.0.remove(name);
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbers_leniently() {
        assert_eq!(parse_float("12"), 12.0);
        assert_eq!(parse_float("-3.5abc"), -3.5);
        assert_eq!(parse_float("abc"), 0.0);
        assert_eq!(parse_float(""), 0.0);
    }

    #[test]
    fn local_prefixes() {
        assert!(is_local("\u{a7}x") && is_local("@x") && is_local("\u{a3}x"));
        assert!(!is_local("#x") && !is_local("&x") && !is_local("$x"));
    }
}
