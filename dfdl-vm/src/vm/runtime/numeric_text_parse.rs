use super::numeric_text_format::{pad_char_for_kind, text_justification_for_kind};
use super::calendar_text::*;
use super::cursor::Cursor;
use super::delimited::*;
use super::encoding_name;
use super::framed_payload::{read_length_span, read_prefixed_payload};
use super::nil::{
    match_nil_literal_prefix, nil_value_includes_empty, text_matches_nil_literal,
    validate_nil_value_runtime,
};
use super::pad_trim::*;
use super::property::default_value_for;
use super::scalar::*;
use crate::ir::{IrProps, StringId, StringPool, ValueKind};
use crate::length_validate::DaffodilTunables;
use crate::schema::*;
use crate::value::DfdlValue;
use crate::vm::encoding::{
    bits_charset_spec, decode_bits_charset_payload, decode_text_bytes, hex_charset_order,
    hex_charset_payload_to_text, normalize_encoding_name, read_one_utf8_char,
    remap_xml_illegal_characters_to_pua,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn read_implicit_numeric_text(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> alloc::vec::Vec<u8> {
    if props.custom_text_number_pattern {
        if let Some(pat_id) = props.text_number_pattern {
            if let Ok(raw_pattern) = strings.get(pat_id) {
                let pattern_owned = if props.text_number_rep == crate::schema::TextNumberRep::Zoned
                {
                    crate::vm::zoned_text::strip_zoned_plus_markers(raw_pattern)
                } else {
                    raw_pattern.to_string()
                };
                let pattern = pattern_owned.as_str();
                if text_number_pattern_needs_full_span(pattern) {
                    let (dec_seps, grouping, exponent, pad, check_policy) =
                        resolved_text_number_format_parts(props, strings);
                    let fmt = crate::vm::text_number::TextNumberFormatProps {
                        check_policy,
                        decimal_separators: &dec_seps,
                        grouping_separator: grouping.as_deref(),
                        exponent_chars: &exponent,
                        pad_character: pad,
                        ignore_case: props.ignore_case,
                    };
                    let available = cursor.data.len() - cursor.pos;
                    if let Some(mut len) = crate::vm::text_number::implicit_text_number_byte_length(
                        &cursor.data[cursor.pos..],
                        pattern,
                        &fmt,
                    ) {
                        if pattern.contains('*') && len < available {
                            len = available;
                        }
                        let start = cursor.pos;
                        cursor.pos += len;
                        return cursor.data[start..cursor.pos].to_vec();
                    } else if pattern.contains('*') && available > 0 {
                        let at_nil = match_nil_literal_prefix(cursor, props, strings)
                            .ok()
                            .flatten()
                            .is_some();
                        if !at_nil {
                            let start = cursor.pos;
                            cursor.pos += available;
                            return cursor.data[start..cursor.pos].to_vec();
                        }
                    }
                }
            }
        }
    }
    read_numeric_token(cursor)
}

pub(crate) fn type_size(kind: ValueKind) -> usize {
    use ValueKind::*;
    match kind {
        Boolean => 1,
        Byte | UnsignedByte => 1,
        Short | UnsignedShort => 2,
        Int | UnsignedInt | Float => 4,
        Long | Double => 8,
        String | HexBinary | Decimal | DateTime | Time | Complex | Integer => 0,
    }
}

pub(crate) fn implicit_binary_scalar_byte_length(kind: ValueKind, props: &IrProps) -> usize {
    if kind == ValueKind::DateTime {
        match props.binary_calendar_rep {
            BinaryNumberRep::BinarySeconds => return 4,
            BinaryNumberRep::BinaryMilliseconds => return 8,
            _ => {}
        }
    }
    if kind == ValueKind::Boolean && props.representation == Representation::Binary {
        return 4;
    }
    type_size(kind)
}

pub(crate) fn pattern_str(strings: &StringPool, id: StringId) -> Result<&str, crate::error::VmError> {
    strings.get(id)
}

pub(crate) fn packed_sign_codes(
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::vm::packed_decimal::PackedSignCodes, crate::error::VmError> {
    let spec = strings.get(props.binary_packed_sign_codes)?;
    crate::vm::packed_decimal::PackedSignCodes::parse(spec, props.binary_number_check_policy)
}

fn binary_payload_to_u64(
    rep: BinaryNumberRep,
    bytes: &[u8],
    le: bool,
) -> Result<u64, crate::error::VmError> {
    match rep {
        BinaryNumberRep::Binary => Ok(decode_unsigned_binary_bytes(bytes, le)),
        BinaryNumberRep::Bcd => {
            crate::vm::packed_decimal::digits_to_u64(&crate::vm::packed_decimal::bcd_to_digit_string(bytes, le)?)
        }
        BinaryNumberRep::Ibm4690Packed => {
            let (_neg, digits) = crate::vm::packed_decimal::ibm4690_to_digit_string(bytes, le)?;
            crate::vm::packed_decimal::digits_to_u64(&digits)
        }
        BinaryNumberRep::PackedBcd => Err(crate::error::VmError::InvalidValue {
            message: "packed decimal requires sign codes".into(),
        }),
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds are calendar encodings".into(),
            })
        }
    }
}

fn decode_unsigned_binary_bytes(bytes: &[u8], le: bool) -> u64 {
    let mut value = 0u64;
    if le {
        for (i, byte) in bytes.iter().enumerate() {
            value |= (*byte as u64) << (i * 8);
        }
    } else {
        for byte in bytes {
            value = (value << 8) | (*byte as u64);
        }
    }
    value
}

fn format_binary_decimal_magnitude(abs: u64, virtual_point: i32) -> String {
    let s = abs.to_string();
    if virtual_point == 0 {
        return s;
    }
    if virtual_point > 0 {
        let p = virtual_point as usize;
        if s.len() > p {
            let idx = s.len() - p;
            alloc::format!("{}.{}", &s[..idx], &s[idx..])
        } else {
            let zeros = "0".repeat(p - s.len());
            alloc::format!("0.{zeros}{s}")
        }
    } else {
        let zeros = "0".repeat((-virtual_point) as usize);
        alloc::format!("{s}{zeros}")
    }
}

pub(crate) fn signed_magnitude_to_dfdl(
    negative: bool,
    digits: &str,
    kind: ValueKind,
    virtual_point: i32,
) -> Result<DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use ValueKind::*;

    let abs = crate::vm::packed_decimal::digits_to_u64(digits)?;
    if kind == Decimal {
        let body = format_binary_decimal_magnitude(abs, virtual_point);
        let text = if negative {
            format!("-{body}")
        } else {
            body
        };
        return Ok(DfdlValue::Decimal(text));
    }
    if negative {
        match kind {
            UnsignedByte | UnsignedShort | UnsignedInt => {
                return Err(VmError::InvalidValue {
                    message: "out of range for type".into(),
                });
            }
            _ => {}
        }
    }
    let signed_i64 = if negative { -(abs as i64) } else { abs as i64 };
    macro_rules! signed {
        ($t:ty, $cons:expr) => {{
            <$t>::try_from(signed_i64)
                .map($cons)
                .map_err(|_| VmError::InvalidValue {
                    message: "out of range for type".into(),
                })
        }};
    }
    match kind {
        Byte => signed!(i8, DfdlValue::Byte),
        Short => signed!(i16, DfdlValue::Short),
        Int => signed!(i32, DfdlValue::Int),
        Long => signed!(i64, DfdlValue::Long),
        Integer => Ok(DfdlValue::Integer(signed_i64.to_string())),
        UnsignedByte => Ok(DfdlValue::UnsignedByte(signed_i64 as u8)),
        UnsignedShort => Ok(DfdlValue::UnsignedShort(signed_i64 as u16)),
        UnsignedInt => Ok(DfdlValue::UnsignedInt(signed_i64 as u32)),
        Float => Ok(DfdlValue::Float(signed_i64 as f32)),
        Double => Ok(DfdlValue::Double(signed_i64 as f64)),
        _ => Err(VmError::UnsupportedOperation {
            op: "signed magnitude binary for type".into(),
        }),
    }
}

