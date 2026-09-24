use super::cursor::Cursor;
use super::encoding_name;
use crate::error::VmError;
use crate::ir::{IrProps, StringPool};
use crate::schema::{LengthKind, LengthUnits, NilKind};
use alloc::string::String;
use alloc::vec::Vec;

pub(crate) fn nil_value_alternatives<'a>(
    props: &'a IrProps,
    strings: &'a StringPool,
) -> Result<Option<Vec<String>>, VmError> {
    if !props.nillable {
        return Ok(None);
    }
    if !matches!(
        props.nil_kind,
        Some(NilKind::LiteralValue) | Some(NilKind::LiteralCharacter)
    ) {
        return Ok(None);
    }
    let Some(id) = props.nil_value else {
        return Ok(None);
    };
    let raw = strings.get(id)?;
    Ok(Some(crate::schema::nil_value_alternatives(raw)))
}

#[allow(dead_code)]
pub(crate) fn nil_first_alternative(
    props: &IrProps,
    strings: &StringPool,
) -> Result<Option<String>, VmError> {
    Ok(nil_value_alternatives(props, strings)?.and_then(|alts| alts.into_iter().next()))
}

pub(crate) fn nil_value_includes_empty(
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(false);
    };
    Ok(alts
        .iter()
        .any(|a| crate::schema::expand_entities(a.trim()).is_empty()))
}

pub(crate) fn nil_character_repeat_count(props: &IrProps) -> Result<usize, VmError> {
    match props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit/fixed nil field missing length".into(),
            })? as usize;
            Ok(match props.length_units {
                LengthUnits::Bytes | LengthUnits::Characters => len,
                LengthUnits::Bits => len.div_ceil(8),
            })
        }
        _ => Ok(1),
    }
}

pub(crate) fn expand_nil_alternative(
    alt: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Vec<u8>, VmError> {
    let trimmed = alt.trim();
    if trimmed == "%NL;" {
        if let Some(id) = props.output_new_line {
            if let Ok(onl) = strings.get(id) {
                return Ok(crate::schema::expand_entities(onl));
            }
        }
    }
    Ok(crate::schema::expand_entities(trimmed))
}

pub(crate) fn nil_unparse_bytes_for_encode(
    props: &IrProps,
    strings: &StringPool,
) -> Result<Vec<u8>, VmError> {
    nil_unparse_bytes(props, strings)
}

pub(crate) fn nil_unparse_bytes(props: &IrProps, strings: &StringPool) -> Result<Vec<u8>, VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(Vec::new());
    };
    let first = alts.first().map(|s| s.as_str()).unwrap_or("");
    if props.nil_kind == Some(NilKind::LiteralCharacter) {
        let unit = expand_nil_alternative(first, props, strings)?;
        if unit.is_empty() {
            return Ok(Vec::new());
        }
        let repeat = nil_character_repeat_count(props)?;
        let mut out = Vec::with_capacity(unit.len().saturating_mul(repeat));
        for _ in 0..repeat {
            out.extend_from_slice(&unit);
        }
        return Ok(out);
    }
    expand_nil_alternative(first, props, strings)
}

pub(crate) fn text_matches_nil_literal(
    text: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(false);
    };
    if props.nil_kind == Some(NilKind::LiteralCharacter) {
        if alts.iter().any(|a| a.is_empty()) && text.is_empty() {
            return Ok(true);
        }
        if let Some(nil) = alts.first() {
            if nil.is_empty() {
                return Ok(text.is_empty());
            }
        }
        if !alts.is_empty() {
            for alt in alts {
                if alt.is_empty() {
                    if text.is_empty() {
                        return Ok(true);
                    }
                    continue;
                }
                let expanded = crate::schema::expand_entities_str(alt.trim());
                if expanded.is_empty() {
                    if text.is_empty() {
                        return Ok(true);
                    }
                    continue;
                }
                let nil_char = expanded.chars().next().unwrap_or('\0');
                if expanded.chars().count() == 1
                    && !text.is_empty()
                    && text.chars().all(|c| {
                        if props.ignore_case {
                            c.eq_ignore_ascii_case(&nil_char)
                        } else {
                            c == nil_char
                        }
                    })
                {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
    }
    Ok(alts.iter().any(|alt| text == alt.as_str()))
}

pub(crate) fn validate_nil_value_runtime(
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), VmError> {
    if !props.nillable {
        return Ok(());
    }
    let Some(nil_id) = props.nil_value else {
        return Ok(());
    };
    let raw = strings.get(nil_id)?;
    if raw.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Property dfdl:nilValue cannot be empty string. Use dfdl:nilValue='%ES;' for empty string.".into(),
        });
    }
    if props.nil_kind == Some(NilKind::LiteralCharacter) {
        for token in ["%NL;", "%ES;", "%WSP;", "%WSP+;", "%WSP*;"] {
            if raw.contains(token) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Schema Definition Error: Property dfdl:nilValue contains disallowed character class(es): {token}"
                    ),
                });
            }
        }
        for alt in crate::schema::nil_value_alternatives(raw) {
            let expanded = crate::schema::expand_entities(alt.trim());
            let as_text = String::from_utf8_lossy(&expanded);
            if as_text.chars().count() != 1 {
                return Err(VmError::InvalidValue {
                    message: "Schema Definition Error: For property dfdl:nilValue the length of string must be exactly 1 character.".into(),
                });
            }
        }
    }
    let _ = strings;
    Ok(())
}

