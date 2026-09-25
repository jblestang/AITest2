use super::binary::{read_binary_scalar, write_binary_scalar, write_blob_scalar, write_byte_aligned};
use super::cursor::Cursor;
use super::delimited::*;
use super::encoding_name;
use super::numeric_text_format::write_text_scalar;
use super::numeric_text_parse::{
    parse_int_with_base, parse_text_boolean, parse_unbounded_integer_decimal, parse_unsigned_radix,
    read_text_scalar,
};
use super::pad_trim::{format_delimiter_for_error, value_kind_type_name};
use super::property::{
    encode_framing_delimiter_bytes, resolve_encode_property_pattern, resolve_output_new_line_for_encode,
};
use super::RuntimeConfig;
use crate::ir::{IrProps, StringPool, ValueKind};
use crate::length_validate::DaffodilTunables;
use crate::schema::{encode_property_delimiter, LengthKind, LengthUnits, Representation};
use crate::value::DfdlValue;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub(crate) fn parse_float(s: &str) -> Result<f64, crate::error::VmError> {
    match s.trim() {
        "INF" | "Inf" | "+INF" | "+Inf" | "Infinity" | "+Infinity" => Ok(f64::INFINITY),
        "-INF" | "-Inf" | "-Infinity" => Ok(f64::NEG_INFINITY),
        "NaN" => Ok(f64::NAN),
        _ => s.parse().map_err(|_| crate::error::VmError::InvalidValue {
            message: alloc::format!("invalid float `{s}`"),
        }),
    }
}

pub(crate) fn decode_hex_binary(s: &str) -> Result<Vec<u8>, crate::error::VmError> {
    decode_hex(s)
}

pub(crate) fn decode_hex(s: &str) -> Result<Vec<u8>, crate::error::VmError> {
    if !s.len().is_multiple_of(2) {
        return Err(crate::error::VmError::InvalidValue {
            message: "invalid hexBinary".into(),
        });
    }
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi_ch = chunk[0] as char;
        let hi = hi_ch
            .to_digit(16)
            .ok_or_else(|| crate::error::VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error: Hex character must be 0-9, a-f, or A-F, but was '{hi_ch}'"
                ),
            })?;
        let lo_ch = chunk[1] as char;
        let lo = lo_ch
            .to_digit(16)
            .ok_or_else(|| crate::error::VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error: Hex character must be 0-9, a-f, or A-F, but was '{lo_ch}'"
                ),
            })?;
        out.push((hi << 4 | lo) as u8);
    }
    Ok(out)
}

pub(crate) fn encode_hex(bytes: &[u8]) -> alloc::string::String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = alloc::string::String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub(crate) fn parse_xs_boolean_lexical(trimmed: &str) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    match trimmed {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(VmError::InvalidValue {
            message: format!("invalid boolean lexical `{trimmed}`"),
        }),
    }
}

pub(crate) fn finalize_simple_value(
    value: DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
    enable_facet_validation: bool,
    defer_facet_validation: bool,
) -> Result<DfdlValue, crate::error::VmError> {
    crate::vm::facet_validate::validate_assert_int_eq(&value, props)?;
    if crate::vm::facet_validate::needs_facet_validation(props)
        && enable_facet_validation
        && !defer_facet_validation
    {
        crate::vm::facet_validate::validate_decoded_facets(&value, kind, props, strings, tunables)?;
    }
    Ok(value)
}

