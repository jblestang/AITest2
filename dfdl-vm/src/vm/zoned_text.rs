//! Zoned decimal text (`textNumberRep="zoned"`) overpunch decoding.

use crate::error::VmError;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverpunchLocation {
    Start,
    End,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextZonedSignStyle {
    /// Default when `textZonedSignStyle` is unset and encoding is EBCDIC.
    Ebcdic,
    AsciiStandard,
    AsciiTranslatedEBCDIC,
    AsciiCARealiaModified,
    AsciiTandemModified,
}

const EBCDIC_B0_B9: &str = "^£¥·©§¶¼½¾";

fn convert_from_zoned_ebcdic(digit: char) -> Result<(u8, bool), VmError> {
    if digit.is_ascii_digit() {
        return Ok((digit as u8 - b'0', false));
    }
    if digit == '{' {
        return Ok((0, false));
    }
    if ('A'..='I').contains(&digit) {
        return Ok((digit as u8 - b'A' + 1, false));
    }
    if digit == '}' {
        return Ok((0, true));
    }
    if ('J'..='R').contains(&digit) {
        return Ok((digit as u8 - b'J' + 1, true));
    }
    if let Some(idx) = EBCDIC_B0_B9.find(digit) {
        return Ok((idx as u8, true));
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("Invalid zoned digit: {digit}"),
    })
}

fn convert_from_ascii_standard(digit: char) -> Result<(u8, bool), VmError> {
    if digit.is_ascii_digit() {
        return Ok((digit as u8 - b'0', false));
    }
    if ('p'..='y').contains(&digit) {
        return Ok((digit as u8 - b'p', true));
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("Invalid zoned digit: {digit}"),
    })
}

fn convert_from_ascii_translated_ebcdic(digit: char) -> Result<(u8, bool), VmError> {
    match digit {
        '{' => Ok((0, false)),
        '}' => Ok((0, true)),
        'A'..='I' => Ok((digit as u8 - b'A' + 1, false)),
        'J'..='R' => Ok((digit as u8 - b'J' + 1, true)),
        '0'..='9' => Ok((digit as u8 - b'0', false)),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("Invalid zoned digit: {digit}"),
        }),
    }
}

fn convert_from_ascii_ca_realia_modified(digit: char) -> Result<(u8, bool), VmError> {
    match digit {
        '{' => Ok((0, false)),
        '}' => Ok((0, true)),
        'A'..='I' => Ok((digit as u8 - b'A' + 1, false)),
        'J'..='R' => Ok((digit as u8 - b'J' + 1, true)),
        '0'..='9' => Ok((digit as u8 - b'0', false)),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("Invalid zoned digit: {digit}"),
        }),
    }
}

fn convert_from_ascii_tandem_modified(digit: char) -> Result<(u8, bool), VmError> {
    match digit {
        'p'..='y' => Ok((digit as u8 - b'p', true)),
        'P'..='Y' => Ok((digit as u8 - b'P', false)),
        '0'..='9' => Ok((digit as u8 - b'0', false)),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("Invalid zoned digit: {digit}"),
        }),
    }
}

fn decode_overpunch(
    ch: char,
    style: TextZonedSignStyle,
) -> Result<(u8, bool), VmError> {
    match style {
        TextZonedSignStyle::Ebcdic => convert_from_zoned_ebcdic(ch),
        TextZonedSignStyle::AsciiStandard => convert_from_ascii_standard(ch),
        TextZonedSignStyle::AsciiTranslatedEBCDIC => convert_from_ascii_translated_ebcdic(ch),
        TextZonedSignStyle::AsciiCARealiaModified => convert_from_ascii_ca_realia_modified(ch),
        TextZonedSignStyle::AsciiTandemModified => convert_from_ascii_tandem_modified(ch),
    }
}

/// Map `+` in a zoned pattern to overpunch location (Daffodil `PrimitivesZoned`).
pub(crate) fn overpunch_location_from_pattern(pattern: &str) -> OverpunchLocation {
    let trimmed = pattern.trim();
    if trimmed.starts_with('+') {
        OverpunchLocation::Start
    } else if trimmed.ends_with('+') {
        OverpunchLocation::End
    } else {
        OverpunchLocation::None
    }
}

pub(crate) fn strip_zoned_plus_markers(pattern: &str) -> String {
    pattern.replace('+', "")
}

pub(crate) fn validate_zoned_text_number_pattern_runtime(
    pattern: &str,
    kind: crate::ir::ValueKind,
) -> Result<(), VmError> {
    use crate::ir::ValueKind;
    let positive = pattern.split(';').next().unwrap_or(pattern);
    if positive.contains('@') {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: The '@' symbol may not be used in textNumberPattern for textNumberRep='zoned'".into(),
        });
    }
    if positive.contains('E') {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: The 'E' symbol may not be used in textNumberPattern for textNumberRep='zoned'".into(),
        });
    }
    if pattern.contains(';') {
        let (_, neg) = pattern.split_once(';').unwrap_or((pattern, ""));
        if !neg.is_empty() {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Negative patterns may not be used in textNumberPattern for textNumberRep='zoned'".into(),
            });
        }
    }
    let stripped = strip_zoned_plus_markers(pattern);
    let has_leading = pattern.starts_with('+');
    let has_trailing = pattern.ends_with('+') || stripped.ends_with('+');
    if has_leading && has_trailing {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: The textNumberPattern may either begin or end with a '+', not both.".into(),
        });
    }
    if matches!(kind, ValueKind::Float | ValueKind::Double) {
        return Ok(());
    }
    if !has_leading && !has_trailing {
        let msg = if matches!(
            kind,
            ValueKind::UnsignedByte | ValueKind::UnsignedShort | ValueKind::UnsignedInt
        ) {
            "Schema Definition Error: textNumberPattern must have '+' at the beginning or the end of the pattern when textNumberRep='zoned' for unsigned numbers"
        } else {
            "Schema Definition Error: textNumberPattern must have '+' at the beginning or the end of the pattern when textNumberRep='zoned' for signed numbers"
        };
        return Err(VmError::InvalidValue {
            message: msg.into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_zoned_pattern_characters(pattern: &str) -> Result<(), VmError> {
    let bare = strip_zoned_plus_markers(pattern);
    let positive = bare.split(';').next().unwrap_or(bare.as_str());
    for c in positive.chars() {
        if c.is_ascii_digit() || c == 'V' || c == 'v' {
            continue;
        }
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: textNumberPattern `{pattern}` must contain only digits 0-9"
            ),
        });
    }
    Ok(())
}

pub(crate) fn zoned_to_number(
    raw: &str,
    style: TextZonedSignStyle,
    opl: OverpunchLocation,
) -> Result<String, VmError> {
    if raw.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Unable to parse zoned number from empty string".into(),
        });
    }
    let opindex = match opl {
        OverpunchLocation::Start => 0,
        OverpunchLocation::End => raw.len().saturating_sub(1),
        OverpunchLocation::None => return Ok(raw.to_string()),
    };
    let ch = raw
        .chars()
        .nth(opindex)
        .ok_or_else(|| VmError::InvalidValue {
            message: "Invalid zoned overpunch index".into(),
        })?;
    let (digit, negative) = decode_overpunch(ch, style)?;
    let mut chars: Vec<char> = raw.chars().collect();
    chars[opindex] = char::from(b'0' + digit);
    let all_digits: String = chars.into_iter().collect();
    Ok(if negative {
        alloc::format!("-{all_digits}")
    } else {
        all_digits
    })
}