pub(crate) fn text_standard_infinity_nan_match(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Option<&'static str> {
    let inf_str = strings
        .get(props.text_standard_infinity_rep)
        .unwrap_or("Inf");
    let inf = if inf_str.is_empty() { "Inf" } else { inf_str };
    let nan_str = strings.get(props.text_standard_nan_rep).unwrap_or("NaN");
    let nan = if nan_str.is_empty() { "NaN" } else { nan_str };
    let ic = props.ignore_case;
    let eq = |a: &str, b: &str| {
        if ic {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if eq(trimmed, nan) || trimmed.eq_ignore_ascii_case("NaN") {
        return Some("NaN");
    }
    if eq(trimmed, inf)
        || trimmed.eq_ignore_ascii_case("INF")
        || trimmed.eq_ignore_ascii_case("Infinity")
    {
        return Some("INF");
    }
    if trimmed.starts_with('-') {
        let rest = &trimmed[1..];
        if eq(rest, inf)
            || rest.eq_ignore_ascii_case("INF")
            || rest.eq_ignore_ascii_case("Infinity")
        {
            return Some("-INF");
        }
    }
    if trimmed.starts_with('+') {
        let rest = &trimmed[1..];
        if eq(rest, inf)
            || rest.eq_ignore_ascii_case("INF")
            || rest.eq_ignore_ascii_case("Infinity")
        {
            return Some("INF");
        }
    }
    None
}

fn text_standard_zero_rep_match(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Option<alloc::string::String> {
    if !props.text_standard_zero_rep_defined {
        return None;
    }
    let raw = strings.get(props.text_standard_zero_rep).ok()?;
    let reps = crate::schema::parse_text_standard_zero_rep_list(raw);
    for rep in reps {
        let matched = if props.ignore_case {
            trimmed.eq_ignore_ascii_case(&rep)
        } else {
            trimmed == rep
        };
        if matched {
            return Some("0".into());
        }
    }
    None
}

fn match_text_number_subpattern(
    text: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let mut ti = 0usize;
    let mut digits = alloc::string::String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid textNumberPattern `{pattern}`"),
                });
            }
            let lit: alloc::string::String = chars[start..i].iter().collect();
            i += 1;
            if !text[ti..].starts_with(&lit) {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            ti += lit.len();
        } else if matches!(chars[i], '0' | '#') {
            let Some(ch) = text.as_bytes().get(ti) else {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            };
            if !ch.is_ascii_digit() {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            digits.push(*ch as char);
            ti += 1;
            i += 1;
        } else {
            i += 1;
        }
    }
    if ti != text.len() {
        return Err(VmError::InvalidValue {
            message: "textNumberPattern mismatch".into(),
        });
    }
    Ok(digits)
}

fn apply_text_number_sign_pattern(
    text: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let mut alts = pattern.split(';');
    if let Some(pos) = alts.next() {
        if let Ok(digits) = match_text_number_subpattern(text, pos) {
            return Ok(digits);
        }
    }
    if let Some(neg) = alts.next() {
        if let Ok(digits) = match_text_number_subpattern(text, neg) {
            return Ok(alloc::format!("-{digits}"));
        }
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("Parse Error. xs:int {text}"),
    })
}

fn apply_text_number_pattern_numeric(
    text: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    if pattern.contains(';') && pattern.contains('\'') {
        return apply_text_number_sign_pattern(text, pattern);
    }
    if !pattern.contains('V') {
        return Ok(text.into());
    }
    let mut parts = pattern.split('V');
    let before = parts.next().unwrap_or("");
    let after = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(VmError::InvalidValue {
            message: alloc::format!("unsupported textNumberPattern `{pattern}`"),
        });
    }
    let digit_before = before.chars().filter(|c| matches!(c, '0' | '#')).count();
    let digit_after = after.chars().filter(|c| matches!(c, '0' | '#')).count();
    let (negative, body) = if text.starts_with('-') {
        (true, &text[1..])
    } else {
        (false, text)
    };
    if body.chars().any(|c| !c.is_ascii_digit()) {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Parse Error. Unable to parse xs:decimal from text: {text}"),
        });
    }
    let total_digits = digit_before + digit_after;
    let mut body_owned = body.to_string();
    if body_owned.len() < total_digits {
        let pad = total_digits - body_owned.len();
        body_owned = alloc::format!("{}{body_owned}", "0".repeat(pad));
    } else if body_owned.len() > total_digits {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "textNumberPattern `{pattern}` expected {} digits, got `{text}`",
                total_digits
            ),
        });
    }
    let body = body_owned.as_str();
    let mut out = alloc::format!("{}.{}", &body[..digit_before], &body[digit_before..]);
    if negative {
        out.insert(0, '-');
    }
    Ok(out)
}

fn validate_standard_v_pattern_runtime(
    pattern: &str,
    kind: crate::ir::ValueKind,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    if !pattern.contains('V') {
        return Ok(());
    }
    if !matches!(
        kind,
        ValueKind::Float
            | ValueKind::Double
            | ValueKind::Decimal
            | ValueKind::Int
            | ValueKind::Integer
            | ValueKind::Long
            | ValueKind::Short
            | ValueKind::Byte
            | ValueKind::UnsignedInt
            | ValueKind::UnsignedShort
            | ValueKind::UnsignedByte
    ) {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: pattern with 'V' not supported for kind {kind:?}"
            ),
        });
    }
    Ok(())
}

