//! Script text and event lookup.

/// A loaded script: raw bytes, ASCII-lowercased (the engine lowercases whole scripts on load).
/// Scripts use a legacy 8-bit encoding; bytes are never interpreted as UTF-8.
#[derive(Debug, Clone)]
pub struct Script {
    pub data: Vec<u8>,
}

impl Script {
    pub fn new(raw: &[u8]) -> Self {
        Script { data: raw.to_ascii_lowercase() }
    }

    /// Position just after the first uncommented occurrence of `pattern` (e.g. `"on init"`) that is
    /// followed by whitespace, or `None`.
    pub fn find_pos(&self, pattern: &str) -> Option<usize> {
        let d = &self.data;
        let pat = pattern.as_bytes();
        if pat.is_empty() || d.len() <= pat.len() {
            return None;
        }
        let mut pos = 0;
        while pos < d.len() {
            let found = d[pos..].windows(pat.len()).position(|w| w == pat)? + pos;
            if found + pat.len() >= d.len() {
                return None;
            }
            if d[found + pat.len()] > 32 {
                pos = found + 1;
                continue;
            }
            // Skip occurrences inside a `//` comment on the same line.
            let mut p = found;
            let commented = loop {
                if d[p] == b'/' && d.get(p + 1) == Some(&b'/') {
                    break true;
                }
                if d[p] == b'\n' || p == 0 {
                    break false;
                }
                p -= 1;
            };
            if !commented {
                return Some(found + pat.len());
            }
            pos = found + 1;
        }
        None
    }
}

/// Parentheses separate tokens exactly like spaces do (`if ( #a == 1 )` is `if #a == 1`).
pub(crate) fn is_whitespace(c: u8) -> bool {
    c <= 32 || c == b'(' || c == b')'
}

/// Latin-1 decoding of script bytes.
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_events_and_ignores_comments() {
        let s = Script::new(b"// on init {\nON INIT {\n accept\n}\non initend {\n}\n");
        let p = s.find_pos("on init").unwrap();
        assert_eq!(&s.data[p..p + 3], b" {\n");
        assert!(s.data[..p].ends_with(b"\non init"));
        // "on init" must be followed by whitespace: "on initend" is a different event.
        let s = Script::new(b"on initend {\n}\n");
        assert_eq!(s.find_pos("on init"), None);
        assert!(s.find_pos("on initend").is_some());
    }
}
