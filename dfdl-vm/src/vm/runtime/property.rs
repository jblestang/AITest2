use super::numeric_text_parse::{
    parse_int_with_base, parse_text_boolean, parse_unbounded_integer_decimal, parse_unsigned_radix,
};
use super::scalar::{decode_hex, parse_float};
use crate::ir::{IrProps, StringPool};
use alloc::string::{String, ToString};

pub(crate) fn parse_sibling_property_expr(raw: &str) -> Option<alloc::string::String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let path = inner.strip_prefix("../")?;
    Some(path.rsplit(':').next().unwrap_or(path).trim().to_string())
}

pub(crate) fn sibling_string_from_encode_map(
    map: &alloc::collections::BTreeMap<String, crate::value::DfdlValue>,
    local: &str,
) -> Option<alloc::string::String> {
    for (k, v) in map {
        if crate::xml_util::local_name_str(k) == local {
            return sibling_string_from_dfdl_value(v);
        }
    }
    map.get(local).and_then(sibling_string_from_dfdl_value)
}

fn sibling_string_from_dfdl_value(
    value: &crate::value::DfdlValue,
) -> Option<alloc::string::String> {
    match value {
        crate::value::DfdlValue::String(s) => Some(s.text.clone()),
        crate::value::DfdlValue::Decimal(s) | crate::value::DfdlValue::DateTime(s) => {
            Some(s.clone())
        }
        _ => None,
    }
}

pub(crate) fn delimited_field_escape_markup(
    field: &IrProps,
    parent_seq: Option<&IrProps>,
    strings: &StringPool,
    siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
) -> alloc::vec::Vec<alloc::string::String> {
    use crate::schema::LengthKind;
    let mut out = alloc::vec::Vec::new();
    let mut push_resolved = |raw: &str| {
        let pat = resolve_encode_property_pattern(raw, siblings);
        for alt in crate::schema::delimiter_alternatives(&pat) {
            if !alt.is_empty() && !out.iter().any(|x| x == &alt) {
                out.push(alt);
            }
        }
    };
    if field.length_kind != LengthKind::Delimited {
        return out;
    }
    if let Some(id) = field.initiator {
        if let Ok(raw) = strings.get(id) {
            push_resolved(raw);
        }
    }
    if let Some(id) = field.terminator {
        if let Ok(raw) = strings.get(id) {
            push_resolved(raw);
        }
    }
    if let Some(seq) = parent_seq {
        if let Some(id) = seq.separator {
            if let Ok(raw) = strings.get(id) {
                push_resolved(raw);
            }
        }
    }
    out
}

pub(crate) fn resolve_escape_scheme_runtime(
    scheme: &crate::schema::EscapeSchemeDef,
    siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
) -> crate::schema::EscapeSchemeDef {
    let mut resolved = scheme.clone();
    if let Some(raw) = scheme.escape_character_raw.as_deref() {
        resolved.escape_character = Some(resolve_encode_property_pattern(raw, siblings));
    } else if let Some(raw) = scheme.escape_character.as_deref() {
        if parse_sibling_property_expr(raw).is_some() {
            resolved.escape_character = Some(resolve_encode_property_pattern(raw, siblings));
        }
    }
    if let Some(raw) = scheme.escape_escape_character_raw.as_deref() {
        resolved.escape_escape_character = Some(resolve_encode_property_pattern(raw, siblings));
    } else if let Some(raw) = scheme.escape_escape_character.as_deref() {
        if parse_sibling_property_expr(raw).is_some() {
            resolved.escape_escape_character = Some(resolve_encode_property_pattern(raw, siblings));
        }
    }
    if let Some(raw) = scheme.escape_block_start_raw.as_deref() {
        resolved.escape_block_start = Some(resolve_encode_property_pattern(raw, siblings));
    }
    if let Some(raw) = scheme.escape_block_end_raw.as_deref() {
        resolved.escape_block_end = Some(resolve_encode_property_pattern(raw, siblings));
    }
    if let Some(raw) = scheme.extra_escaped_characters_raw.as_deref() {
        if parse_sibling_property_expr(raw).is_some() || raw.trim().starts_with('{') {
            let text = resolve_encode_property_pattern(raw, siblings);
            resolved.extra_escaped_characters =
                crate::schema::extra_escaped_characters_from_property(&text);
        }
    }
    resolved
}

