use super::binary::{write_bits_from_stream_with_config, write_byte_aligned};
use super::calendar_text::*;
use super::encoding_name;
use super::framed_payload::write_prefixed_bytes;
use super::nil::nil_unparse_bytes;
use super::property::{
    delimited_field_escape_markup, resolve_encode_property_pattern,
    resolve_escape_scheme_runtime,
};
use super::scalar::encode_hex;
use super::RuntimeConfig;
use crate::ir::{IrProps, StringPool, ValueKind};
use crate::schema::*;
use crate::value::DfdlValue;
use crate::vm::text_number_format::TextNumberRoundingProps;
use crate::vm::encoding::{
    bits_charset_spec, count_characters, encode_document_text, remap_pua_to_xml_illegal_characters,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub(crate) fn length_kind_name(kind: LengthKind) -> &'static str {
    match kind {
        LengthKind::Implicit => "implicit",
        LengthKind::Explicit => "explicit",
        LengthKind::Fixed => "fixed",
        LengthKind::Delimited => "delimited",
        LengthKind::Prefixed => "prefixed",
        LengthKind::Pattern => "pattern",
        LengthKind::EndOfParent => "endOfParent",
    }
}

pub(crate) fn write_text_scalar(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
    config: &RuntimeConfig,
    field_name: Option<&str>,
    encode_siblings: Option<&alloc::collections::BTreeMap<String, DfdlValue>>,
    encode_escape_parent: Option<&IrProps>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use ValueKind::*;

    if matches!(value, DfdlValue::Null) {
        let payload = nil_unparse_bytes(props, strings)?;
        write_byte_aligned(out, bit_count, &payload)?;
        return Ok(());
    }

    let source_bytes = match (kind, value) {
        (String, DfdlValue::String(v)) => v.meta.source_bytes.clone(),
        _ => None,
    };

    let numeric_radix = props.text_number_rep == TextNumberRep::Standard
        && props.text_standard_base != 10
        && matches!(
            kind,
            ValueKind::Byte
                | ValueKind::Short
                | ValueKind::Int
                | ValueKind::Long
                | ValueKind::Integer
                | ValueKind::UnsignedByte
                | ValueKind::UnsignedShort
                | ValueKind::UnsignedInt
        );

    let text = if numeric_radix {
        format_text_standard_radix_unparse(value, kind, props)?
    } else {
        match (kind, value) {
            (Boolean, DfdlValue::Boolean(v)) => {
                let true_reps = text_boolean_rep_candidates(props, strings, true, None)?;
                let false_reps = text_boolean_rep_candidates(props, strings, false, None)?;
                if *v {
                    let picked = pick_text_boolean_unparse_rep(&true_reps);
                    if picked.is_empty() {
                        alloc::string::String::from("true")
                    } else {
                        picked
                    }
                } else {
                    let picked = pick_text_boolean_unparse_rep(&false_reps);
                    if picked.is_empty() {
                        alloc::string::String::from("false")
                    } else {
                        picked
                    }
                }
            }
            (Byte, DfdlValue::Byte(v)) => format!("{v}"),
            (UnsignedByte, DfdlValue::UnsignedByte(v)) => format!("{v}"),
            (Short, DfdlValue::Short(v)) => format!("{v}"),
            (UnsignedShort, DfdlValue::UnsignedShort(v)) => format!("{v}"),
            (Int, DfdlValue::Int(v)) => format!("{v}"),
            (Integer, DfdlValue::Integer(v)) => v.clone(),
            (UnsignedInt, DfdlValue::UnsignedInt(v)) => format!("{v}"),
            (Long, DfdlValue::Long(v)) => format!("{v}"),
            (Float, DfdlValue::Float(v)) => format!("{v}"),
            (Double, DfdlValue::Double(v)) => format!("{v}"),
            (Decimal, DfdlValue::Decimal(v)) => v.clone(),
            (DateTime, DfdlValue::DateTime(v)) | (Time, DfdlValue::DateTime(v)) => {
                if let Some(pat_id) = props.calendar_pattern {
                    let pattern = strings.get(pat_id)?;
                    let cal_lang = resolve_calendar_language(
                        props,
                        strings,
                        sibling_text_map_for_calendar(encode_siblings).as_ref(),
                    )?;
                    if crate::vm::calendar_binary::calendar_pattern_time_only(pattern)
                        || kind == ValueKind::Time
                    {
                        unparse_iso_time_to_calendar_pattern(v, pattern)?
                    } else {
                        unparse_iso_date_to_calendar_pattern(v, pattern, cal_lang.as_deref())?
                    }
                } else {
                    v.clone()
                }
            }
            (String, DfdlValue::String(v)) => {
                if config.encode_pua_codepoints_as_utf8 {
                    v.text.clone()
                } else {
                    remap_pua_to_xml_illegal_characters(&v.text)
                }
            }
            (String, other) => {
                let text = crate::vm::decoder::xpath::dfdl_value_to_string(other);
                if config.encode_pua_codepoints_as_utf8 {
                    text
                } else {
                    remap_pua_to_xml_illegal_characters(&text)
                }
            }
            (HexBinary, DfdlValue::HexBinary(v)) => encode_hex(v),
            (expected, _) => {
                return Err(VmError::TypeMismatch {
                    expected: format!("{expected:?}"),
                });
            }
        }
    };

    let text = if numeric_radix {
        text
    } else {
        format_field_text_number(&text, kind, props, strings)?
    };
    let text = if kind == ValueKind::String
        && props.escape_scheme.is_some()
        && matches!(
            props.length_kind,
            LengthKind::Delimited
                | LengthKind::Pattern
                | LengthKind::Implicit
                | LengthKind::EndOfParent
        ) {
        if let Some(scheme) = props.escape_scheme.as_ref() {
            let resolved = resolve_escape_scheme_runtime(scheme, encode_siblings);
            if resolved.escape_kind == EscapeKind::EscapeCharacter {
                if let Some(esc) = resolved
                    .escape_character
                    .as_deref()
                    .filter(|s| !s.is_empty())
                {
                    if let Some(parent) = encode_escape_parent {
                        if let Some(id) = parent.separator {
                            if let Ok(raw) = strings.get(id) {
                                let sep = resolve_encode_property_pattern(raw, encode_siblings);
                                if crate::schema::delimiter_alternatives(&sep)
                                    .into_iter()
                                    .any(|alt| alt.starts_with(esc) || alt == esc)
                                {
                                    return Err(VmError::InvalidValue {
                                        message: "Unparse Error: dfdl:terminator and dfdl:separator properties may not begin with the dfdl:escapeCharacter property value.".into(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            let markup = delimited_field_escape_markup(
                props,
                encode_escape_parent,
                strings,
                encode_siblings,
            );
            let block_markup_owned: Vec<alloc::string::String>;
            let markup_refs: Vec<&str> = match scheme.escape_kind {
                EscapeKind::EscapeBlock => {
                    block_markup_owned = delimited_field_escape_markup(
                        props,
                        encode_escape_parent,
                        strings,
                        encode_siblings,
                    );
                    block_markup_owned.iter().map(|s| s.as_str()).collect()
                }
                EscapeKind::EscapeCharacter => markup.iter().map(|s| s.as_str()).collect(),
            };
            crate::vm::escape::escape_field_text(&text, &resolved, &markup_refs)
        } else {
            text
        }
    } else {
        text
    };
    let text_before_pad = text.clone();
    let text = apply_min_length_pad(&text, props, strings, kind);

    if props.length_kind == LengthKind::Prefixed {
        let encoded = if let Some(raw) = source_bytes.filter(|_| text == text_before_pad) {
            raw
        } else {
            encode_document_text(&text, encoding_name(props, strings)?)?
        };
        return write_prefixed_bytes(out, bit_count, &encoded, props, strings, field_name);
    }

    let encoding = encoding_name(props, strings)?;
    if let Some(raw) = source_bytes.filter(|_| text == text_before_pad) {
        let payload = match props.length_kind {
            LengthKind::Fixed | LengthKind::Explicit => {
                let len = props.length.ok_or(VmError::InvalidValue {
                    message: "Unparse Error: Value length unknown".into(),
                })? as usize;
                pad_raw_text_field(
                    &raw,
                    len,
                    props.length_units,
                    props,
                    strings,
                    kind,
                    encoding,
                )?
            }
            LengthKind::Delimited
            | LengthKind::Pattern
            | LengthKind::Implicit
            | LengthKind::EndOfParent => raw,
            other => {
                return Err(VmError::UnsupportedOperation {
                    op: format!("text lengthKind `{}` encode", length_kind_name(other)),
                });
            }
        };
        write_byte_aligned(out, bit_count, &payload)?;
        return Ok(());
    }

    let payload = match props.length_kind {
        LengthKind::Fixed | LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "Unparse Error: Value length unknown".into(),
            })? as usize;
            if props.length_units == LengthUnits::Bits {
                let encoded = encode_document_text(&text, encoding)?;
                write_bits_from_stream_with_config(
                    out,
                    bit_count,
                    &encoded,
                    len,
                    props.bit_order,
                    Some(config),
                )?;
                return Ok(());
            }
            let text = truncate_string_for_explicit_length(
                &text,
                len,
                props.length_units,
                encoding,
                props,
                kind,
                field_name,
            )?;
            pad_text_field(
                &text,
                len,
                props.length_units,
                props,
                strings,
                kind,
                encoding,
            )?
        }
        LengthKind::Delimited
        | LengthKind::Pattern
        | LengthKind::Implicit
        | LengthKind::EndOfParent => {
            let encoded = encode_document_text(&text, encoding)?;
            if let Some(spec) = bits_charset_spec(encoding) {
                let len = text.chars().count().saturating_mul(spec.width as usize);
                write_bits_from_stream_with_config(
                    out,
                    bit_count,
                    &encoded,
                    len,
                    props.bit_order,
                    Some(config),
                )?;
                return Ok(());
            }
            encoded
        }
        other => {
            return Err(VmError::UnsupportedOperation {
                op: format!("text lengthKind `{}` encode", length_kind_name(other)),
            });
        }
    };
    write_byte_aligned(out, bit_count, &payload)?;
    Ok(())
}

fn apply_min_length_pad(
    text: &str,
    props: &IrProps,
    strings: &StringPool,
    kind: ValueKind,
) -> String {
    use ValueKind;

    let Some(min_len) = props.min_length else {
        return text.to_string();
    };
    if props.text_pad_kind != TextPadKind::PadChar {
        return text.to_string();
    }
    if kind != ValueKind::String {
        return text.to_string();
    }
    let mut min_len = min_len as usize;
    if matches!(props.length_kind, LengthKind::Fixed | LengthKind::Explicit) {
        if let Some(explicit) = props.length {
            min_len = min_len.min(explicit as usize);
        }
    }
    let current = text.chars().count();
    if current >= min_len {
        return text.to_string();
    }
    let pad_char = pad_char_for_kind(props, strings, kind);
    let pad_ch = pad_char.chars().next().unwrap_or(' ');
    let pad_count = min_len - current;
    match text_justification_for_kind(props, kind) {
        TextStringJustification::Right => {
            let mut out = String::new();
            for _ in 0..pad_count {
                out.push(pad_ch);
            }
            out.push_str(text);
            out
        }
        TextStringJustification::Center => {
            let right = pad_count / 2;
            let left = pad_count - right;
            let mut out = String::new();
            for _ in 0..left {
                out.push(pad_ch);
            }
            out.push_str(text);
            for _ in 0..right {
                out.push(pad_ch);
            }
            out
        }
        TextStringJustification::Left => {
            let mut out = text.to_string();
            for _ in 0..pad_count {
                out.push(pad_ch);
            }
            out
        }
    }
}

pub(crate) fn pad_text_field(
    text: &str,
    len: usize,
    units: LengthUnits,
    props: &IrProps,
    strings: &StringPool,
    kind: ValueKind,
    encoding: &str,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;

    let pad_char = pad_char_for_kind(props, strings, kind);
    let pad_byte = pad_char.chars().next().unwrap_or(b' ' as char) as u8;
    let justification = text_justification_for_kind(props, kind);

    match units {
        LengthUnits::Bytes => {
            let mut bytes = text.as_bytes().to_vec();
            if bytes.len() > len {
                if props.truncate_specified_length_string {
                    bytes = match justification {
                        TextStringJustification::Center => {
                            return Err(VmError::InvalidValue {
                                message: "Unparse Error: dfdl:textStringJustification=\"center\" cannot be used with dfdl:truncateSpecifiedLengthString=\"yes\" when truncation is required".to_string(),
                            });
                        }
                        TextStringJustification::Left => bytes[..len].to_vec(),
                        TextStringJustification::Right => bytes[bytes.len() - len..].to_vec(),
                    };
                } else {
                    return Err(VmError::InvalidValue {
                        message: "text value too long for explicit length".into(),
                    });
                }
            }
            let pad_count = len - bytes.len();
            match justification {
                TextStringJustification::Right => {
                    bytes.splice(0..0, std::iter::repeat_n(pad_byte, pad_count));
                }
                TextStringJustification::Center => {
                    let right = pad_count / 2;
                    let left = pad_count - right;
                    bytes.splice(0..0, std::iter::repeat_n(pad_byte, left));
                    bytes.extend(std::iter::repeat_n(pad_byte, right));
                }
                TextStringJustification::Left => {
                    bytes.extend(std::iter::repeat_n(pad_byte, pad_count));
                }
            }
            Ok(bytes)
        }
        LengthUnits::Characters => {
            let current = count_characters(text.as_bytes(), encoding, EncodingErrorPolicy::Error)?;
            if current > len {
                return Err(VmError::InvalidValue {
                    message: "text value too long for explicit character length".into(),
                });
            }
            let mut padded = text.to_string();
            let pad_count = len - current;
            let pad_str: String = pad_char.chars().take(1).collect();
            match justification {
                TextStringJustification::Right => {
                    for _ in 0..pad_count {
                        padded.insert_str(0, &pad_str);
                    }
                }
                TextStringJustification::Center => {
                    let right = pad_count / 2;
                    let left = pad_count - right;
                    for _ in 0..left {
                        padded.insert_str(0, &pad_str);
                    }
                    for _ in 0..right {
                        padded.push_str(&pad_str);
                    }
                }
                TextStringJustification::Left => {
                    for _ in 0..pad_count {
                        padded.push_str(&pad_str);
                    }
                }
            }
            encode_document_text(&padded, encoding)
        }
        LengthUnits::Bits => Err(VmError::UnsupportedOperation {
            op: "explicit text bit length encode".into(),
        }),
    }
}

fn pad_raw_text_field(
    raw: &[u8],
    len: usize,
    units: LengthUnits,
    props: &IrProps,
    strings: &StringPool,
    kind: ValueKind,
    encoding: &str,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;

    let pad_char = pad_char_for_kind(props, strings, kind);
    let pad_byte = pad_char.chars().next().unwrap_or(b' ' as char) as u8;
    let justification = text_justification_for_kind(props, kind);

    match units {
        LengthUnits::Bytes => {
            let mut bytes = raw.to_vec();
            if bytes.len() > len {
                bytes.truncate(len);
                return Ok(bytes);
            }
            let pad_count = len - bytes.len();
            match justification {
                TextStringJustification::Right => {
                    bytes.splice(0..0, std::iter::repeat_n(pad_byte, pad_count));
                }
                TextStringJustification::Center => {
                    let left = pad_count / 2;
                    let right = pad_count - left;
                    bytes.splice(0..0, std::iter::repeat_n(pad_byte, left));
                    bytes.extend(std::iter::repeat_n(pad_byte, right));
                }
                TextStringJustification::Left => {
                    bytes.extend(std::iter::repeat_n(pad_byte, pad_count));
                }
            }
            Ok(bytes)
        }
        LengthUnits::Characters => {
            let current = count_characters(raw, encoding, EncodingErrorPolicy::Replace)?;
            if current > len {
                return Err(VmError::InvalidValue {
                    message: "text value too long for explicit character length".into(),
                });
            }
            if current == len {
                return Ok(raw.to_vec());
            }
            let pad_count = len - current;
            let pad_bytes = encode_document_text(&pad_char, encoding)?;
            let mut out = Vec::new();
            match justification {
                TextStringJustification::Right => {
                    for _ in 0..pad_count {
                        out.extend_from_slice(&pad_bytes);
                    }
                    out.extend_from_slice(raw);
                }
                TextStringJustification::Center => {
                    let left = pad_count / 2;
                    let right = pad_count - left;
                    for _ in 0..left {
                        out.extend_from_slice(&pad_bytes);
                    }
                    out.extend_from_slice(raw);
                    for _ in 0..right {
                        out.extend_from_slice(&pad_bytes);
                    }
                }
                TextStringJustification::Left => {
                    out.extend_from_slice(raw);
                    for _ in 0..pad_count {
                        out.extend_from_slice(&pad_bytes);
                    }
                }
            }
            Ok(out)
        }
        LengthUnits::Bits => Err(VmError::UnsupportedOperation {
            op: "explicit text bit length encode".into(),
        }),
    }
}

pub(crate) fn format_u128_radix_lower(mut val: u128, base: u32) -> alloc::string::String {
    if val == 0 {
        return "0".into();
    }
    let mut digits = alloc::vec::Vec::new();
    while val > 0 {
        let rem = (val % base as u128) as u32;
        val /= base as u128;
        let ch = if rem < 10 {
            (b'0' + rem as u8) as char
        } else {
            (b'a' + (rem - 10) as u8) as char
        };
        digits.push(ch);
    }
    digits.into_iter().rev().collect()
}

pub(crate) fn format_text_standard_radix_unparse(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    use crate::value::DfdlValue;

    let base = props.text_standard_base;
    let (negative, magnitude) = match (kind, value) {
        (ValueKind::Byte, DfdlValue::Byte(v)) => (*v < 0, (*v as i64).unsigned_abs() as u128),
        (ValueKind::Byte, DfdlValue::Long(v)) => {
            if *v < 0 {
                (true, v.unsigned_abs() as u128)
            } else {
                (false, *v as u128)
            }
        }
        (ValueKind::Short, DfdlValue::Short(v)) => (*v < 0, (*v as i64).unsigned_abs() as u128),
        (ValueKind::Int, DfdlValue::Int(v)) => (*v < 0, (*v as i64).unsigned_abs() as u128),
        (ValueKind::Long, DfdlValue::Long(v)) => (*v < 0, v.unsigned_abs() as u128),
        (ValueKind::Integer, DfdlValue::Integer(s)) => {
            let trimmed = s.trim();
            if trimmed.starts_with('-') {
                (true, trimmed[1..].parse::<u128>().unwrap_or(0))
            } else {
                (false, trimmed.parse::<u128>().unwrap_or(0))
            }
        }
        (ValueKind::UnsignedByte, DfdlValue::UnsignedByte(v)) => (false, *v as u128),
        (ValueKind::UnsignedShort, DfdlValue::UnsignedShort(v)) => (false, *v as u128),
        (ValueKind::UnsignedInt, DfdlValue::UnsignedInt(v)) => (false, *v as u128),
        (other, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{other:?}"),
            });
        }
    };

    if negative {
        if props.non_negative_integer {
            let raw = match value {
                DfdlValue::Integer(s) => s.clone(),
                DfdlValue::Long(v) => alloc::format!("{v}"),
                DfdlValue::Int(v) => alloc::format!("{v}"),
                DfdlValue::Byte(v) => alloc::format!("{v}"),
                DfdlValue::Short(v) => alloc::format!("{v}"),
                _ => alloc::format!("-{magnitude}"),
            };
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Unparse Error: value `{raw}` out of range for type xs:nonNegativeInteger"
                ),
            });
        }
        let display = match value {
            DfdlValue::Int(v) => alloc::format!("{v}"),
            DfdlValue::Long(v) => alloc::format!("{v}"),
            DfdlValue::Byte(v) => alloc::format!("{v}"),
            DfdlValue::Short(v) => alloc::format!("{v}"),
            DfdlValue::Integer(s) => s.clone(),
            _ => alloc::format!("-{magnitude}"),
        };
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Unparse Error: Unable to unparse negative value `{display}` when textStandardBase=\"{base}\""
            ),
        });
    }

    Ok(format_u128_radix_lower(magnitude, base))
}

