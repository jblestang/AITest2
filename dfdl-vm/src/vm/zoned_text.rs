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
    AsciiStandard,
    AsciiTranslatedEBCDIC,
    AsciiCARealiaModified,
    AsciiTandemModified,
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