pub(crate) fn read_simple(
    cursor: &mut Cursor<'_>,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
    field_name: Option<&str>,
    tunables: &DaffodilTunables,
    consume_delimited_enclosing: bool,
    mut delim_out: Option<&mut crate::value::FieldDelimiterMeta>,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
    enable_facet_validation: bool,
    defer_facet_validation: bool,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<DfdlValue, crate::error::VmError> {
    use crate::error::VmError;

    let encoding = encoding_name(props, strings)?;
    if let Some(id) = props.initiator {
        let pat = stop_delimiter_literal(id, strings, scan_ctx)?;
        if !pat.is_empty() {
            crate::vm::alignment::align_cursor_to_text_encoding(cursor, props, encoding)?;
            let Some((_n, alt)) =
                cursor.consume_delimiter_with_alt(&pat, props.ignore_case, Some(encoding))
            else {
                let found_display = super::pad_trim::format_found_at_cursor(cursor.data, cursor.pos, Some(encoding));
                let ctx = field_name.unwrap_or("element");
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error: Init('{pat}') not found. Initiator '{pat}' not found. {ctx}: Delimiter not found!\nWas looking for ({pat}) but found \"{found_display}\" instead"
                    ),
                });
            };
            if let Some(out) = delim_out.as_mut() {
                out.initiator_alt = Some(alt);
            }
        }
    }
    let _ = field_name;
    let require_enclosing = require_delimiter || has_non_empty_terminator(props, strings)?;
    let use_text = match kind {
        ValueKind::String => props.object_kind != crate::schema::ObjectKind::Bytes,
        ValueKind::HexBinary => false,
        _ => props.representation == Representation::Text,
    };
    let value = if use_text {
        read_text_scalar(
            cursor,
            kind,
            props,
            strings,
            require_enclosing,
            stop_sequences,
            field_name,
            sibling_env,
            tunables,
            scan_ctx,
        )?
    } else {
        read_binary_scalar(
            cursor,
            kind,
            props,
            strings,
            require_enclosing,
            stop_sequences,
            field_name,
            tunables,
            scan_ctx,
        )?
    };
    if props.length_kind == LengthKind::Delimited {
        if field_terminator_starts_at_cursor(cursor, props, strings, scan_ctx)? {
            consume_enclosing_delimiter(cursor, props, strings, stop_sequences, scan_ctx)?;
        } else {
            let skip_parent_infix_consume =
                scan_ctx.is_some_and(|ctx| ctx.parent_infix_consumed_by_occurrence_loop);
            if !skip_parent_infix_consume {
                let defer = !consume_delimited_enclosing
                    && defer_delimited_enclosing_consume(
                        cursor,
                        props,
                        strings,
                        &value,
                        stop_sequences,
                        scan_ctx,
                    )?;
                if consume_delimited_enclosing || !defer {
                    consume_enclosing_delimiter(cursor, props, strings, stop_sequences, scan_ctx)?;
                }
            }
        }
    } else if props.representation == Representation::Text
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
    {
        // Terminator consumed in read_text_scalar for fixed/explicit text fields.
    } else if let Some(id) = props.terminator {
        let pat = stop_delimiter_literal(id, strings, scan_ctx)?;
        if !pat.is_empty() {
            crate::vm::alignment::align_cursor_to_text_encoding(cursor, props, encoding)?;
            if let Some((n, alt)) =
                cursor.consume_delimiter_with_alt(&pat, props.ignore_case, Some(encoding))
            {
                if n == 0
                    && !cursor.is_empty()
                    && !crate::schema::delimiter_alt_allows_trailing_input(&pat, alt)
                {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "terminator mismatch: expected `{}`",
                            format_delimiter_for_error(&pat)
                        ),
                    });
                }
                if let Some(out) = delim_out.as_mut() {
                    out.terminator_alt = Some(alt);
                }
            } else if !cursor.is_empty() {
                return Err(VmError::InvalidValue {
                    message: super::pad_trim::format_terminator_not_found_error(&pat),
                });
            }
        }
    }
    super::framed_payload::consume_element_trailing_framing(cursor, props)?;
    finalize_simple_value(
        value,
        kind,
        props,
        strings,
        tunables,
        enable_facet_validation,
        defer_facet_validation,
    )
}