fn parse_field_text_number(
    trimmed: &str,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    if !props.custom_text_number_pattern && props.text_standard_base != 10 {
        return Ok(trimmed.into());
    }
    if let Some(zero) = text_standard_zero_rep_match(trimmed, props, strings) {
        return Ok(zero);
    }
    if matches!(kind, ValueKind::Float | ValueKind::Double) {
        if let Some(inf_nan) = text_standard_infinity_nan_match(trimmed, props, strings) {
            return Ok(inf_nan.into());
        }
    }
    let Some(pat_id) = props.text_number_pattern else {
        return Ok(trimmed.into());
    };
    let raw_pattern = strings.get(pat_id)?;
    if matches!(kind, ValueKind::Float | ValueKind::Double)
        && (trimmed.contains('e') || trimmed.contains('E'))
        && !raw_pattern.contains('E')
        && !raw_pattern.contains('e')
    {
        return Ok(trimmed.into());
    }
    if matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && matches!(
            kind,
            ValueKind::Int
                | ValueKind::Integer
                | ValueKind::Long
                | ValueKind::Short
                | ValueKind::Byte
                | ValueKind::UnsignedInt
                | ValueKind::UnsignedShort
                | ValueKind::UnsignedByte
        )
        && !raw_pattern.contains('V')
        && !raw_pattern.contains('.')
        && !raw_pattern.contains('E')
        && !raw_pattern.contains('e')
        && trimmed.contains(':')
    {
        return Err(unable_parse_from_text(
            type_name_for_parse(kind, props),
            trimmed,
        ));
    }
    if props.text_number_rep == crate::schema::TextNumberRep::Standard {
        validate_standard_v_pattern_runtime(raw_pattern, kind)?;
        if raw_pattern.contains('V')
            && trimmed.contains('.')
            && !trimmed.contains('E')
            && !trimmed.contains('e')
        {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. Unable to parse xs:decimal from text: {trimmed}"
                ),
            });
        }
    }
    if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
        if matches!(kind, ValueKind::Float | ValueKind::Double) {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: textNumberRep=\"zoned\" cannot be used with xs:{}",
                    if kind == ValueKind::Float {
                        "float"
                    } else {
                        "double"
                    }
                ),
            });
        }
        crate::vm::zoned_text::validate_zoned_text_number_pattern_runtime(
            raw_pattern,
            kind,
            props.text_number_check_policy,
        )?;
        crate::vm::zoned_text::validate_zoned_pattern_characters(raw_pattern)?;
    }
    let pattern_owned = if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
        crate::vm::zoned_text::strip_zoned_plus_markers(raw_pattern)
    } else {
        raw_pattern.to_string()
    };
    let pattern = pattern_owned.as_str();
    let mut text_to_parse = trimmed.to_string();
    if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
        use crate::schema::TextZonedSignStyle as ZStyle;
        use crate::vm::zoned_text::{
            overpunch_location_from_pattern, zoned_to_number, OverpunchLocation,
            TextZonedSignStyle as VmStyle,
        };
        let enc = strings
            .get(props.encoding)
            .unwrap_or("")
            .to_ascii_lowercase();
        let ebcdic = enc.contains("ebcdic");
        let vm_style = match props.text_zoned_sign_style {
            None if ebcdic => VmStyle::Ebcdic,
            None | Some(ZStyle::AsciiStandard) => VmStyle::AsciiStandard,
            Some(ZStyle::AsciiTranslatedEBCDIC) => VmStyle::AsciiTranslatedEBCDIC,
            Some(ZStyle::AsciiCARealiaModified) => VmStyle::AsciiCARealiaModified,
            Some(ZStyle::AsciiTandemModified) => VmStyle::AsciiTandemModified,
        };
        let opl = overpunch_location_from_pattern(raw_pattern);
        if opl != OverpunchLocation::None {
            text_to_parse = zoned_to_number(trimmed, vm_style, opl).map_err(|e| {
                let VmError::InvalidValue { message: detail } = e else {
                    return unable_parse_from_text(type_name_for_parse(kind, props), trimmed);
                };
                VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. Unable to parse zoned {} from text: {trimmed}. {detail}",
                        type_name_for_parse(kind, props)
                    ),
                }
            })?;
        }
    }
    if pattern.contains('V')
        && !pattern.contains('E')
        && !pattern.contains('e')
        && !pattern.contains('\'')
        && !pattern.contains(';')
    {
        return apply_text_number_pattern_numeric(&text_to_parse, pattern);
    }
    let (dec_seps, grouping, exponent, pad, check_policy) =
        resolved_text_number_format_parts(props, strings);
    let fmt = crate::vm::text_number::TextNumberFormatProps {
        check_policy,
        decimal_separators: &dec_seps,
        grouping_separator: grouping.as_deref(),
        exponent_chars: &exponent,
        pad_character: pad,
        ignore_case: props.ignore_case,
    };
    let parse_result = if text_to_parse.starts_with('-') {
        crate::vm::text_number::parse_standard_text_number(&text_to_parse[1..], pattern, &fmt).map(|inner| {
            if inner.starts_with('-') {
                inner
            } else {
                alloc::format!("-{inner}")
            }
        })
    } else {
        crate::vm::text_number::parse_standard_text_number(&text_to_parse, pattern, &fmt)
    };
    if let Ok(v) = parse_result {
        if props.text_standard_zero_rep_defined {
            let raw = strings.get(props.text_standard_zero_rep).unwrap_or("");
            if crate::schema::parse_text_standard_zero_rep_list(raw).is_empty()
                && !trimmed.is_empty()
                && trimmed.chars().all(|c| c.is_ascii_alphabetic())
            {
                return Err(unable_parse_from_text(
                    type_name_for_parse(kind, props),
                    trimmed,
                ));
            }
        }
        if matches!(
            kind,
            crate::ir::ValueKind::Int
                | crate::ir::ValueKind::Integer
                | crate::ir::ValueKind::Long
                | crate::ir::ValueKind::Short
                | crate::ir::ValueKind::Byte
                | crate::ir::ValueKind::UnsignedInt
                | crate::ir::ValueKind::UnsignedShort
                | crate::ir::ValueKind::UnsignedByte
        ) {
            return normalize_text_number_for_integer(
                &v,
                trimmed,
                type_name_for_parse(kind, props),
            );
        }
        return Ok(v);
    }
    if let Some(zero) = text_standard_zero_rep_match(trimmed, props, strings) {
        return Ok(zero);
    }
    Err(unable_parse_from_text(
        type_name_for_parse(kind, props),
        trimmed,
    ))
}

fn normalize_text_number_for_integer(
    parsed: &str,
    raw_input: &str,
    type_name: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    if let Some((int_part, frac)) = parsed.split_once('.') {
        if frac.chars().all(|c| c == '0') {
            return Ok(int_part.to_string());
        }
        return Err(unable_parse_from_text(type_name, raw_input));
    }
    if parsed.contains('E') || parsed.contains('e') {
        return Err(unable_parse_from_text(type_name, raw_input));
    }
    Ok(parsed.to_string())
}

fn type_name_for_parse(kind: crate::ir::ValueKind, props: &IrProps) -> &'static str {
    value_kind_type_name(kind, Some(props))
}

pub(crate) fn reject_text_standard_special_for_integer(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
    type_name: &str,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let Some(special) = text_standard_infinity_nan_match(trimmed, props, strings) else {
        return Ok(());
    };
    let detail = if special == "NaN" { "NaN" } else { "Infinity" };
    Err(VmError::InvalidValue {
        message: format!("Parse Error. {detail} value out of range for type {type_name}"),
    })
}