pub(crate) fn text_boolean_rep_candidates(
    props: &IrProps,
    strings: &StringPool,
    true_side: bool,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
) -> Result<alloc::vec::Vec<alloc::string::String>, crate::error::VmError> {
    use crate::error::VmError;
    let id = if true_side {
        props.text_boolean_true_rep
    } else {
        props.text_boolean_false_rep
    };
    let Some(id) = id else {
        return Ok(alloc::vec::Vec::new());
    };
    let raw = strings.get(id)?;
    let tokens = crate::schema::boolean_reps::tokenize_text_boolean_rep_list(raw);
    let mut out = alloc::vec::Vec::new();
    for tok in tokens {
        let (sibling_text, sibling_bytes) = sibling_env
            .map(|e| (Some(e.text), Some(e.content_bytes)))
            .unwrap_or((None, None));
        let s = crate::schema::boolean_reps::resolve_text_boolean_rep_token(
            &tok,
            sibling_text,
            sibling_bytes,
        )
        .map_err(|detail| VmError::InvalidValue { message: detail })?;
        out.push(s);
    }
    Ok(out)
}

pub(crate) fn pick_text_boolean_unparse_rep(reps: &[alloc::string::String]) -> alloc::string::String {
    let Some(s) = reps.first() else {
        return alloc::string::String::new();
    };
    s.split_whitespace()
        .next()
        .unwrap_or(s.as_str())
        .to_string()
}

