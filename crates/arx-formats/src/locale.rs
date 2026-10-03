//! Localised text (`localisation/utext_<language>.ini`).
//!
//! The file is UTF-16 (with a byte order mark) and made of sections:
//!
//! ```text
//! [goblinlord_forbidden]//~16
//! String="Hey! Where you think you going?"
//! String2="Dis area forbidden!!!! You not go!!"
//! ```
//!
//! `String` is the first variant of a line, `String2`, `String3`, ... further variants. Spoken lines have one
//! sound per variant: `speech/<language>/<key>.wav`, `<key>2.wav`, ... Keys are case-insensitive.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Locale {
    entries: HashMap<String, Vec<String>>,
}

/// Strip the optional qualifying brackets scripts put around keys: `[description_door]` -> `description_door`.
pub fn key_of(text: &str) -> &str {
    let t = text.trim();
    t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(t)
}

fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], word: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| word([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

impl Locale {
    pub fn parse(file: &[u8]) -> Self {
        let text = decode(file);
        let mut entries: HashMap<String, Vec<String>> = HashMap::new();
        let mut current: Option<String> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(rest) = line.strip_prefix('[') {
                // `[key]` optionally followed by a `//` comment.
                current = rest.split_once(']').map(|(k, _)| k.trim().to_lowercase());
                if let Some(k) = &current {
                    entries.entry(k.clone()).or_default();
                }
                continue;
            }
            let Some(key) = &current else { continue };
            let Some((name, value)) = line.split_once('=') else { continue };
            let name = name.trim().to_ascii_lowercase();
            let Some(digits) = name.strip_prefix("string") else { continue };
            let variant: usize = if digits.is_empty() { 1 } else { digits.parse().unwrap_or(0) };
            if variant == 0 {
                continue;
            }
            let value = value.trim();
            // The value is quoted; anything after the closing quote is a comment.
            let inner = value
                .strip_prefix('"')
                .map(|v| v.rfind('"').map_or(v, |end| &v[..end]))
                .unwrap_or(value);
            let list = entries.get_mut(key).expect("section created above");
            if list.len() < variant {
                list.resize(variant, String::new());
            }
            list[variant - 1] = inner.to_owned();
        }
        Locale { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All variants of a key (`String`, `String2`, ...), empty if the key is unknown.
    pub fn variants(&self, key: &str) -> &[String] {
        self.entries.get(&key_of(key).to_lowercase()).map_or(&[], Vec::as_slice)
    }

    /// The first variant of a key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.variants(key).first().map(String::as_str).filter(|s| !s.is_empty())
    }

    /// Text for a key, or the key itself (without brackets) when it has no entry; scripts often pass
    /// literal text where a key is expected.
    pub fn text_or_key<'a>(&'a self, key: &'a str) -> &'a str {
        self.get(key).unwrap_or_else(|| key_of(key))
    }

    /// Number of variants of a key (0 if unknown).
    pub fn count(&self, key: &str) -> usize {
        self.variants(key).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut v = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_sections_variants_and_comments() {
        let file = utf16(
            "//header\r\n\r\n[description_door]\r\nstring=\"A door.\"\r\n\r\n[Goblinlord_Forbidden]//~16\r\nString=\"Hey!\"\r\nString2=\"Dis area forbidden!\"\r\n[empty]\r\n",
        );
        let l = Locale::parse(&file);
        assert_eq!(l.len(), 3);
        assert_eq!(l.get("description_door"), Some("A door."));
        assert_eq!(l.get("[description_door]"), Some("A door."), "brackets and case are ignored");
        assert_eq!(l.variants("goblinlord_forbidden"), ["Hey!", "Dis area forbidden!"]);
        assert_eq!(l.count("goblinlord_forbidden"), 2);
        assert_eq!(l.get("empty"), None);
        assert_eq!(l.text_or_key("[not_a_key]"), "not_a_key");
        assert_eq!(l.text_or_key("description_door"), "A door.");
    }

    #[test]
    fn accepts_utf8_and_accented_text() {
        let l = Locale::parse("[a]\nstring=\"é à ç\"\n".as_bytes());
        assert_eq!(l.get("a"), Some("é à ç"));
        let l = Locale::parse(&utf16("[a]\nstring=\"é à ç\"\n"));
        assert_eq!(l.get("a"), Some("é à ç"));
    }
}