pub(crate) fn coerce_value_for_kind(
    value: &DfdlValue,
    kind: ValueKind,
) -> Result<DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use ValueKind::*;

    Ok(match (kind, value) {
        (Boolean, v @ DfdlValue::Boolean(_)) => v.clone(),
        (Byte, DfdlValue::Int(v)) => {
            DfdlValue::Byte(i8::try_from(*v).map_err(|_| VmError::InvalidValue {
                message: format!("value `{v}` out of range for byte"),
            })?)
        }
        (Byte, v @ DfdlValue::Byte(_)) => v.clone(),
        (Byte, v @ DfdlValue::Long(_)) => v.clone(),
        (UnsignedByte, DfdlValue::Int(v)) => {
            DfdlValue::UnsignedByte(u8::try_from(*v).map_err(|_| VmError::InvalidValue {
                message: format!("value `{v}` out of range for unsignedByte"),
            })?)
        }
        (UnsignedByte, v @ DfdlValue::UnsignedByte(_)) => v.clone(),
        (Short, DfdlValue::Int(v)) => DfdlValue::Short(*v as i16),
        (Short, v @ DfdlValue::Short(_)) => v.clone(),
        (UnsignedShort, DfdlValue::Int(v)) => {
            DfdlValue::UnsignedShort(u16::try_from(*v).map_err(|_| VmError::InvalidValue {
                message: format!("value `{v}` out of range for unsignedShort"),
            })?)
        }
        (UnsignedShort, v @ DfdlValue::UnsignedShort(_)) => v.clone(),
        (Byte, DfdlValue::Integer(s)) => {
            DfdlValue::Byte(s.parse::<i8>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for byte"),
            })?)
        }
        (UnsignedByte, DfdlValue::Integer(s)) => {
            DfdlValue::UnsignedByte(s.parse::<u8>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for unsignedByte"),
            })?)
        }
        (Short, DfdlValue::Integer(s)) => {
            DfdlValue::Short(s.parse::<i16>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for short"),
            })?)
        }
        (UnsignedShort, DfdlValue::Integer(s)) => {
            DfdlValue::UnsignedShort(s.parse::<u16>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for unsignedShort"),
            })?)
        }
        (Int, DfdlValue::Integer(s)) => {
            DfdlValue::Int(s.parse::<i32>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for int"),
            })?)
        }
        (UnsignedInt, DfdlValue::Integer(s)) => {
            DfdlValue::UnsignedInt(s.parse::<u32>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for unsignedInt"),
            })?)
        }
        (Long, DfdlValue::Integer(s)) => {
            DfdlValue::Long(s.parse::<i64>().map_err(|_| VmError::InvalidValue {
                message: format!("value `{s}` out of range for long"),
            })?)
        }
        (Int, v @ DfdlValue::Int(_)) => v.clone(),
        (UnsignedInt, DfdlValue::Int(v)) => {
            DfdlValue::UnsignedInt(u32::try_from(*v).map_err(|_| VmError::InvalidValue {
                message: format!("value `{v}` out of range for unsignedInt"),
            })?)
        }
        (UnsignedInt, v @ DfdlValue::UnsignedInt(_)) => v.clone(),
        (Long, DfdlValue::Int(v)) => DfdlValue::Long(*v as i64),
        (Long, v @ DfdlValue::Long(_)) => v.clone(),
        (HexBinary, DfdlValue::String(s)) => {
            decode_hex_binary(&s.text).map(DfdlValue::HexBinary)?
        }
        (HexBinary, v @ DfdlValue::HexBinary(_)) => v.clone(),
        (Time, v @ DfdlValue::DateTime(_)) => v.clone(),
        (_, v) => v.clone(),
    })
}

fn unparse_not_valid_xs(type_name: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: format!("Unparse Error: not a valid {type_name}"),
    }
}

fn unparse_not_calendar() -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: "Unparse Error: not a calendar".into(),
    }
}

