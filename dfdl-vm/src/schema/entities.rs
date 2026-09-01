use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use regex_automata::{meta::Regex, Anchored, Input};

/// Expand DFDL entity references in property values.
///
/// Supports `%NL;`, `%CR;`, `%LF;`, `%SP;`, `%HT;`, `%WSP;`, `%WS;`, `%#rNN;` (hex byte).
pub fn expand_entities(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some((entity, consumed)) = parse_entity(&input[i..]) {
                out.extend_from_slice(&entity);
                i += consumed;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Expand entities in a string, preserving UTF-8 where possible.
pub fn expand_entities_str(input: &str) -> String {
    let bytes = expand_entities(input);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_entity(input: &str) -> Option<(Vec<u8>, usize)> {
    if !input.starts_with('%') {
        return None;
    }
    let rest = &input[1..];
    if let Some(end) = rest.find(';') {
        let name = &rest[..end];
        let consumed = 1 + end + 1;
        let value = match name {
            "NL" => vec![b'\n'],
            "CR" => vec![b'\r'],
            "LF" => vec![b'\n'],
            "SP" => vec![b' '],
            "HT" => vec![b'\t'],
            "WSP" | "WS" => vec![b' '], // canonical whitespace for entity expansion
            other if other.starts_with("#r") => {
                let hex = &other[2..];
                u8::from_str_radix(hex, 16).ok().map(|b| vec![b])?
            }
            _ => return None,
        };
        return Some((value, consumed));
    }
    None
}

/// Match input against a DFDL delimiter/initiator/terminator pattern.
/// Supports literal bytes (after entity expansion) plus simple regex suffixes: `+`, `*`, `?`.
/// Compound patterns like `%NL;%WSP*;` are matched segment-by-segment.
pub fn match_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Some(0);
    }

    // Regex-style character class: [abc]+ or [a-zA-Z]+
    if pat.starts_with('[') {
        return match_char_class(input, pat);
    }

    // DFDL whitespace entity with optional quantifier
    if pat.starts_with("%WSP") || pat.starts_with("%WS") {
        return match_wsp_entity(input, pat);
    }

    let expanded = expand_entities(pat);
    if expanded.is_empty() {
        return Some(0);
    }

    let last = pat.as_bytes().last().copied();
    let quantifier = match last {
        Some(b'+') | Some(b'*') | Some(b'?') => last,
        _ => None,
    };

    let base = if quantifier.is_some() {
        &expanded[..expanded.len().saturating_sub(1)]
    } else {
        &expanded[..]
    };

    match quantifier {
        None => {
            if input.starts_with(base) {
                Some(base.len())
            } else {
                None
            }
        }
        Some(b'+') => {
            let mut pos = 0;
            while input.len() >= pos + base.len() && input[pos..].starts_with(base) {
                pos += base.len();
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some(b'*') => {
            let mut pos = 0;
            while input.len() >= pos + base.len() && input[pos..].starts_with(base) {
                pos += base.len();
            }
            Some(pos)
        }
        Some(b'?') => {
            if input.starts_with(base) {
                Some(base.len())
            } else {
                Some(0)
            }
        }
        _ => None,
    }
}

/// Match a compound DFDL delimiter (e.g. `%NL;%WSP*;`).
pub fn match_delimiter(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Some(0);
    }
    let mut pos = 0;
    for segment in split_delimiter_segments(pat) {
        let matched = match_pattern(&input[pos..], segment)?;
        pos += matched;
    }
    Some(pos)
}

/// Minimal bytes to emit for a delimiter on encode (one WSP for `+`, none for `*`/`?`).
pub fn encode_delimiter(pattern: &str) -> Vec<u8> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for segment in split_delimiter_segments(pat) {
        out.extend(minimal_encode_segment(segment));
    }
    out
}

fn split_delimiter_segments(pattern: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(rel) = pattern[i..].find(';') {
                let mut end = i + rel + 1;
                if end < bytes.len() && matches!(bytes[end], b'+' | b'*' | b'?') {
                    end += 1;
                }
                segments.push(&pattern[i..end]);
                i = end;
                continue;
            }
        } else if bytes[i] == b'[' {
            if let Some(rel) = pattern[i..].find(']') {
                let mut end = i + rel + 1;
                if end < bytes.len() && matches!(bytes[end], b'+' | b'*' | b'?') {
                    end += 1;
                }
                segments.push(&pattern[i..end]);
                i = end;
                continue;
            }
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i] != b'%' && bytes[i] != b'[' {
            i += 1;
        }
        segments.push(&pattern[start..i]);
    }
    segments
}

fn minimal_encode_segment(segment: &str) -> Vec<u8> {
    let seg = segment.trim();
    if seg.is_empty() {
        return Vec::new();
    }
    if seg.starts_with("%WSP") || seg.starts_with("%WS") {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b' '],
        };
    }
    if seg.starts_with('[') {
        let q = seg.as_bytes().last().copied();
        return match q {
            Some(b'*') | Some(b'?') => Vec::new(),
            _ => vec![b'0'],
        };
    }
    let q = seg.as_bytes().last().copied();
    let base = match q {
        Some(b'+') | Some(b'*') | Some(b'?') => &seg[..seg.len() - 1],
        _ => seg,
    };
    let expanded = expand_entities(base);
    match q {
        Some(b'*') | Some(b'?') => Vec::new(),
        Some(b'+') => expanded.into_iter().take(1).collect(),
        _ => expanded,
    }
}

