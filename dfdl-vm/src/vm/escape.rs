use crate::error::VmError;
use crate::schema::{EscapeKind, EscapeSchemeDef};

/// Apply DFDL escape scheme after padding trim (Section 7 / DFDL-7-089R).
pub fn unescape_field_text(input: &str, scheme: &EscapeSchemeDef) -> Result<String, VmError> {
    match scheme.escape_kind {
        EscapeKind::EscapeBlock => unescape_block(input, scheme),
        EscapeKind::EscapeCharacter => Ok(unescape_character(input, scheme)),
    }
}

fn unescape_block(input: &str, scheme: &EscapeSchemeDef) -> Result<String, VmError> {
    let start = scheme.escape_block_start.as_deref().unwrap_or("");
    let end = scheme.escape_block_end.as_deref().unwrap_or("");
    if start.is_empty() || end.is_empty() {
        return Ok(input.to_string());
    }
    let inner = if input.starts_with(start) && input.ends_with(end) && input.len() >= start.len() + end.len()
    {
        &input[start.len()..input.len() - end.len()]
    } else {
        input
    };
    if scheme
        .escape_escape_character
        .as_deref()
        .is_some_and(|s| !s.is_empty())
    {
        let esc = scheme.escape_escape_character.clone();
        let inner_scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: esc,
            escape_escape_character: Some(String::new()),
            ..Default::default()
        };
        return Ok(unescape_character(inner, &inner_scheme));
    }
    Ok(inner.to_string())
}

/// Next byte index when scanning delimited data with an escape scheme (Section 7).
pub(crate) fn advance_escape_scan_index(data: &[u8], i: usize, scheme: &EscapeSchemeDef) -> usize {
    if i >= data.len() {
        return i;
    }
    match scheme.escape_kind {
        EscapeKind::EscapeBlock => {
            let start = scheme.escape_block_start.as_deref().unwrap_or("");
            let end = scheme.escape_block_end.as_deref().unwrap_or("");
            if !start.is_empty()
                && data.len() >= i + start.len()
                && &data[i..i + start.len()] == start.as_bytes()
            {
                if let Some(rel) = find_subslice(data, i + start.len(), end.as_bytes()) {
                    return rel + end.len();
                }
            }
            i + 1
        }
        EscapeKind::EscapeCharacter => {
            let Some(esc) = scheme.escape_character.as_deref().filter(|s| !s.is_empty()) else {
                return i + 1;
            };
            let esc_bytes = esc.as_bytes();
            let esc_esc_bytes = scheme
                .escape_escape_character
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| s.as_bytes());
            if let Some(ee) = esc_esc_bytes {
                if i + ee.len() + esc_bytes.len() <= data.len()
                    && &data[i..i + ee.len()] == ee
                    && &data[i + ee.len()..i + ee.len() + esc_bytes.len()] == esc_bytes
                {
                    return i + ee.len() + esc_bytes.len();
                }
            }
            if i + esc_bytes.len() < data.len() && &data[i..i + esc_bytes.len()] == esc_bytes {
                return i + esc_bytes.len() + 1;
            }
            if i + esc_bytes.len() <= data.len() && &data[i..i + esc_bytes.len()] == esc_bytes {
                return i + esc_bytes.len();
            }
            i + 1
        }
    }
}