pub(crate) fn format_field_text_number(
    text: &str,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::ir::ValueKind;
    use crate::schema::TextNumberRep;

    if props.text_number_rep != TextNumberRep::Standard || props.text_standard_base != 10 {
        return Ok(text.to_string());
    }
    if !matches!(
        kind,
        ValueKind::Byte
            | ValueKind::Short
            | ValueKind::Int
            | ValueKind::Long
            | ValueKind::Integer
            | ValueKind::UnsignedByte
            | ValueKind::UnsignedShort
            | ValueKind::UnsignedInt
            | ValueKind::Float
            | ValueKind::Double
            | ValueKind::Decimal
    ) {
        return Ok(text.to_string());
    }
    let Some(pat_id) = props.text_number_pattern else {
        return Ok(text.to_string());
    };
    let raw_pattern = strings.get(pat_id)?;
    let pattern_owned = if props.text_number_rep == TextNumberRep::Zoned {
        crate::vm::zoned_text::strip_zoned_plus_markers(raw_pattern)
    } else {
        raw_pattern.to_string()
    };
    let pattern = pattern_owned.as_str();
    if pattern.contains('V')
        && !pattern.contains('E')
        && !pattern.contains('e')
        && !pattern.contains('\'')
        && !pattern.contains(';')
    {
        return Ok(text.to_string());
    }
    let (dec_seps, grouping, exponent, pad, check_policy) =
        super::numeric_text_parse::resolved_text_number_format_parts(props, strings);
    let fmt = crate::vm::text_number::TextNumberFormatProps {
        check_policy,
        decimal_separators: &dec_seps,
        grouping_separator: grouping.as_deref(),
        exponent_chars: &exponent,
        pad_character: pad,
        ignore_case: props.ignore_case,
    };
    if props.text_standard_zero_rep_defined {
        if let Ok(raw) = strings.get(props.text_standard_zero_rep) {
            if let Some(z) = crate::vm::text_number_format::text_standard_zero_unparse(text, raw) {
                return Ok(z);
            }
        }
    }

    let increment_owned = if props.text_number_rounding_increment_defined {
        strings
            .get(props.text_number_rounding_increment)?
            .to_string()
    } else {
        alloc::string::String::from("0")
    };
    let rounding = TextNumberRoundingProps {
        rounding: props.text_number_rounding,
        mode: props.text_number_rounding_mode,
        increment: increment_owned.as_str(),
    };
    crate::vm::text_number_format::format_standard_text_number(text, pattern, &fmt, rounding)
}

