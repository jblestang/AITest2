use crate::ir::IrProps;
use crate::schema::{TextPadKind, TextStringJustification, TextTrimKind};
use alloc::string::{String, ToString};

/// Visible glyph for delimiter-mismatch errors (DFDL control-picture convention).
pub(crate) fn remap_control_or_line_ending_to_visible(c: char) -> char {
    let n = c as u32;
    match n {
        n if n <= 0x1f => char::from_u32(n + 0x2400).unwrap_or(c),
        0x20 => '\u{2423}',
        0x7f => '\u{2421}',
        _ => c,
    }
}

pub(crate) fn format_found_at_cursor(
    data: &[u8],
    pos: usize,
    encoding: Option<&str>,
) -> alloc::string::String {
    if pos >= data.len() {
        return String::new();
    }
    let enc = encoding
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_else(|| alloc::string::String::from("UTF-8"));
    if enc.contains("UTF-16") || enc.contains("UTF16") {
        let le = enc.contains("LE");
        let (hi, lo) = if le { (1, 0) } else { (0, 1) };
        if pos + 1 < data.len() {
            let code = u32::from(data[pos + hi]) << 8 | u32::from(data[pos + lo]);
            if let Some(ch) = char::from_u32(code) {
                return remap_control_or_line_ending_to_visible(ch).to_string();
            }
        }
    }
    let b = data[pos];
    if b.is_ascii() && !b.is_ascii_control() {
        return (b as char).to_string();
    }
    remap_control_or_line_ending_to_visible(b as char).to_string()
}

pub(crate) fn format_delimiter_for_error(pat: &str) -> alloc::string::String {
    pat.replace('\n', "%NL;").replace('\r', "%CR;")
}

pub(crate) fn format_terminator_not_found_error(pat: &str) -> alloc::string::String {
    alloc::format!(
        "Parse Error. Terminator '{}' not found",
        format_delimiter_for_error(pat)
    )
}

pub(crate) fn pad_char_from_props<'a>(props: &IrProps, strings: &'a crate::ir::StringPool) -> Option<&'a str> {
    props
        .text_number_pad_character
        .and_then(|id| strings.get(id).ok())
}

pub(crate) fn trim_numeric_text<'a>(input: &'a str, kind: TextTrimKind, pad: Option<&str>) -> &'a str {
    match kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim => input.trim(),
        TextTrimKind::Left => input.trim_start(),
        TextTrimKind::Right => input.trim_end(),
        TextTrimKind::PadChar => trim_pad_char(input, pad.unwrap_or(" ")),
    }
}

pub(crate) fn trim_string<'a>(input: &'a str, trim_kind: TextTrimKind, pad: Option<&str>) -> &'a str {
    match trim_kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim => input.trim(),
        TextTrimKind::Left => input.trim_start(),
        TextTrimKind::Right => input.trim_end(),
        TextTrimKind::PadChar => trim_pad_char(input, pad.unwrap_or(" ")),
    }
}

pub(crate) fn trim_pad_char<'a>(input: &'a str, pad: &str) -> &'a str {
    trim_pad_char_for_justification(input, pad, TextStringJustification::Center)
}

pub(crate) fn trim_pad_char_for_justification<'a>(
    input: &'a str,
    pad: &str,
    justification: TextStringJustification,
) -> &'a str {
    if pad.is_empty() {
        return input;
    }
    let alt_pad = crate::vm::encoding::remap_pua_to_xml_illegal_characters(pad);
    let pua_pad = crate::vm::encoding::remap_xml_illegal_characters_to_pua(pad);
    let has_alt = alt_pad != pad;
    let has_pua = pua_pad != pad;
    let mut start = 0usize;
    let mut end = input.len();
    match justification {
        TextStringJustification::Left | TextStringJustification::Center => {
            while end > start {
                let current = &input[..end];
                if current.ends_with(pad) {
                    end -= pad.len();
                } else if has_alt && current.ends_with(&alt_pad) {
                    end -= alt_pad.len();
                } else if has_pua && current.ends_with(&pua_pad) {
                    end -= pua_pad.len();
                } else {
                    break;
                }
            }
        }
        TextStringJustification::Right => {}
    }
    match justification {
        TextStringJustification::Right | TextStringJustification::Center => {
            while start < end {
                let current = &input[start..end];
                if current.starts_with(pad) {
                    start += pad.len();
                } else if has_alt && current.starts_with(&alt_pad) {
                    start += alt_pad.len();
                } else if has_pua && current.starts_with(&pua_pad) {
                    start += pua_pad.len();
                } else {
                    break;
                }
            }
        }
        TextStringJustification::Left => {}
    }
    &input[start..end]
}

pub(crate) fn apply_text_pad_trim<'a>(
    input: &'a str,
    props: &IrProps,
    pad: Option<&'a str>,
) -> &'a str {
    if props.text_pad_kind == TextPadKind::None && props.text_trim_kind == TextTrimKind::None {
        return input;
    }
    trim_string(input, props.text_trim_kind, pad)
}

pub(crate) fn parse_out_of_range(type_name: &str, decimal_value: &str) -> crate::error::VmError {
    let alt_name = if type_name.ends_with("nonNegativeInteger") {
        "NonNegativeInteger"
    } else {
        type_name
    };
    crate::error::VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. Cannot convert '{decimal_value}' to {type_name} ({alt_name}). Out of Range. out of range. {type_name} {decimal_value}"
        ),
    }
}

pub(crate) fn value_kind_type_name(kind: crate::ir::ValueKind, props: Option<&IrProps>) -> &'static str {
    use crate::ir::ValueKind;
    match kind {
        ValueKind::Byte => "xs:byte",
        ValueKind::Short => "xs:short",
        ValueKind::Int => {
            if props.is_some_and(|p| p.non_negative_integer) {
                "xs:nonNegativeInteger"
            } else {
                "xs:int"
            }
        }
        ValueKind::Long => {
            if props.is_some_and(|p| p.non_negative_integer) {
                "xs:nonNegativeInteger"
            } else if props.is_some_and(|p| p.unsigned_integer) {
                "xs:unsignedLong"
            } else {
                "xs:long"
            }
        }
        ValueKind::UnsignedByte => "xs:unsignedByte",
        ValueKind::UnsignedShort => "xs:unsignedShort",
        ValueKind::UnsignedInt => "xs:unsignedInt",
        ValueKind::Integer => {
            if props.is_some_and(|p| p.non_negative_integer || p.unsigned_integer) {
                "xs:nonNegativeInteger"
            } else {
                "xs:integer"
            }
        }
        ValueKind::Float => "xs:float",
        ValueKind::Double => "xs:double",
        ValueKind::Decimal => "xs:decimal",
        ValueKind::Boolean => "xs:boolean",
        ValueKind::String => "xs:string",
        ValueKind::HexBinary => "xs:hexBinary",
        ValueKind::DateTime => "xs:dateTime",
        ValueKind::Time => "xs:time",
        ValueKind::Complex => "xs:complexType",
    }
}