fn text_number_for_parse(
    trimmed: &str,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<String, crate::error::VmError> {
    if !matches!(
        kind,
        ValueKind::Float | ValueKind::Double | ValueKind::Decimal
    ) {
        return Ok(trimmed.into());
    }
    if let Some(special) = text_standard_infinity_nan_match(trimmed, props, strings) {
        return Ok(special.into());
    }
    parse_field_text_number(trimmed, kind, props, strings)
}

pub(crate) fn parse_int_with_base<T>(s: &str, type_name: &str, base: u32) -> Result<T, crate::error::VmError>
where
    T: TryFrom<i64>,
    <T as TryFrom<i64>>::Error: core::fmt::Debug,
{
    parse_int_typed_with_base(s, type_name, base, s)
}

pub(crate) fn parse_int_typed_with_base<T>(
    s: &str,
    type_name: &str,
    base: u32,
    field_text: &str,
) -> Result<T, crate::error::VmError>
where
    T: TryFrom<i64>,
    <T as TryFrom<i64>>::Error: core::fmt::Debug,
{
    let v = parse_int_typed_with_base_i64(s, type_name, base, field_text)?;
    T::try_from(v).map_err(|_| {
        let (sign, digits) = split_sign_digits(s).unwrap_or((1, ""));
        let abs = parse_u128_radix_base10(digits, base, type_name, field_text).unwrap_or(0);
        parse_out_of_range(type_name, &decimal_from_sign_magnitude(sign, abs))
    })
}

fn parse_int_typed_with_base_i64(
    s: &str,
    type_name: &str,
    base: u32,
    field_text: &str,
) -> Result<i64, crate::error::VmError> {
    if base != 10 {
        let val = parse_non_base10_signed_string(s, type_name, base)?;
        return val.parse::<i64>().map_err(|_| parse_out_of_range(type_name, &val));
    }
    let (sign, digits) = split_sign_digits(s)?;
    let abs = parse_u128_radix_base10(digits, base, type_name, field_text)?;
    let decimal = decimal_from_sign_magnitude(sign, abs);
    let (min_abs, max_abs): (u128, u128) = match type_name {
        "xs:byte" => (128, 127),
        "xs:short" => (32768, 32767),
        "xs:int" => (2147483648, 2147483647),
        "xs:long" => (9223372036854775808, 9223372036854775807),
        _ => (0, u128::MAX),
    };
    if sign >= 0 && abs > max_abs {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    if sign < 0 && abs > min_abs {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    if sign < 0 {
        let signed = -(abs as i128);
        if signed < i64::MIN as i128 || signed > i64::MAX as i128 {
            return Err(parse_out_of_range(type_name, &decimal));
        }
        Ok(signed as i64)
    } else {
        if abs > i64::MAX as u128 {
            return Err(parse_out_of_range(type_name, &decimal));
        }
        Ok(abs as i64)
    }
}

pub(crate) fn parse_unsigned_radix<T>(s: &str, base: u32) -> Result<T, crate::error::VmError>
where
    T: TryFrom<u64>,
    <T as TryFrom<u64>>::Error: core::fmt::Debug,
{
    let type_name = match core::mem::size_of::<T>() {
        1 => "xs:unsignedByte",
        2 => "xs:unsignedShort",
        4 => "xs:unsignedInt",
        8 => "xs:unsignedLong",
        _ => "xs:unsignedInt",
    };
    let v = parse_unsigned_radix_typed(s, type_name, base, s)?;
    T::try_from(v).map_err(|_| parse_out_of_range(type_name, s))
}

pub(crate) fn parse_unsigned_radix_typed(
    s: &str,
    type_name: &str,
    base: u32,
    field_text: &str,
) -> Result<u64, crate::error::VmError> {
    let trimmed = s.trim();
    if trimmed.starts_with('-') {
        return Err(parse_out_of_range(type_name, trimmed));
    }
    if trimmed.starts_with('+') {
        return Err(if base == 10 {
            unable_parse_from_text(type_name, field_text)
        } else {
            crate::error::VmError::InvalidValue {
                message: format!("invalid integer `{s}`"),
            }
        });
    }
    let abs = parse_u128_radix_base10(trimmed, base, type_name, field_text)?;
    let decimal = abs.to_string();
    let max = match type_name {
        "xs:unsignedByte" => u8::MAX as u128,
        "xs:unsignedShort" => u16::MAX as u128,
        "xs:unsignedInt" => u32::MAX as u128,
        "xs:unsignedLong" => u64::MAX as u128,
        _ => u128::MAX,
    };
    if abs > max {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    u64::try_from(abs).map_err(|_| parse_out_of_range(type_name, &decimal))
}

pub(crate) fn parse_unbounded_integer_decimal(
    s: &str,
    base: u32,
    unsigned_long_range: bool,
) -> Result<String, crate::error::VmError> {
    let type_name = if unsigned_long_range {
        "xs:nonNegativeInteger"
    } else {
        "xs:integer"
    };
    if base != 10 {
        let val = parse_non_base10_signed_string(s, type_name, base)?;
        if unsigned_long_range && val.starts_with('-') {
            return Err(parse_out_of_range(type_name, &val));
        }
        return Ok(val);
    }
    let (sign, digits) = split_sign_digits(s)?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(unable_parse_from_text(type_name, s));
    }
    let trimmed_digits = digits.trim_start_matches('0');
    let norm = if trimmed_digits.is_empty() { "0" } else { trimmed_digits };
    if unsigned_long_range && sign < 0 && norm != "0" {
        return Err(parse_out_of_range(type_name, &alloc::format!("-{norm}")));
    }
    if sign < 0 && norm != "0" {
        Ok(alloc::format!("-{norm}"))
    } else {
        Ok(norm.to_string())
    }
}

fn split_sign_digits(s: &str) -> Result<(i32, &str), crate::error::VmError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok((1, ""));
    }
    if let Some(rest) = trimmed.strip_prefix('+') {
        Ok((1, rest))
    } else if let Some(rest) = trimmed.strip_prefix('-') {
        Ok((-1, rest))
    } else {
        Ok((1, trimmed))
    }
}

fn decimal_from_sign_magnitude(sign: i32, abs: u128) -> String {
    if sign < 0 && abs != 0 {
        format!("-{abs}")
    } else {
        abs.to_string()
    }
}

fn parse_non_base10_signed_string(
    s: &str,
    type_name: &str,
    base: u32,
) -> Result<String, crate::error::VmError> {
    let trimmed = s.trim();
    let (sign, digits) = if let Some(rest) = trimmed.strip_prefix('-') {
        (-1i32, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (1i32, rest)
    } else {
        (1i32, trimmed)
    };
    if digits.is_empty() {
        return Err(unable_parse_from_text(type_name, s));
    }
    let abs = u128::from_str_radix(digits, base).map_err(|_| unable_parse_from_text(type_name, s))?;
    if sign < 0 {
        Ok(alloc::format!("-{abs}"))
    } else {
        Ok(abs.to_string())
    }
}

fn parse_u128_radix_base10(
    digits: &str,
    base: u32,
    type_name: &str,
    field_text: &str,
) -> Result<u128, crate::error::VmError> {
    if base != 10 {
        return u128::from_str_radix(digits, base)
            .map_err(|_| unable_parse_from_text(type_name, field_text));
    }
    if digits.is_empty() {
        return Err(unable_parse_from_text(type_name, field_text));
    }
    let mut clean = digits;
    if let Some((int_part, frac_part)) = digits.split_once('.') {
        if frac_part.chars().all(|c| c == '0') {
            clean = int_part;
        } else {
            return Err(unable_parse_from_text(type_name, field_text));
        }
    }
    if clean.is_empty() || !clean.chars().all(|c| c.is_ascii_digit()) {
        return Err(unable_parse_from_text(type_name, field_text));
    }
    clean
        .parse::<u128>()
        .map_err(|_| parse_out_of_range(type_name, field_text))
}



pub(crate) fn validate_runtime_text_boolean_same_length(
    props: &IrProps,
    true_reps: &[alloc::string::String],
    false_reps: &[alloc::string::String],
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthKind;
    if !matches!(
        props.length_kind,
        LengthKind::Explicit | LengthKind::Implicit
    ) {
        return Ok(());
    }
    if props.text_pad_kind != TextPadKind::None && props.text_trim_kind != TextTrimKind::None {
        return Ok(());
    }
    let true_len = true_reps.first().map(|s| s.chars().count()).unwrap_or(0);
    let false_len = false_reps.first().map(|s| s.chars().count()).unwrap_or(0);
    if true_len != false_len
        || true_reps.iter().any(|r| r.chars().count() != true_len)
        || false_reps.iter().any(|r| r.chars().count() != false_len)
    {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: dfdl:textBooleanTrueRep and dfdl:textBooleanFalseRep must have the same length".into(),
        });
    }
    Ok(())
}