pub(crate) fn truncate_string_for_explicit_length(
    text: &str,
    len: usize,
    units: LengthUnits,
    encoding: &str,
    props: &IrProps,
    kind: crate::ir::ValueKind,
    field_name: Option<&str>,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    use crate::vm::encoding::count_characters;

    let too_long = match units {
        LengthUnits::Bytes => text.len() > len,
        LengthUnits::Characters => {
            count_characters(text.as_bytes(), encoding, EncodingErrorPolicy::Error)? > len
        }
        LengthUnits::Bits => text.len() * 8 > len,
    };
    if !too_long {
        return Ok(text.to_string());
    }
    if !props.truncate_specified_length_string {
        let mut message =
            "Unparse Error: data too long for explicit length and unable to truncate".to_string();
        if let Some(name) = field_name {
            message.push_str("\nSchema context: ");
            message.push_str(name);
        }
        return Err(VmError::InvalidValue { message });
    }
    match text_justification_for_kind(props, kind) {
        TextStringJustification::Center => Err(VmError::InvalidValue {
            message: "Unparse Error: dfdl:textStringJustification=\"center\" cannot be used with dfdl:truncateSpecifiedLengthString=\"yes\" when truncation is required".to_string(),
        }),
        TextStringJustification::Left => {
            let out = match units {
                LengthUnits::Bytes => {
                    if text.len() <= len {
                        text.to_string()
                    } else {
                        text[..len].to_string()
                    }
                }
                LengthUnits::Characters => {
                    let mut out = alloc::string::String::new();
                    for (idx, ch) in text.chars().enumerate() {
                        if idx >= len {
                            break;
                        }
                        out.push(ch);
                    }
                    out
                }
                LengthUnits::Bits => text.to_string(),
            };
            Ok(out)
        }
        TextStringJustification::Right => {
            let out = match units {
                LengthUnits::Bytes => {
                    if text.len() <= len {
                        text.to_string()
                    } else {
                        text[text.len() - len..].to_string()
                    }
                }
                LengthUnits::Characters => {
                    let chars: Vec<char> = text.chars().collect();
                    if chars.len() <= len {
                        text.to_string()
                    } else {
                        chars[chars.len() - len..].iter().collect()
                    }
                }
                LengthUnits::Bits => text.to_string(),
            };
            Ok(out)
        }
    }
}