pub(crate) fn resolve_encode_property_pattern(
    raw: &str,
    siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
) -> alloc::string::String {
    if let Some(map) = siblings {
        if let Some(sib) = parse_sibling_property_expr(raw) {
            if let Some(text) = sibling_string_from_encode_map(map, &sib) {
                return text;
            }
        }
    }
    raw.to_string()
}

fn parse_encode_dfdl_entities_sibling(raw: &str) -> Option<alloc::string::String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let prefix = "dfdl:encodeDFDLEntities(";
    if !inner.starts_with(prefix) || !inner.ends_with(')') {
        return None;
    }
    let path = inner[prefix.len()..inner.len() - 1].trim();
    let path = path.strip_prefix("../")?;
    Some(path.rsplit(':').next().unwrap_or(path).trim().to_string())
}

pub fn resolve_output_new_line_for_encode(
    props: &IrProps,
    siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
    strings: &StringPool,
) -> Result<Option<alloc::string::String>, crate::error::VmError> {
    if let Some(id) = props.output_new_line_sibling {
        let local = strings.get(id)?;
        if let Some(map) = siblings {
            if let Some(text) = sibling_string_from_encode_map(map, local) {
                return Ok(Some(text));
            }
            if let Some(val) = map.get(local) {
                if let Some(text) = sibling_string_from_dfdl_value(val) {
                    return Ok(Some(text));
                }
            }
        }
        return Ok(None);
    }
    let Some(id) = props.output_new_line else {
        return Ok(None);
    };
    let raw = strings.get(id)?;
    if let Some(local) = parse_encode_dfdl_entities_sibling(raw) {
        if let Some(map) = siblings {
            if let Some(text) = sibling_string_from_encode_map(map, &local) {
                return Ok(Some(text));
            }
        }
        return Ok(None);
    }
    Ok(Some(raw.to_string()))
}

pub(crate) fn encode_framing_delimiter_bytes(
    pattern: &str,
    output_new_line: Option<&str>,
    encoding: &str,
    delim_alt: Option<u8>,
) -> alloc::vec::Vec<u8> {
    if let Some(alt) = delim_alt {
        return crate::schema::encode_delimiter_by_alt(pattern, alt);
    }
    crate::schema::encode_delimiter_for_encoding(pattern, output_new_line, Some(encoding))
}

pub(crate) fn default_value_for(
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Option<crate::value::DfdlValue> {
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let raw = props.default_value.and_then(|id| strings.get(id).ok())?;
    let base = props.text_standard_base;
    match kind {
        Boolean => parse_text_boolean(raw, props, strings, None)
            .ok()
            .map(DfdlValue::Boolean),
        Byte => parse_int_with_base(raw, "xs:byte", base)
            .ok()
            .map(DfdlValue::Byte),
        UnsignedByte => parse_unsigned_radix(raw, base)
            .ok()
            .map(DfdlValue::UnsignedByte),
        Short => parse_int_with_base(raw, "xs:short", base)
            .ok()
            .map(DfdlValue::Short),
        UnsignedShort => parse_unsigned_radix(raw, base)
            .ok()
            .map(DfdlValue::UnsignedShort),
        Int => parse_int_with_base(raw, "xs:int", base)
            .ok()
            .map(DfdlValue::Int),
        Integer => parse_unbounded_integer_decimal(raw, base, false)
            .ok()
            .map(DfdlValue::Integer),
        UnsignedInt => parse_unsigned_radix(raw, base)
            .ok()
            .map(DfdlValue::UnsignedInt),
        Long => parse_int_with_base(raw, "xs:long", base)
            .ok()
            .map(DfdlValue::Long),
        Float => parse_float(raw).ok().map(|v| DfdlValue::Float(v as f32)),
        Double => parse_float(raw).ok().map(DfdlValue::Double),
        Decimal => Some(DfdlValue::Decimal(raw.into())),
        DateTime | Time => Some(DfdlValue::DateTime(raw.into())),
        String => Some(DfdlValue::string(raw)),
        HexBinary => decode_hex(raw).ok().map(DfdlValue::HexBinary),
        Complex => None,
    }
}
