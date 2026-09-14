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
    if input.starts_with(start) && input.ends_with(end) && input.len() >= start.len() + end.len() {
        Ok(input[start.len()..input.len() - end.len()].to_string())
    } else {
        Ok(input.to_string())
    }
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
            escape_block_start: None,
            escape_block_end: None,
        };
        assert_eq!(unescape_character("ab#,cd", &scheme), "ab,cd");
    }

    #[test]
    fn slash_escape_padding_case() {
        let scheme = EscapeSchemeDef {
            escape_kind: EscapeKind::EscapeCharacter,
            escape_character: Some("/".into()),
            escape_escape_character: Some("[".into()),
            escape_block_start: None,
            escape_block_end: None,
        };
        assert_eq!(unescape_character("word/", &scheme), "word");
    }
}