fn is_wsp(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn match_wsp_entity(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if !pat.starts_with('%') {
        return None;
    }
    let body = pat.strip_prefix('%')?;
    let semi = body.find(';')?;
    let entity = &body[..semi];
    if !entity.starts_with("WSP") && entity != "WS" && !entity.starts_with("WS") {
        return None;
    }
    let q = entity.chars().last();
    let quantifier = match q {
        Some('+') | Some('*') | Some('?') => q,
        _ => None,
    };

    let mut pos = 0;
    match quantifier {
        Some('+') => {
            while pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            if pos > 0 { Some(pos) } else { None }
        }
        Some('*') => {
            while pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            Some(pos)
        }
        Some('?') => {
            if pos < input.len() && is_wsp(input[pos]) {
                pos += 1;
            }
            Some(pos)
        }
        _ => {
            if pos < input.len() && is_wsp(input[pos]) {
                Some(1)
            } else {
                None
            }
        }
    }
}

/// Match value bytes against a DFDL length pattern (full ECMAScript-style regex).
///
/// Uses [`regex-automata`](https://docs.rs/regex-automata) (`no_std` + `alloc`) for
/// alternation, negated classes, Unicode property classes (`\p{L}`), counted
/// closures, and other constructs beyond simple `[char-class]+` patterns.
pub fn match_length_pattern(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Some(0);
    }

    // Fast path for simple char-class patterns without regex metacharacters.
    if pat.starts_with('[')
        && !pat.contains('\\')
        && !pat.contains('(')
        && !pat.contains('|')
    {
        if let Some(len) = match_char_class(input, pat) {
            return Some(len);
        }
    }

    let re = Regex::new(pat).ok()?;
    let hay = Input::new(input).anchored(Anchored::Yes);
    let m = re.find(hay)?;
    if m.start() == 0 {
        Some(m.end())
    } else {
        None
    }
}

fn match_char_class(input: &[u8], pattern: &str) -> Option<usize> {
    let pat = pattern.trim();
    if !pat.starts_with('[') {
        return None;
    }
    let close = pat.find(']')?;
    let class_body = &pat[1..close];
    let suffix = &pat[close + 1..];

    let mut pos = 0;
    while pos < input.len() {
        let ch = input[pos];
        if !char_in_class(ch, class_body) {
            break;
        }
        pos += 1;
    }

    match suffix {
        "+" if pos > 0 => Some(pos),
        "*" => Some(pos),
        "?" => Some(pos.min(1)),
        "" if pos > 0 => Some(pos),
        _ if pos > 0 && suffix.is_empty() => Some(pos),
        _ => None,
    }
}

fn char_in_class(ch: u8, class_body: &str) -> bool {
    let ch_arr = [ch];
    let s = core::str::from_utf8(&ch_arr).unwrap_or("");
    let c = s.chars().next().unwrap_or('\0');
    let mut i = 0;
    let chars: Vec<char> = class_body.chars().collect();
    let mut negate = false;
    if chars.first() == Some(&'^') {
        negate = true;
        i = 1;
    }
    let mut matched = false;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let lo = chars[i];
            let hi = chars[i + 2];
            if c >= lo && c <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if c == chars[i] {
                matched = true;
            }
            i += 1;
        }
    }
    matched ^ negate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_nl() {
        assert_eq!(expand_entities("%NL;"), b"\n");
        assert_eq!(expand_entities("%#r3b;"), b";");
    }

    #[test]
    fn match_wsp_plus() {
        let pat = "%WSP+;";
        assert_eq!(match_pattern(b"   x", pat), Some(3));
    }

    #[test]
    fn match_alpha_pattern() {
        assert_eq!(match_length_pattern(b"aSingleToken123", "[a-zA-Z]+"), Some(12));
        assert_eq!(match_length_pattern(b"123456789", "[0-9]+"), Some(9));
    }

    #[test]
    fn match_alternation_pattern() {
        assert_eq!(match_length_pattern(b"batcz", "(b|c|h)at"), Some(3));
        assert_eq!(match_length_pattern(b"catx", "(b|c|h)at"), Some(3));
        assert_eq!(match_length_pattern(b"dat", "(b|c|h)at"), None);
    }

    #[test]
    fn match_negated_class_pattern() {
        assert_eq!(match_length_pattern(b"cz", "[^ab]z"), Some(2));
        assert_eq!(match_length_pattern(b"az", "[^ab]z"), None);
    }

    #[test]
    fn match_unicode_property_pattern() {
        assert_eq!(match_length_pattern(b"abcDEFG", r"\p{L}{2,5}"), Some(5));
        assert_eq!(match_length_pattern(b"a1", r"\p{L}{2,5}"), None);
    }

    #[test]
    fn match_literal_with_space_pattern() {
        assert_eq!(match_length_pattern(b"3 4x", "3 4"), Some(3));
    }
}