pub(crate) fn validate_unparse_scalar_lexical(
    value: &DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;

    let base = props.text_standard_base;
    let type_name = value_kind_type_name(kind, Some(props));

    match (kind, value) {
        (ValueKind::Byte, DfdlValue::String(s)) => {
            parse_int_with_base::<i8>(&s.text, type_name, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Short, DfdlValue::String(s)) => {
            parse_int_with_base::<i16>(&s.text, type_name, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Int, DfdlValue::String(s)) => {
            parse_int_with_base::<i32>(&s.text, type_name, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Long, DfdlValue::String(s)) => {
            parse_int_with_base::<i64>(&s.text, type_name, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Int, DfdlValue::Long(v)) => {
            i32::try_from(*v).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::UnsignedByte, DfdlValue::String(s)) => {
            parse_unsigned_radix::<u8>(&s.text, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::UnsignedShort, DfdlValue::String(s)) => {
            parse_unsigned_radix::<u16>(&s.text, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::UnsignedInt, DfdlValue::String(s)) => {
            parse_unsigned_radix::<u32>(&s.text, base)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Integer, val @ (DfdlValue::Integer(_) | DfdlValue::String(_))) => {
            let text = match val {
                DfdlValue::Integer(s) => s.as_str(),
                DfdlValue::String(s) => s.text.as_str(),
                _ => return Err(unparse_not_valid_xs(type_name)),
            };
            if props.non_negative_integer && text.trim().starts_with('-') {
                return Err(VmError::InvalidValue {
                    message: format!(
                        "Unparse Error: value `{text}` out of range for type xs:nonNegativeInteger"
                    ),
                });
            }
            parse_unbounded_integer_decimal(text, base, props.non_negative_integer).map_err(
                |e| {
                    if let VmError::InvalidValue { ref message } = e {
                        if message.contains("out of range") {
                            return e;
                        }
                    }
                    unparse_not_valid_xs(type_name)
                },
            )?;
        }
        (ValueKind::Float, DfdlValue::String(s)) => {
            parse_float(&s.text).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Double, DfdlValue::String(s)) => {
            parse_float(&s.text).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Decimal, val @ (DfdlValue::Decimal(_) | DfdlValue::String(_))) => {
            let text = match val {
                DfdlValue::Decimal(s) => s.as_str(),
                DfdlValue::String(s) => s.text.as_str(),
                _ => return Err(unparse_not_valid_xs(type_name)),
            };
            if text.trim().is_empty() {
                return Err(unparse_not_valid_xs(type_name));
            }
            if props.non_negative_integer && text.trim().starts_with('-') {
                return Err(unparse_not_valid_xs(type_name));
            }
            let trimmed = text.trim();
            if trimmed.contains('E') || trimmed.contains('e') {
                crate::vm::text_number_format::decimal_lexical_valid_for_unparse(trimmed)
                    .map_err(|_| unparse_not_valid_xs(type_name))?;
            } else if text.chars().any(|c| c.is_ascii_alphabetic()) {
                return Err(unparse_not_valid_xs(type_name));
            }
        }
        (ValueKind::HexBinary, val @ (DfdlValue::HexBinary(_) | DfdlValue::String(_))) => {
            let bytes = match val {
                DfdlValue::HexBinary(b) => b.clone(),
                DfdlValue::String(s) => {
                    let t = s.text.trim();
                    if t.len() % 2 != 0 {
                        return Err(VmError::InvalidValue {
                            message: format!(
                                "Unparse Error: Hex string must have an even number of characters, but was {} for {t}",
                                t.len()
                            ),
                        });
                    }
                    decode_hex_binary(t).map_err(|_| unparse_not_valid_xs("xs:hexBinary"))?
                }
                _ => return Err(unparse_not_valid_xs("xs:hexBinary")),
            };
            if props.length_kind == LengthKind::Explicit {
                if let Some(len) = props.length {
                    let len_bits = match props.length_units {
                        LengthUnits::Bytes => len.saturating_mul(8),
                        LengthUnits::Bits => len,
                        LengthUnits::Characters => len.saturating_mul(8),
                    };
                    let value_bits = (bytes.len() as u64).saturating_mul(8);
                    if value_bits > len_bits {
                        return Err(VmError::InvalidValue {
                            message: format!(
                                "Unparse Error: Length of xs:hexBinary exceeds calculated length of {value_bits} bits: {len_bits}"
                            ),
                        });
                    }
                }
            }
        }
        (ValueKind::Boolean, DfdlValue::String(s)) => {
            parse_text_boolean(&s.text, props, strings, None)
                .map_err(|_| unparse_not_valid_xs("xs:boolean"))?;
        }
        (ValueKind::DateTime, DfdlValue::DateTime(s))
        | (ValueKind::Time, DfdlValue::DateTime(s)) => {
            if props.calendar_date_only {
                crate::vm::calendar_binary::parse_xs_calendar_lexical(kind, true, s)
                    .map_err(|_| unparse_not_calendar())?;
            } else {
                crate::vm::calendar_binary::parse_xs_calendar_lexical(kind, false, s).map_err(
                    |_| {
                        unparse_not_valid_xs(if kind == ValueKind::Time {
                            "xs:time"
                        } else {
                            "xs:dateTime"
                        })
                    },
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn hex_binary_from_integer(value: i64, width: Option<usize>) -> Vec<u8> {
    if let Some(w) = width {
        if w == 0 {
            return Vec::new();
        }
        if value >= 0 {
            let mut v = value as u64;
            let mut out = vec![0u8; w];
            for i in (0..w).rev() {
                out[i] = (v & 0xff) as u8;
                v >>= 8;
            }
            return out;
        }
        let bits = (w as u32).saturating_mul(8);
        let mask = if bits >= 128 {
            u128::MAX
        } else {
            (1u128 << bits) - 1
        };
        let twos = (value as i128 as u128) & mask;
        let mut out = vec![0u8; w];
        for i in 0..w {
            let shift = ((w - 1 - i) * 8) as u32;
            out[i] = ((twos >> shift) & 0xff) as u8;
        }
        return out;
    }
    if value == 0 {
        return vec![0];
    }
    if value > 0 {
        let mut v = value as u64;
        let mut bytes = Vec::new();
        while v > 0 {
            bytes.push((v & 0xff) as u8);
            v >>= 8;
        }
        bytes.reverse();
        return bytes;
    }
    for w in 1..=8usize {
        let out = hex_binary_from_integer(value, Some(w));
        let sign = out[0] & 0x80 != 0;
        if sign {
            return out;
        }
    }
    hex_binary_from_integer(value, Some(8))
}

pub(crate) fn int_bytes(value: i64, size: usize, le: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size);
    let mut v = value as u64;
    if le {
        for _ in 0..size {
            bytes.push((v & 0xff) as u8);
            v >>= 8;
        }
    } else {
        for i in (0..size).rev() {
            bytes.push(((v >> (i * 8)) & 0xff) as u8);
        }
    }
    bytes
}

pub(crate) fn write_simple(
    out: &mut Vec<u8>,
    bit_count: &mut u8,
    value: &DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
    config: &RuntimeConfig,
    field_name: Option<&str>,
    delim_meta: Option<&crate::value::FieldDelimiterMeta>,
    encode_siblings: Option<&alloc::collections::BTreeMap<String, DfdlValue>>,
    encode_escape_parent: Option<&IrProps>,
) -> Result<(), crate::error::VmError> {
    validate_unparse_scalar_lexical(value, kind, props, strings)?;
    let value = coerce_value_for_kind(value, kind)?;
    if props.object_kind == crate::schema::ObjectKind::Bytes {
        if let Some(id) = props.initiator {
            let pat = strings.get(id)?;
            if !pat.is_empty() {
                let output_nl = props.output_new_line.and_then(|id| strings.get(id).ok());
                let bytes = encode_property_delimiter(pat, output_nl);
                write_byte_aligned(out, bit_count, &bytes)?;
            }
        }
        write_blob_scalar(out, bit_count, &value, props, field_name)?;
        if let Some(id) = props.terminator {
            let pat = strings.get(id)?;
            if !pat.is_empty() {
                let output_nl = props.output_new_line.and_then(|id| strings.get(id).ok());
                let bytes = encode_property_delimiter(pat, output_nl);
                write_byte_aligned(out, bit_count, &bytes)?;
            }
        }
        crate::vm::alignment::write_trailing_skip(out, bit_count, props)?;
        return Ok(());
    }
    let encoding = encoding_name(props, strings)?;
    let output_nl = resolve_output_new_line_for_encode(props, encode_siblings, strings)?
        .map(|s| s as alloc::string::String);
    let output_nl_ref = output_nl.as_deref();
    if let Some(id) = props.initiator {
        let raw = strings.get(id)?;
        let pat = resolve_encode_property_pattern(raw, encode_siblings);
        if !pat.is_empty() {
            let bytes = encode_framing_delimiter_bytes(
                &pat,
                output_nl_ref,
                encoding,
                delim_meta.and_then(|m| m.initiator_alt),
            );
            write_byte_aligned(out, bit_count, &bytes)?;
        }
    }
    match props.representation {
        Representation::Binary => write_binary_scalar(
            out, bit_count, &value, kind, props, strings, tunables, config, field_name,
        )?,
        Representation::Text => write_text_scalar(
            out,
            bit_count,
            &value,
            kind,
            props,
            strings,
            config,
            field_name,
            encode_siblings,
            encode_escape_parent,
        )?,
    }
    if let Some(id) = props.terminator {
        let raw = strings.get(id)?;
        let pat = resolve_encode_property_pattern(raw, encode_siblings);
        if !pat.is_empty() {
            let bytes = encode_framing_delimiter_bytes(
                &pat,
                output_nl_ref,
                encoding,
                delim_meta.and_then(|m| m.terminator_alt),
            );
            write_byte_aligned(out, bit_count, &bytes)?;
        }
    }
    crate::vm::alignment::write_trailing_skip(out, bit_count, props)?;
    Ok(())
}