pub(crate) fn text_justification_for_kind(
    props: &IrProps,
    kind: crate::ir::ValueKind,
) -> TextStringJustification {
    use crate::ir::ValueKind::*;
    let numeric = matches!(
        kind,
        Int | Integer
            | Long
            | Short
            | Byte
            | UnsignedInt
            | UnsignedShort
            | UnsignedByte
            | Float
            | Double
            | Decimal
    );
    if numeric {
        match props.text_number_justification {
            TextNumberJustification::Left => TextStringJustification::Left,
            TextNumberJustification::Right => TextStringJustification::Right,
            TextNumberJustification::Center => TextStringJustification::Center,
        }
    } else {
        props.text_string_justification
    }
}

pub(crate) fn pad_char_for_kind(
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
) -> alloc::string::String {
    use crate::ir::ValueKind::*;
    use crate::schema::expand_entities_str;
    if matches!(kind, String) {
        if let Some(id) = props.text_string_pad_character {
            if let Ok(raw) = strings.get(id) {
                return expand_entities_str(raw);
            }
        }
    }
    if matches!(
        kind,
        crate::ir::ValueKind::DateTime | crate::ir::ValueKind::Time
    ) {
        if let Some(id) = props.text_calendar_pad_character {
            if let Ok(raw) = strings.get(id) {
                return expand_entities_str(raw);
            }
        }
    }
    if matches!(kind, Boolean) {
        if let Some(id) = props.text_boolean_pad_character {
            if let Ok(raw) = strings.get(id) {
                return expand_entities_str(raw);
            }
        }
    }
    let ch = super::pad_trim::pad_char_from_props(props, strings)
        .and_then(|p| p.chars().next())
        .unwrap_or(' ');
    ch.to_string()
}