fn find_subslice(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

fn unescape_character(input: &str, scheme: &EscapeSchemeDef) -> String {
    let Some(esc) = scheme.escape_character.as_deref() else {
        return input.to_string();
    };
    if esc.is_empty() {
        return input.to_string();
    }
    let esc_esc = scheme
        .escape_escape_character
        .as_deref()
        .filter(|s| !s.is_empty());
    let esc_bytes = esc.as_bytes();
    let esc_esc_bytes = esc_esc.map(|s| s.as_bytes());
    let bytes = input.as_bytes();
    let mut out = alloc::vec::Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(ee) = esc_esc_bytes {
            if i + ee.len() <= bytes.len() && &bytes[i..i + ee.len()] == ee {
                if i + ee.len() + esc_bytes.len() <= bytes.len()
                    && &bytes[i + ee.len()..i + ee.len() + esc_bytes.len()] == esc_bytes
                {
                    out.extend_from_slice(esc_bytes);
                    i += ee.len() + esc_bytes.len();
                    continue;
                }
            }
        }
        if i + esc_bytes.len() <= bytes.len() && &bytes[i..i + esc_bytes.len()] == esc_bytes {
            i += esc_bytes.len();
            if i < bytes.len() {
                out.push(bytes[i]);
                i += 1;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escape logical field text for delimited unparsing (inverse of [`unescape_character`]).
pub fn escape_field_text(
    input: &str,
    scheme: &EscapeSchemeDef,
    markup: &[&str],
) -> alloc::string::String {
    match scheme.escape_kind {
        EscapeKind::EscapeBlock => escape_block_field(input, scheme, markup),
        EscapeKind::EscapeCharacter => escape_character_field(input, scheme, markup),
    }
}

fn escape_block_field(
    input: &str,
    scheme: &EscapeSchemeDef,
    markup: &[&str],
) -> alloc::string::String {
    let start = scheme.escape_block_start.as_deref().unwrap_or("");
    let end = scheme.escape_block_end.as_deref().unwrap_or("");
    if start.is_empty() || end.is_empty() {
        return input.to_string();
    }
    let inner = escape_block_interior(input, scheme, markup);
    if inner.starts_with(start) && inner.ends_with(end) {
        return inner;
    }
    alloc::format!("{start}{inner}{end}")
}

fn escape_block_interior(input: &str, scheme: &EscapeSchemeDef, markup: &[&str]) -> alloc::string::String {
    let start = scheme.escape_block_start.as_deref().unwrap_or("");
    let end = scheme.escape_block_end.as_deref().unwrap_or("");
    let body = if !start.is_empty()
        && !end.is_empty()
        && input.starts_with(start)
        && input.ends_with(end)
        && input.len() >= start.len() + end.len()
    {
        &input[start.len()..input.len() - end.len()]
    } else {
        input
    };
    let ee = scheme
        .escape_escape_character
        .as_deref()
        .filter(|s| !s.is_empty());
    let ee_bytes = ee.map(|s| s.as_bytes());
    let start_b = start.as_bytes();
    let end_b = end.as_bytes();
    let bytes = body.as_bytes();
    let mut out = alloc::vec::Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched = false;
        if !start.is_empty() && i + start_b.len() <= bytes.len() && &bytes[i..i + start_b.len()] == start_b {
            if let Some(e) = ee_bytes {
                out.extend_from_slice(e);
            }
            out.extend_from_slice(start_b);
            i += start_b.len();
            matched = true;
        } else if !end.is_empty() && i + end_b.len() <= bytes.len() && &bytes[i..i + end_b.len()] == end_b
        {
            if let Some(e) = ee_bytes {
                out.extend_from_slice(e);
            }
            out.extend_from_slice(end_b);
            i += end_b.len();
            matched = true;
        } else {
            for m in markup {
                let mb = m.as_bytes();
                if !m.is_empty() && i + mb.len() <= bytes.len() && &bytes[i..i + mb.len()] == mb {
                    if let Some(e) = ee_bytes {
                        out.extend_from_slice(e);
                    }
                    out.extend_from_slice(mb);
                    i += mb.len();
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        if let Some(e) = ee_bytes {
            if i + e.len() <= bytes.len() && &bytes[i..i + e.len()] == e {
                out.extend_from_slice(e);
                out.extend_from_slice(e);
                i += e.len();
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape_character_field(
    input: &str,
    scheme: &EscapeSchemeDef,
    markup: &[&str],
) -> alloc::string::String {
    let Some(esc) = scheme.escape_character.as_deref() else {
        return input.to_string();
    };
    if esc.is_empty() {
        return input.to_string();
    }
    let esc_esc = scheme
        .escape_escape_character
        .as_deref()
        .filter(|s| !s.is_empty());
    let esc_bytes = esc.as_bytes();
    let esc_esc_bytes = esc_esc.map(|s| s.as_bytes());
    let bytes = input.as_bytes();
    let mut out = alloc::vec::Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched_markup = false;
        for term in markup {
            let tb = term.as_bytes();
            if !term.is_empty() && i + tb.len() <= bytes.len() && &bytes[i..i + tb.len()] == tb {
                out.extend_from_slice(esc_bytes);
                out.extend_from_slice(tb);
                i += tb.len();
                matched_markup = true;
                break;
            }
        }
        if matched_markup {
            continue;
        }
        if i + esc_bytes.len() <= bytes.len() && &bytes[i..i + esc_bytes.len()] == esc_bytes {
            if let Some(ee) = esc_esc_bytes {
                out.extend_from_slice(ee);
                out.extend_from_slice(esc_bytes);
            } else {
                out.extend_from_slice(esc_bytes);
            }
            i += esc_bytes.len();
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::EscapeKind;

    #[test]
    fn pound_escape_comma() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: Some("#".into()),
            escape_escape_character: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(unescape_character("ab#,cd", &scheme), "ab,cd");
    }

    #[test]
    fn slash_escape_padding_case() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: Some("/".into()),
            escape_escape_character: Some("[".into()),
            ..Default::default()
        };
        assert_eq!(unescape_character("word/", &scheme), "word");
    }

    #[test]
    fn escape_semicolon_with_runtime_esc() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: Some("x".into()),
            escape_escape_character: Some("^".into()),
            ..Default::default()
        };
        assert_eq!(
            escape_field_text("test;ing", &scheme, &[";"]),
            "testx;ing"
        );
    }

    #[test]
    fn pound_escape_comma_in_field() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: Some("#".into()),
            escape_escape_character: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(escape_field_text("one, two", &scheme, &[","]), "one#, two");
    }

    #[test]
    fn block_interior_escape_escape() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeBlock,
            escape_block_start: Some("/*".into()),
            escape_block_end: Some("*/".into()),
            escape_escape_character: Some("#".into()),
            ..Default::default()
        };
        assert_eq!(
            unescape_field_text("/*, three and four#*/", &scheme).unwrap(),
            ", three and four*/"
        );
    }

    #[test]
    fn escape_escape_char_itself() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: Some("^".into()),
            escape_escape_character: Some("z".into()),
            ..Default::default()
        };
        assert_eq!(escape_field_text("test^", &scheme, &[";"]), "testz^");
    }
}