pub(crate) fn parse_text_boolean(
    s: &str,
    props: &IrProps,
    strings: &StringPool,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    use super::numeric_text_format::text_boolean_rep_candidates;
    let trimmed = apply_text_pad_trim(s, props, props.text_string_pad_character.and_then(|id| strings.get(id).ok()));
    let ignore = props.ignore_case;
    let true_reps = text_boolean_rep_candidates(props, strings, true, sibling_env)?;
    let false_reps = text_boolean_rep_candidates(props, strings, false, sibling_env)?;
    validate_runtime_text_boolean_same_length(props, &true_reps, &false_reps)?;
    let matches = |a: &str, b: &str| {
        if ignore {
            if a.eq_ignore_ascii_case(b) {
                return true;
            }
        } else if a == b {
            return true;
        }
        if a.len() < b.len() && b.as_bytes().get(a.len()) == Some(&b' ') {
            let head = &b[..a.len()];
            if ignore {
                if a.eq_ignore_ascii_case(head) {
                    return true;
                }
            } else if a == head {
                return true;
            }
        }
        if a.chars().count() == 1 {
            if let Some(ch) = a.chars().next() {
                for word in b.split_whitespace() {
                    let mut wch = word.chars();
                    let Some(first) = wch.next() else {
                        continue;
                    };
                    if ignore {
                        if first.eq_ignore_ascii_case(&ch) {
                            return true;
                        }
                    } else if first == ch {
                        return true;
                    }
                }
            }
        }
        false
    };
    if true_reps.iter().any(|r| matches(trimmed, r)) {
        return Ok(true);
    }
    if false_reps.iter().any(|r| matches(trimmed, r)) {
        return Ok(false);
    }
    if !true_reps.is_empty() || !false_reps.is_empty() {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Parse Error. Unable to parse xs:boolean from text: {trimmed}"),
        });
    }
    match trimmed {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("invalid boolean `{trimmed}`"),
        }),
    }
}

fn trim_text_value<'a>(
    input: &'a str,
    kind: crate::ir::ValueKind,
    trim_kind: crate::schema::TextTrimKind,
    props: &IrProps,
    strings: &StringPool,
) -> &'a str {
    match trim_kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim => input.trim(),
        TextTrimKind::Left => input.trim_start(),
        TextTrimKind::Right => input.trim_end(),
        TextTrimKind::PadChar => {
            let pad = pad_char_for_kind(props, strings, kind);
            if kind == crate::ir::ValueKind::String {
                trim_pad_char_for_justification(input, &pad, props.text_string_justification)
            } else if matches!(
                kind,
                crate::ir::ValueKind::DateTime | crate::ir::ValueKind::Time
            ) {
                let just = props
                    .text_calendar_justification
                    .unwrap_or(props.text_string_justification);
                trim_pad_char_for_justification(input, &pad, just)
            } else {
                let just = text_justification_for_kind(props, kind);
                let trimmed = trim_pad_char_for_justification(input, &pad, just);
                if trimmed.is_empty() && !input.is_empty() && pad == "0" {
                    "0"
                } else {
                    trimmed
                }
            }
        }
    }
}












fn pattern_allows_zero_length_match(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    let id = props
        .length_pattern
        .ok_or(crate::error::VmError::InvalidValue {
            message: "pattern length missing lengthPattern".into(),
        })?;
    let pat = pattern_str(strings, id)?;
    if pat == "." && normalize_encoding_name(encoding_name(props, strings)?) == Some("utf-8") {
        return Ok(cursor.pos >= cursor.data.len());
    }
    Ok(match_length_pattern(&cursor.data[cursor.pos..], pat) == Some(0))
}

/// Explicit/fixed length in bytes with UTF-16: Daffodil may decode a lone trailing byte as a character.
fn decode_specified_length_text_bytes(
    raw: &[u8],
    enc: &str,
    props: &IrProps,
) -> Result<alloc::string::String, crate::error::VmError> {
    let utf16 = normalize_encoding_name(enc).is_some_and(|n| n.starts_with("utf-16"));
    let byte_len = matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && props.length_units == LengthUnits::Bytes;
    // Single-byte explicit UTF-16 fields: Daffodil StringOfSpecifiedLength leaves incomplete
    // code units empty, but decodes a lone non-zero byte as that character.
    if utf16 && byte_len && raw.len() == 1 {
        return Ok(if raw[0] == 0 {
            alloc::string::String::new()
        } else {
            alloc::string::String::from(raw[0] as char)
        });
    }
    decode_text_bytes(raw, enc, props.encoding_error_policy)
}








fn unable_parse_from_text(type_name: &str, text: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. Unable to parse / Failed to parse {type_name} from text: {text}"
        ),
    }
}

fn is_numeric_text_kind(kind: crate::ir::ValueKind) -> bool {
    use crate::ir::ValueKind::*;
    matches!(
        kind,
        Byte | UnsignedByte
            | Short
            | UnsignedShort
            | Int
            | Integer
            | UnsignedInt
            | Long
            | Float
            | Double
            | Decimal
    )
}

fn read_numeric_token(cursor: &mut Cursor<'_>) -> Vec<u8> {
    let start = cursor.pos;
    if cursor.pos < cursor.data.len() {
        let b = cursor.data[cursor.pos];
        if b == b'+' || b == b'-' {
            cursor.advance(1);
        }
    }
    while cursor.pos < cursor.data.len() {
        let b = cursor.data[cursor.pos];
        if b.is_ascii_digit() || b == b'.' || b == b',' || b == b'e' || b == b'E' {
            cursor.advance(1);
        } else if (b == b'+' || b == b'-')
            && cursor.pos > start
            && matches!(cursor.data[cursor.pos - 1], b'e' | b'E')
        {
            cursor.advance(1);
        } else {
            break;
        }
    }
    cursor.data[start..cursor.pos].to_vec()
}

fn normalize_string_line_endings(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}


