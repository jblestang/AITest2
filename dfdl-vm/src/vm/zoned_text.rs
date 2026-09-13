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
    if let Some(idx) = EBCDIC_B0_B9.chars().position(|c| c == digit) {
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
    if digit.is_ascii_digit() {
        return Ok((digit as u8 - b'0', false));
    }
    if (' '..=')').contains(&digit) {
        return Ok((digit as u8 - b' ', true));
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("Invalid zoned digit: {digit}"),
    })
}

fn convert_from_ascii_tandem_modified(digit: char) -> Result<(u8, bool), VmError> {
    if digit.is_ascii_digit() {
        return Ok((digit as u8 - b'0', false));
    }
    let cp = digit as u32;
    if (128..=137).contains(&cp) {
        return Ok(((cp - 128) as u8, true));
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("Invalid zoned digit: {digit}"),
    })
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

fn zoned_v_pattern_matches(pattern: &str) -> bool {
    let p = pattern.trim();
    let mut chars = p.chars().peekable();
    let mut prefix_plus = false;
    let mut suffix_plus = false;
    if chars.peek() == Some(&'+') {
        prefix_plus = true;
        chars.next();
    }
    let mut before_v = 0usize;
    while let Some(c) = chars.next() {
        if c == 'V' {
            break;
        }
        if !c.is_ascii_digit() {
            return false;
        }
        before_v += 1;
    }
    if before_v == 0 {
        return false;
    }
    let mut after_v = 0usize;
    while let Some(c) = chars.next() {
        if c == '+' {
            suffix_plus = true;
            break;
        }
        if !c.is_ascii_digit() {
            return false;
        }
        after_v += 1;
    }
    if after_v == 0 {
        return false;
    }
    if prefix_plus && suffix_plus {
        return false;
    }
    chars.peek().is_none()
}

pub(crate) fn validate_zoned_v_in_pattern(pattern: &str) -> Result<(), VmError> {
    let bare = strip_zoned_plus_markers(pattern);
    if !bare.contains('V') {
        return Ok(());
    }
    if zoned_v_pattern_matches(&bare) {
        return Ok(());
    }
    Err(VmError::InvalidValue {
        message: alloc::format!(
            "Schema Definition Error: The dfdl:textNumberPattern '{pattern}' contains 'V' (virtual decimal point). Other than the leading or trailing '+' sign indicator, it can contain only digits 0-9."
        ),
    })
}

pub(crate) fn validate_zoned_text_number_pattern_runtime(
    pattern: &str,
    kind: crate::ir::ValueKind,
    check_policy: crate::schema::BinaryNumberCheckPolicy,
) -> Result<(), VmError> {
    use crate::ir::ValueKind;
    use crate::schema::BinaryNumberCheckPolicy;
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
    validate_zoned_v_in_pattern(pattern)?;
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
        let require_plus = match kind {
            ValueKind::UnsignedByte | ValueKind::UnsignedShort | ValueKind::UnsignedInt =>
                check_policy == BinaryNumberCheckPolicy::Lax,
            _ => true,
        };
        if require_plus {
            let msg = if matches!(
                kind,
                ValueKind::UnsignedByte | ValueKind::UnsignedShort | ValueKind::UnsignedInt
            ) {
                "Schema Definition Error: textNumberPattern must have '+' at the beginning or the end of the pattern when textNumberRep='zoned' and textNumberPolicy='lax' for unsigned numbers"
            } else {
                "Schema Definition Error: textNumberPattern must have '+' at the beginning or the end of the pattern when textNumberRep='zoned' for signed numbers"
            };
            return Err(VmError::InvalidValue {
                message: msg.into(),
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebcdic_section_sign_is_negative_five() {
        let section = char::from_u32(0x00A7).unwrap();
        let (d, neg) = convert_from_zoned_ebcdic(section).unwrap();
        assert!(neg);
        assert_eq!(d, 5);
    }
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
    let mut chars: Vec<char> = raw.chars().collect();
    if chars.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Unable to parse zoned number from empty string".into(),
        });
    }
    let opindex = match opl {
        OverpunchLocation::Start => 0,
        OverpunchLocation::End => chars.len() - 1,
        OverpunchLocation::None => return Ok(raw.to_string()),
    };
    let ch = chars.get(opindex).copied().ok_or_else(|| VmError::InvalidValue {
        message: "Invalid zoned overpunch index".into(),
    })?;
    let (digit, negative) = decode_overpunch(ch, style)?;
    chars[opindex] = char::from(b'0' + digit);
    let all_digits: String = chars.into_iter().collect();
    Ok(if negative {
        alloc::format!("-{all_digits}")
    } else {
        all_digits
    })
}