pub(crate) fn enclosing_terminator_at_cursor(
    cursor: &Cursor<'_>,
    stop_sequences: &[&IrProps],
    strings: &StringPool,
) -> Result<bool, VmError> {
    for stop in stop_sequences {
        let Some(id) = stop.terminator else {
            continue;
        };
        let pat = strings.get(id)?;
        if pat.is_empty() {
            continue;
        }
        let enc = encoding_name(stop, strings).ok();
        if crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            stop.ignore_case,
            enc,
        )
        .is_some_and(|n| n > 0)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn try_consume_nillable_element_nil(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    parent_sequence: Option<&IrProps>,
    strings: &StringPool,
    // When true, empty nil may consume the enclosing sequence separator (complex `/after` nil).
    empty_nil_at_parent_separator: bool,
    enclosing_stops: &[&IrProps],
) -> Result<bool, VmError> {
    if !props.nillable {
        return Ok(false);
    }
    if !matches!(
        props.nil_kind,
        Some(NilKind::LiteralValue) | Some(NilKind::LiteralCharacter)
    ) {
        return Ok(false);
    }
    let empty_nil = nil_value_includes_empty(props, strings)?;
    let saved = cursor.pos;
    if let Some(nil_len) = match_nil_literal_prefix(cursor, props, strings)? {
        cursor.advance(nil_len);
        if let Some(term_id) = props.terminator {
            let term = strings.get(term_id)?;
            if !term.is_empty()
                && crate::schema::match_delimiter_opts(
                    &cursor.data[cursor.pos..],
                    term,
                    props.ignore_case,
                )
                .is_some()
            {
                let parent_owns =
                    parent_sequence.is_some_and(|parent| parent.terminator == Some(term_id));
                if !parent_owns {
                    let enc = encoding_name(props, strings).ok();
                    let _ = cursor.consume_delimiter(term, props.ignore_case, enc);
                }
                return Ok(true);
            }
        }
        if props.nil_kind == Some(NilKind::LiteralValue) && nil_len > 0 {
            return Ok(true);
        }
        if empty_nil && nil_len == 0 {
            // fall through to parent-separator / EOS empty-nil checks below
        } else {
            cursor.pos = saved;
        }
    }
    if empty_nil {
        if let Some(term_id) = props.terminator {
            let term = strings.get(term_id)?;
            if !term.is_empty()
                && crate::schema::match_delimiter_opts(
                    &cursor.data[cursor.pos..],
                    term,
                    props.ignore_case,
                )
                .is_some()
            {
                let parent_owns =
                    parent_sequence.is_some_and(|parent| parent.terminator == Some(term_id));
                if parent_owns {
                    return Ok(true);
                }
                let enc = encoding_name(props, strings).ok();
                let _ = cursor.consume_delimiter(term, props.ignore_case, enc);
                return Ok(true);
            }
        }
        if let Some(parent) = parent_sequence {
            if empty_nil_at_parent_separator {
                if let Some(sep_id) = parent.separator {
                    let sep = strings.get(sep_id)?;
                    if !sep.is_empty()
                        && crate::schema::match_delimiter_opts(
                            &cursor.data[cursor.pos..],
                            sep,
                            parent.ignore_case,
                        )
                        .is_some()
                    {
                        if parent.separator_position == crate::schema::SeparatorPosition::Infix {
                            return Ok(true);
                        }
                        let enc = encoding_name(parent, strings).ok();
                        let _ = cursor.consume_delimiter(sep, parent.ignore_case, enc);
                        return Ok(true);
                    }
                }
            } else if let Some(sep_id) = parent.separator {
                let sep = strings.get(sep_id)?;
                if !sep.is_empty()
                    && crate::schema::match_delimiter_opts(
                        &cursor.data[cursor.pos..],
                        sep,
                        parent.ignore_case,
                    )
                    .is_some()
                {
                    return Ok(true);
                }
            }
            if let Some(term_id) = parent.terminator {
                let term = strings.get(term_id)?;
                if !term.is_empty()
                    && crate::schema::match_delimiter_opts(
                        &cursor.data[cursor.pos..],
                        term,
                        parent.ignore_case,
                    )
                    .is_some()
                {
                    return Ok(true);
                }
            }
        }
        if cursor.pos >= cursor.data.len() {
            return Ok(true);
        }
        if enclosing_terminator_at_cursor(cursor, enclosing_stops, strings)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn match_nil_literal_prefix(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Option<usize>, VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(None);
    };
    let mut best: Option<usize> = None;
    let data = &cursor.data[cursor.pos..];
    for alt in alts {
        if alt.contains('%') {
            if let Some(len) = crate::schema::match_pattern_opts_for_encoding(
                data,
                alt.as_str(),
                props.ignore_case,
                None,
            ) {
                if len > 0 && best.map(|prev| len > prev).unwrap_or(true) {
                    best = Some(len);
                }
            }
            continue;
        }
        let bytes = crate::schema::expand_entities(&alt);
        if bytes.is_empty() {
            continue;
        }
        let matched = if props.ignore_case {
            data.len() >= bytes.len()
                && data[..bytes.len()]
                    .iter()
                    .zip(bytes.iter())
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
        } else {
            data.starts_with(&bytes)
        };
        if matched {
            let len = bytes.len();
            if best.map(|prev| len > prev).unwrap_or(true) {
                best = Some(len);
            }
        }
    }
    Ok(best)
}