fn delimited_trailing_fraction_at_cursor(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> bool {
    if cursor.data.get(cursor.pos) != Some(&b'.') {
        return false;
    }
    if !cursor
        .data
        .get(cursor.pos.saturating_add(1))
        .is_some_and(|b| b.is_ascii_digit())
    {
        return false;
    }
    if let Some(tid) = props.terminator {
        if let Ok(term) = stop_delimiter_literal(tid, strings, scan_ctx) {
            if crate::schema::match_delimiter_opts(
                &cursor.data[cursor.pos..],
                &term,
                props.ignore_case,
            )
            .is_some()
            {
                return false;
            }
        }
    }
    if let Some(ctx) = scan_ctx {
        if let Some(sep_id) = ctx.parent_sequence.separator {
            if ctx.parent_sequence.separator_position == SeparatorPosition::Infix {
                if let Ok(sep_lit) = stop_delimiter_literal(sep_id, strings, Some(ctx)) {
                    if crate::schema::match_delimiter_opts(
                        &cursor.data[cursor.pos..],
                        &sep_lit,
                        ctx.parent_sequence.ignore_case,
                    )
                    .is_some()
                    {
                        return false;
                    }
                }
            }
        }
    }
    let _ = props;
    true
}

fn is_text_float_infinity_or_nan(s: &str) -> bool {
    matches!(
        s.trim(),
        "INF"
            | "Inf"
            | "+INF"
            | "+Inf"
            | "Infinity"
            | "+Infinity"
            | "-INF"
            | "-Inf"
            | "-Infinity"
            | "NaN"
    )
}



fn unsigned_uses_text_number_pattern(trimmed: &str, props: &IrProps) -> bool {
    props.custom_text_number_pattern
        || props.text_number_pattern.is_some()
        || props.text_number_check_policy == crate::schema::BinaryNumberCheckPolicy::Strict
        || trimmed.contains(',')
}

fn explicit_length_unsigned_short_whitespace(type_name: &str, props: &IrProps) -> bool {
    type_name == "xs:unsignedShort"
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
}

fn lax_numeric_field_text(text: &str, props: &IrProps, type_name: &str) -> alloc::string::String {
    use crate::schema::BinaryNumberCheckPolicy;
    if props.text_standard_base == 10
        && props.text_number_check_policy == BinaryNumberCheckPolicy::Lax
    {
        if explicit_length_unsigned_short_whitespace(type_name, props) {
            if props.text_trim_kind == TextTrimKind::PadChar {
                text.trim().to_string()
            } else {
                text.chars().filter(|c| !c.is_whitespace()).collect()
            }
        } else {
            text.trim().to_string()
        }
    } else {
        text.to_string()
    }
}

fn reject_internal_whitespace_explicit_field(
    text: &str,
    type_name: &str,
    props: &IrProps,
    base: u32,
    trailing_input: bool,
) -> Result<(), crate::error::VmError> {
    use crate::schema::BinaryNumberCheckPolicy;
    if base != 10 {
        return Ok(());
    }
    if !matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        return Ok(());
    }
    if trailing_input && type_name == "xs:short" {
        return Ok(());
    }
    if explicit_length_unsigned_short_whitespace(type_name, props) {
        if props.text_number_check_policy == BinaryNumberCheckPolicy::Lax
            && props.text_trim_kind != TextTrimKind::PadChar
        {
            return Ok(());
        }
        if props.text_number_check_policy == BinaryNumberCheckPolicy::Strict
            && text.chars().any(char::is_whitespace)
        {
            return Err(unable_parse_from_text(type_name, text));
        }
    }
    let t = text.trim();
    if t.chars().any(char::is_whitespace) {
        return Err(unable_parse_from_text(type_name, text));
    }
    Ok(())
}

pub(crate) fn read_text_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
    field_name: Option<&str>,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
    tunables: &DaffodilTunables,
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    validate_nil_value_runtime(props, strings)?;

    if matches!(kind, String) {
        if let Some(id) = props.text_string_pad_character {
            let raw = strings.get(id)?;
            if let Err(msg) = crate::schema::validate_text_string_pad_character_runtime(raw) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Schema Definition Error: {msg}"),
                });
            }
        }
    }

    if props.length_kind == LengthKind::Pattern {
        if let Some(nil_len) = match_nil_literal_prefix(cursor, props, strings)? {
            cursor.advance(nil_len);
            return Ok(DfdlValue::Null);
        }
        if props.empty_element_parse_policy == crate::schema::EmptyElementParsePolicy::TreatAsAbsent
            && pattern_allows_zero_length_match(cursor, props, strings)?
        {
            return Err(crate::error::VmError::ElementAbsent);
        }
        if nil_value_includes_empty(props, strings)?
            && pattern_allows_zero_length_match(cursor, props, strings)?
        {
            return Ok(DfdlValue::Null);
        }
    }

    let enc = encoding_name(props, strings)?;
    let mut raw = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed text missing length".into(),
            })? as usize;
            read_length_span(
                cursor,
                len,
                props.length_units,
                enc,
                props.bit_order,
                props.encoding_error_policy,
                false,
            )?
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit text missing length".into(),
            })? as usize;
            read_length_span(
                cursor,
                len,
                props.length_units,
                enc,
                props.bit_order,
                props.encoding_error_policy,
                false,
            )?
        }
        LengthKind::Delimited => {
            let raw = read_until_delimiters(
                cursor,
                props,
                strings,
                require_delimiter,
                stop_sequences,
                Some(enc),
                scan_ctx,
            )?;
            if has_non_empty_terminator(props, strings)?
                && field_terminator_matches_at_cursor(cursor, props, strings, scan_ctx)?.is_none()
                && !cursor.is_empty()
            {
                let patterns = non_empty_delimiter_scan_patterns(props, strings, scan_ctx)?;
                let pat = patterns.first().map(|p| p.pat.as_str()).unwrap_or("");
                return Err(VmError::InvalidValue {
                    message: format_terminator_not_found_error(pat),
                });
            }
            raw
        }
        LengthKind::Pattern => {
            let id = props.length_pattern.ok_or(VmError::InvalidValue {
                message: "pattern length missing lengthPattern".into(),
            })?;
            let pat = pattern_str(strings, id)?;
            let policy = props.encoding_error_policy;
            let len = if pat == "." && normalize_encoding_name(enc) == Some("utf-8") {
                read_one_utf8_char(cursor.data, cursor.pos, policy)?.1
            } else {
                match_length_pattern(&cursor.data[cursor.pos..], pat).ok_or(
                    VmError::InvalidValue {
                        message: alloc::format!("pattern `{pat}` mismatch"),
                    },
                )?
            };
            cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)?
        }
        LengthKind::Implicit => {
            if is_numeric_text_kind(kind) {
                read_implicit_numeric_text(cursor, props, strings)
            } else if matches!(kind, String | HexBinary) {
                if let Some(len) = crate::vm::facet_validate::implicit_facet_byte_length(props) {
                    read_length_span(
                        cursor,
                        len,
                        props.length_units,
                        enc,
                        props.bit_order,
                        props.encoding_error_policy,
                        false,
                    )?
                } else {
                    read_until_delimiters(
                        cursor,
                        props,
                        strings,
                        false,
                        stop_sequences,
                        Some(enc),
                        scan_ctx,
                    )?
                }
            } else {
                read_until_delimiters(
                    cursor,
                    props,
                    strings,
                    false,
                    stop_sequences,
                    Some(enc),
                    scan_ctx,
                )?
            }
        }
        LengthKind::Prefixed => read_prefixed_payload(cursor, props, strings, field_name)?,
        LengthKind::EndOfParent => {
            let rest = cursor.data[cursor.pos..].to_vec();
            cursor.pos = cursor.data.len();
            rest
        }
    };

    if matches!(kind, DateTime | Time)
        && props.calendar_pattern.is_some()
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && props.length_units == LengthUnits::Bytes
    {
        if let Some(pat_id) = props.calendar_pattern {
            if let Ok(pat) = strings.get(pat_id) {
                if calendar_explicit_pattern_extends_year_beyond_length(pat) {
                    while cursor.pos < cursor.data.len() && cursor.data[cursor.pos].is_ascii_digit()
                    {
                        raw.push(cursor.data[cursor.pos]);
                        cursor.pos += 1;
                    }
                }
            }
        }
    }

    let trailing_input = cursor.pos < cursor.data.len();

    let text = if hex_charset_order(enc).is_some() {
        hex_charset_payload_to_text(&raw)
    } else if let Some(spec) = bits_charset_spec(enc) {
        let n_bits = match props.length_kind {
            LengthKind::Fixed | LengthKind::Explicit if props.length_units == LengthUnits::Bits => {
                props.length.unwrap_or(0) as usize
            }
            _ => raw.len().saturating_mul(8),
        };
        decode_bits_charset_payload(&raw, n_bits, spec)?
    } else {
        decode_specified_length_text_bytes(&raw, enc, props)?
    };
    let text = if kind == crate::ir::ValueKind::String {
        remap_xml_illegal_characters_to_pua(&text)
    } else {
        text
    };
    let text = if kind == crate::ir::ValueKind::String {
        normalize_string_line_endings(&text)
    } else {
        text
    };
    let trimmed = trim_text_value(&text, kind, props.text_trim_kind, props, strings);
    let trimmed = if kind == crate::ir::ValueKind::String {
        let scheme = scan_ctx
            .and_then(|ctx| ctx.resolved_escape_scheme.as_ref())
            .or(props.escape_scheme.as_ref());
        if let Some(scheme) = scheme {
            crate::vm::escape::unescape_field_text(trimmed, scheme)?
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };
    let trimmed = trimmed.as_str();

    if text_matches_nil_literal(trimmed, props, strings)? {
        return Ok(DfdlValue::Null);
    }

    if trimmed.is_empty() {
        if let Some(v) = default_value_for(kind, props, strings) {
            return Ok(v);
        }
        if is_numeric_text_kind(kind)
            && matches!(
                props.length_kind,
                LengthKind::Delimited | LengthKind::Implicit
            )
        {
            if props.occurs_min == 0 {
                return Err(crate::error::VmError::ElementAbsent);
            }
            let type_name = value_kind_type_name(kind, Some(props));
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. Unable to parse {type_name} from empty string"
                ),
            });
        }
    }

    if props.custom_text_number_pattern
        && props.text_number_rep == crate::schema::TextNumberRep::Standard
    {
        if let Some(id) = props.text_number_pattern {
            validate_standard_v_pattern_runtime(strings.get(id)?, kind)?;
        }
    }

    let base = props.text_standard_base;
    let value = match kind {
        Boolean => parse_text_boolean(trimmed, props, strings, sibling_env).map(DfdlValue::Boolean),
        Byte => {
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:byte",
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_int_typed_with_base(&num, "xs:byte", base, trimmed).map(DfdlValue::Byte)
        }
        UnsignedByte => {
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:unsignedByte",
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 && unsigned_uses_text_number_pattern(trimmed, props) {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else if base == 10 {
                lax_numeric_field_text(trimmed, props, "xs:unsignedByte")
            } else {
                trimmed.to_string()
            };
            parse_unsigned_radix_typed(&num, "xs:unsignedByte", base, trimmed).and_then(|v| {
                u8::try_from(v)
                    .map(DfdlValue::UnsignedByte)
                    .map_err(|_| parse_out_of_range("xs:unsignedByte", trimmed))
            })
        }
        Short => {
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:short",
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_int_typed_with_base(&num, "xs:short", base, trimmed).map(DfdlValue::Short)
        }
        UnsignedShort => {
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:unsignedShort",
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 && unsigned_uses_text_number_pattern(trimmed, props) {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else if base == 10 {
                lax_numeric_field_text(trimmed, props, "xs:unsignedShort")
            } else {
                trimmed.to_string()
            };
            parse_unsigned_radix_typed(&num, "xs:unsignedShort", base, trimmed).and_then(|v| {
                u16::try_from(v)
                    .map(DfdlValue::UnsignedShort)
                    .map_err(|_| parse_out_of_range("xs:unsignedShort", trimmed))
            })
        }
        Int => {
            reject_text_standard_special_for_integer(trimmed, props, strings, "xs:int")?;
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:int",
                props,
                base,
                trailing_input,
            )?;
            if base == 10
                && trimmed.contains('.')
                && !trimmed.contains('e')
                && !trimmed.contains('E')
            {
                return Err(unable_parse_from_text("xs:int", trimmed));
            }
            if props.length_kind == LengthKind::Delimited
                && !props.custom_text_number_pattern
                && props.text_number_pattern.is_none()
                && trimmed.chars().any(|c| c.is_ascii_alphabetic() || c == ':')
            {
                return Err(unable_parse_from_text("xs:int", trimmed));
            }
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            let v = parse_int_typed_with_base(&num, "xs:int", base, trimmed).map(DfdlValue::Int)?;
            if props.length_kind == LengthKind::Delimited
                && delimited_trailing_fraction_at_cursor(cursor, props, strings, scan_ctx)
            {
                return Err(unable_parse_from_text("xs:int", trimmed));
            }
            Ok(v)
        }
        Integer => {
            reject_text_standard_special_for_integer(trimmed, props, strings, "xs:integer")?;
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:integer",
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_unbounded_integer_decimal(&num, base, props.non_negative_integer)
                .map(DfdlValue::Integer)
        }
        UnsignedInt => {
            reject_internal_whitespace_explicit_field(
                trimmed,
                "xs:unsignedInt",
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 && unsigned_uses_text_number_pattern(trimmed, props) {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else if base == 10 {
                lax_numeric_field_text(trimmed, props, "xs:unsignedInt")
            } else {
                trimmed.to_string()
            };
            parse_unsigned_radix_typed(&num, "xs:unsignedInt", base, trimmed).and_then(|v| {
                u32::try_from(v)
                    .map(DfdlValue::UnsignedInt)
                    .map_err(|_| parse_out_of_range("xs:unsignedInt", trimmed))
            })
        }
        Long => {
            reject_text_standard_special_for_integer(trimmed, props, strings, "xs:long")?;
            reject_internal_whitespace_explicit_field(
                trimmed,
                if props.unsigned_integer {
                    "xs:unsignedLong"
                } else {
                    "xs:long"
                },
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            if props.unsigned_integer {
                let v = parse_unsigned_radix_typed(&num, "xs:unsignedLong", base, trimmed)?;
                Ok(DfdlValue::UnsignedLong(v))
            } else {
                parse_int_typed_with_base(&num, "xs:long", base, trimmed).map(DfdlValue::Long)
            }
        }
        Float => {
            let (dec_seps, grouping, _, _, _) = resolved_text_number_format_parts(props, strings);
            let colon_allowed = dec_seps.iter().any(|s| s.contains(':'))
                || grouping.as_deref().is_some_and(|g| g.contains(':'));
            if props.length_kind == LengthKind::Delimited
                && !is_text_float_infinity_or_nan(trimmed)
                && ((!colon_allowed && trimmed.contains(':'))
                    || trimmed
                        .chars()
                        .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E'))
            {
                return Err(unable_parse_from_text("xs:float", trimmed));
            }
            let num = text_number_for_parse(trimmed, kind, props, strings)?;
            let v = parse_float(&num)
                .map(|v| DfdlValue::Float(v as f32))
                .map_err(|_| unable_parse_from_text("xs:float", trimmed))?;
            if props.length_kind == LengthKind::Delimited
                && !is_text_float_infinity_or_nan(trimmed)
                && delimited_trailing_fraction_at_cursor(cursor, props, strings, scan_ctx)
            {
                return Err(unable_parse_from_text("xs:float", trimmed));
            }
            Ok(v)
        }
        Double => {
            let (dec_seps, grouping, _, _, _) = resolved_text_number_format_parts(props, strings);
            let colon_allowed = dec_seps.iter().any(|s| s.contains(':'))
                || grouping.as_deref().is_some_and(|g| g.contains(':'));
            if props.length_kind == LengthKind::Delimited
                && !is_text_float_infinity_or_nan(trimmed)
                && ((!colon_allowed && trimmed.contains(':'))
                    || trimmed
                        .chars()
                        .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E'))
            {
                return Err(unable_parse_from_text("xs:double", trimmed));
            }
            let num = text_number_for_parse(trimmed, kind, props, strings)?;
            let v = parse_float(&num)
                .map(DfdlValue::Double)
                .map_err(|_| unable_parse_from_text("xs:double", trimmed))?;
            if props.length_kind == LengthKind::Delimited
                && !is_text_float_infinity_or_nan(trimmed)
                && delimited_trailing_fraction_at_cursor(cursor, props, strings, scan_ctx)
            {
                return Err(unable_parse_from_text("xs:double", trimmed));
            }
            Ok(v)
        }
        Decimal => {
            let num = text_number_for_parse(trimmed, kind, props, strings)?;
            let canon = crate::vm::facet_validate::canonicalize_xs_decimal_lexical(&num);
            Ok(DfdlValue::Decimal(canon))
        }
        DateTime | Time => {
            if props.calendar_pattern_kind == crate::schema::CalendarPatternKind::Implicit {
                let processed = crate::vm::calendar_binary::process_implicit_calendar_text(
                    kind,
                    props.calendar_date_only,
                    props.calendar_check_policy_lax,
                    trimmed,
                    tunables,
                )?;
                let with_tz = append_packed_calendar_timezone(
                    props,
                    strings,
                    kind,
                    props.calendar_date_only,
                    &processed,
                    false,
                    true,
                )?;
                return Ok(DfdlValue::DateTime(with_tz));
            }
            if let Some(pat_id) = props.calendar_pattern {
                let pattern = strings.get(pat_id)?;
                let parsed = if trimmed.chars().all(|c| c.is_ascii_digit()) {
                    format_calendar_pattern(
                        trimmed,
                        pattern,
                        props.calendar_century_start,
                        props.calendar_first_day_of_week,
                    )?
                } else {
                    let cal_lang =
                        resolve_calendar_language(props, strings, sibling_env.map(|e| e.text))?;
                    let parsed_text = format_calendar_text(
                        trimmed,
                        pattern,
                        props.calendar_check_policy_lax,
                        props.calendar_century_start,
                        CalendarTextConfig {
                            language: cal_lang.as_deref(),
                            first_day_of_week: props.calendar_first_day_of_week,
                            days_in_first_week: props.calendar_days_in_first_week,
                        },
                        kind == crate::ir::ValueKind::DateTime,
                        props.calendar_date_only,
                    );
                    if !props.calendar_check_policy_lax {
                        if props.calendar_date_only {
                            parsed_text.map_err(|_| calendar_text_strict_date_error(trimmed))?
                        } else if kind == crate::ir::ValueKind::Time {
                            parsed_text.map_err(|_| calendar_text_strict_time_error(trimmed))?
                        } else {
                            parsed_text.map_err(|_| calendar_text_strict_datetime_error(trimmed))?
                        }
                    } else {
                        parsed_text?
                    }
                };
                use crate::schema::{CalendarPatternKind, Representation};
                let inherit_format_tz = props.representation == Representation::Text
                    && props.calendar_pattern_kind == CalendarPatternKind::Explicit;
                let default_utc = kind == crate::ir::ValueKind::DateTime
                    && parsed.contains('T')
                    && !lexical_has_xsd_timezone(&parsed);
                let with_tz = append_packed_calendar_timezone(
                    props,
                    strings,
                    kind,
                    props.calendar_date_only,
                    &parsed,
                    default_utc,
                    inherit_format_tz,
                )?;
                Ok(DfdlValue::DateTime(with_tz))
            } else {
                let val = crate::vm::calendar_binary::process_implicit_calendar_text(
                    kind,
                    props.calendar_date_only,
                    props.calendar_check_policy_lax,
                    trimmed,
                    tunables,
                )
                .map_err(|_| {
                    unable_parse_from_text(value_kind_type_name(kind, Some(props)), trimmed)
                })?;
                Ok(DfdlValue::DateTime(val))
            }
        }
        String => {
            let sv = if props.encoding_error_policy == crate::schema::EncodingErrorPolicy::Replace
                && trimmed == text
            {
                crate::value::StringValue::with_source_bytes(trimmed, raw.clone())
            } else {
                crate::value::StringValue::new(trimmed)
            };
            Ok(DfdlValue::String(sv))
        }
        HexBinary => decode_hex(trimmed).map(DfdlValue::HexBinary),
        Complex => Err(VmError::TypeMismatch {
            expected: "complex".into(),
        }),
    }?;
    if props.representation == Representation::Text
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
    {
        consume_text_field_terminator_after_fixed_length(cursor, props, strings)?;
    }
        if std::env::var("DEBUG_LION").is_ok() {
        std::eprintln!(
            "READ_TEXT_SCALAR name={:?} kind={:?} len_kind={:?} raw={:?} trimmed={:?} res={:?}",
            field_name,
            kind,
            props.length_kind,
            alloc::string::String::from_utf8_lossy(&raw),
            trimmed,
            value
        );
    }
    Ok(value)
}

pub(crate) fn text_number_pattern_needs_full_span(pattern: &str) -> bool {
    if pattern.contains('*') || pattern.contains(';') || pattern.contains('\'') {
        return true;
    }
    pattern
        .chars()
        .any(|c| c.is_ascii_alphabetic() && c != 'E' && c != 'e' && c != 'V' && c != 'P')
}

pub(crate) fn resolved_text_number_format_parts(
    props: &IrProps,
    strings: &StringPool,
) -> (
    alloc::vec::Vec<alloc::string::String>,
    Option<alloc::string::String>,
    alloc::string::String,
    Option<char>,
    crate::schema::BinaryNumberCheckPolicy,
) {
    let dec_seps = if props.text_standard_decimal_separator_defined {
        if let Ok(raw) = strings.get(props.text_standard_decimal_separator) {
            alloc::vec![raw.to_string()]
        } else {
            alloc::vec![alloc::string::String::from(".")]
        }
    } else {
        alloc::vec![alloc::string::String::from(".")]
    };

    let grouping = props
        .text_standard_grouping_separator
        .and_then(|id| strings.get(id).ok())
        .map(|g| g.to_string());

    let exponent = if props.text_standard_exponent_rep_defined {
        strings
            .get(props.text_standard_exponent_rep)
            .ok()
            .map(|e| e.to_string())
            .unwrap_or_else(|| alloc::string::String::from("E"))
    } else {
        alloc::string::String::from("E")
    };

    let pad = props
        .text_number_pad_character
        .and_then(|id| strings.get(id).ok())
        .and_then(|p| p.chars().next());

    (
        dec_seps,
        grouping,
        exponent,
        pad,
        props.text_number_check_policy,
    )
}
