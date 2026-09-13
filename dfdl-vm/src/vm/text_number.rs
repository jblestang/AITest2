//! Standard text number parsing (`textNumberRep="standard"`) using `textNumberPattern`.
//!
//! Lax/strict behavior follows Daffodil/ICU `DecimalFormat.setParseStrict`.

use crate::error::VmError;
use crate::schema::BinaryNumberCheckPolicy;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub(crate) struct TextNumberFormatProps<'a> {
    pub check_policy: BinaryNumberCheckPolicy,
    pub decimal_separators: &'a [String],
    pub grouping_separator: Option<&'a str>,
    pub exponent_chars: &'a str,
    pub pad_character: Option<char>,
}

impl Default for TextNumberFormatProps<'_> {
    fn default() -> Self {
        Self {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &[],
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        }
    }
}

fn default_decimal_separators() -> Vec<String> {
    vec![".".into()]
}

/// Parse data against a DFDL text number pattern; returns a canonical numeric string for `parse()` / `parse_float`.
fn pattern_without_quoted_regions(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Daffodil `P` (decimal scaling position) virtual point from the positive subpattern.
fn text_decimal_virtual_point_from_pattern(pattern: &str) -> i32 {
    let bare = pattern_without_quoted_regions(pattern);
    if bare.contains('V') {
        return 0;
    }
    if !bare.contains('P') {
        return 0;
    }
    let chars: Vec<char> = bare.chars().collect();
    let mut p_left = 0usize;
    let mut digits_left = 0usize;
    let mut p_right = 0usize;
    let mut phase = 0u8; // 0=prefix, 1=Ps left, 2=digits, 3=Ps right
    for c in chars {
        match phase {
            0 if c == 'P' => {
                phase = 1;
                p_left += 1;
            }
            0 if matches!(c, '0' | '#' | '*' | '.' | ',' | 'E' | 'e' | ' ') => {
                phase = 2;
                if matches!(c, '0' | '#') {
                    digits_left += 1;
                }
            }
            1 if c == 'P' => p_left += 1,
            1 if matches!(c, '0' | '#') => {
                phase = 2;
                digits_left += 1;
            }
            2 if c == 'P' => {
                phase = 3;
                p_right += 1;
            }
            2 if matches!(c, '0' | '#') => digits_left += 1,
            3 if c == 'P' => p_right += 1,
            _ => {}
        }
    }
    if p_left > 0 && p_right == 0 {
        (p_left + digits_left) as i32
    } else if p_right > 0 && p_left == 0 {
        -(p_right as i32)
    } else {
        0
    }
}

fn strip_p_from_pattern(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            out.push('\'');
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if chars[i] != 'P' {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

fn apply_decimal_virtual_point(num: &str, vp: i32) -> String {
    if vp == 0 {
        return num.to_string();
    }
    let negative = num.starts_with('-');
    let body = num.trim_start_matches('-');
    let (mantissa, exp_suffix) = if let Some(idx) = body.find('E').or_else(|| body.find('e')) {
        (&body[..idx], Some(&body[idx..]))
    } else {
        (body, None)
    };
    let (int_part, frac_part) = if let Some(dot) = mantissa.find('.') {
        (&mantissa[..dot], &mantissa[dot + 1..])
    } else {
        (mantissa, "")
    };
    let mut digits: String = int_part.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.extend(frac_part.chars().filter(|c| c.is_ascii_digit()));
    if digits.is_empty() {
        digits.push('0');
    }
    let scaled = if vp > 0 {
        let vp = vp as usize;
        if digits.len() <= vp {
            let zeros = vp - digits.len();
            format!("0.{}{}", "0".repeat(zeros), digits.trim_end_matches('0'))
        } else {
            let split = digits.len() - vp;
            let (a, b) = digits.split_at(split);
            let frac = b.trim_end_matches('0');
            if frac.is_empty() {
                a.to_string()
            } else {
                format!("{a}.{frac}")
            }
        }
    } else {
        let mul = (-vp) as usize;
        let trimmed = digits.trim_start_matches('0');
        let core = if trimmed.is_empty() { "0" } else { trimmed };
        format!("{core}{}", "0".repeat(mul))
    };
    let mut out = if negative {
        format!("-{scaled}")
    } else {
        scaled
    };
    if let Some(exp) = exp_suffix {
        out.push_str(exp);
    }
    out
}

pub(crate) fn parse_standard_text_number(
    input: &str,
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
) -> Result<String, VmError> {
    let lax = props.check_policy == BinaryNumberCheckPolicy::Lax;
    let work = if lax {
        input.trim().to_string()
    } else {
        input.to_string()
    };

    let subpatterns: Vec<&str> = if pattern.contains(';') {
        pattern.split(';').collect()
    } else {
        vec![pattern]
    };

    let positive = subpatterns.first().copied().unwrap_or(pattern);
    let virtual_point = text_decimal_virtual_point_from_pattern(positive);
    let match_pattern = if virtual_point != 0 {
        strip_p_from_pattern(pattern)
    } else {
        pattern.to_string()
    };
    let match_subs: Vec<&str> = if match_pattern.contains(';') {
        match_pattern.split(';').collect()
    } else {
        vec![match_pattern.as_str()]
    };
    let positive_match = match_subs.first().copied().unwrap_or(match_pattern.as_str());

    for (idx, sub) in match_subs.iter().enumerate() {
        let mut scratch = work.clone();
        let result = if idx == 0 {
            match_subpattern(&mut scratch, sub, props, lax, false)
        } else {
            match_negative_subpattern(&mut scratch, sub, positive_match, props, lax)
        };
        match result {
            Ok(num) => {
                if virtual_point != 0 {
                    return Ok(apply_decimal_virtual_point(&num, virtual_point));
                }
                return Ok(num);
            }
            Err(_) => continue,
        }
    }

    Err(VmError::InvalidValue {
        message: format!("Parse Error. xs:int {input}"),
    })
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r'
}

fn skip_ws(bytes: &[u8], pos: &mut usize, lax: bool) {
    if !lax {
        return;
    }
    while *pos < bytes.len() && is_ws(bytes[*pos]) {
        *pos += 1;
    }
}

/// Lax whitespace skip that must not consume a configured decimal separator (e.g. `%SP;`).
fn skip_ws_respecting_decimal_sep(
    bytes: &[u8],
    pos: &mut usize,
    props: &TextNumberFormatProps<'_>,
) {
    while *pos < bytes.len() && is_ws(bytes[*pos]) {
        if props
            .decimal_separators
            .iter()
            .any(|ds| bytes[*pos..].starts_with(ds.as_bytes()))
        {
            break;
        }
        *pos += 1;
    }
}

fn match_decimal_separator(
    bytes: &[u8],
    pos: &mut usize,
    props: &TextNumberFormatProps<'_>,
) -> bool {
    for ds in props.decimal_separators {
        if *pos + ds.len() <= bytes.len() && bytes[*pos..].starts_with(ds.as_bytes()) {
            *pos += ds.len();
            return true;
        }
    }
    if *pos < bytes.len() && bytes[*pos] == b'.' {
        *pos += 1;
        return true;
    }
    false
}

fn is_pattern_digit_slot(c: char) -> bool {
    matches!(c, '0' | '#' | '*' | '.' | ',' | 'E' | 'e' | 'V' | 'P' | ' ')
}

fn negative_affixes(pattern: &str) -> (String, String) {
    let chars: Vec<char> = pattern.chars().collect();
    let mut prefix = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        prefix.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                prefix.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if is_pattern_digit_slot(chars[i]) {
            break;
        }
        prefix.push(chars[i]);
        i += 1;
    }
    let mut suffix = String::new();
    let mut j = chars.len();
    while j > i {
        let c = chars[j - 1];
        if c == '\'' {
            break;
        }
        if is_pattern_digit_slot(c) {
            break;
        }
        suffix.insert(0, c);
        j -= 1;
    }
    (prefix, suffix)
}

fn match_negative_subpattern(
    text: &mut String,
    negative_pattern: &str,
    positive_pattern: &str,
    props: &TextNumberFormatProps<'_>,
    lax: bool,
) -> Result<String, VmError> {
    let (prefix, suffix) = negative_affixes(negative_pattern);
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    skip_ws(bytes, &mut pos, lax);
    if !prefix.is_empty() {
        if text[pos..].starts_with(&prefix) {
            pos += prefix.len();
        } else if !lax {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        } else {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        }
    }
    let mut end = bytes.len();
    if lax {
        while end > pos && is_ws(bytes[end - 1]) {
            end -= 1;
        }
    }
    if !suffix.is_empty() {
        let sl = suffix.len();
        if end >= sl && &text[end - sl..end] == suffix.as_str() {
            end -= sl;
        } else if !lax {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        } else {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        }
    }
    if pos > end {
        return Err(VmError::InvalidValue {
            message: "textNumberPattern mismatch".into(),
        });
    }
    let core = text[pos..end].to_string();
    let mut inner = core;
    match_subpattern(&mut inner, positive_pattern, props, lax, true)
}

fn skip_pad(
    bytes: &[u8],
    pos: &mut usize,
    pad: Option<char>,
    lax: bool,
    props: &TextNumberFormatProps<'_>,
    after_decimal: bool,
) {
    if !lax {
        return;
    }
    loop {
        skip_ws_respecting_decimal_sep(bytes, pos, props);
        if *pos >= bytes.len() {
            break;
        }
        let ch = bytes[*pos] as char;
        // Leading pad only (integer side of the decimal separator).
        if !after_decimal && Some(ch) == pad {
            *pos += ch.len_utf8();
            continue;
        }
        if !after_decimal && pad == Some('0') && bytes[*pos] == b'0' {
            *pos += 1;
            continue;
        }
        break;
    }
}

fn match_subpattern(
    text: &mut String,
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
    lax: bool,
    negative_subpattern: bool,
) -> Result<String, VmError> {
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    let mut int_digits = String::new();
    let mut frac_digits = String::new();
    let mut exponent: Option<String> = None;
    let mut negative = negative_subpattern;
    let mut saw_decimal = false;
    let mut in_exponent = false;

    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            let mut lit = String::new();
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        lit.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                lit.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: format!("invalid textNumberPattern `{pattern}`"),
                });
            }
            i += 1;
            skip_ws(bytes, &mut pos, lax);
            if text[pos..].starts_with(&lit) {
                pos += lit.len();
            } else if lax || negative {
                // lax: quoted literal may be omitted (DFDL-13-052R); also when parsing
                // the numeric core after a negative subpattern prefix (hex sign C/D, etc.)
            } else {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            continue;
        }

        match chars[i] {
            '0' | '#' => {
                let mut j = i;
                let mut max_digits = 0usize;
                while j < chars.len() && matches!(chars[j], '0' | '#') {
                    max_digits += 1;
                    j += 1;
                }
                i = j;
                // DFDL/ICU parse: digit characters in the data are always recognized (0 vs # affects unparse only).
                let min_digits = 0usize;

                skip_pad(bytes, &mut pos, props.pad_character, lax, props, saw_decimal);
                if !in_exponent && !saw_decimal {
                    if match_decimal_separator(bytes, &mut pos, props) {
                        saw_decimal = true;
                    }
                }
                let start = pos;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                let digit_count = pos - start;
                if digit_count < min_digits && !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                if digit_count == 0 && min_digits > 0 && !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                let chunk = core::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                if in_exponent {
                    exponent.get_or_insert_with(String::new).push_str(chunk);
                } else if saw_decimal {
                    frac_digits.push_str(chunk);
                } else {
                    int_digits.push_str(chunk);
                }
            }
            '*' => {
                skip_pad(bytes, &mut pos, props.pad_character, lax, props, saw_decimal);
                i += 1;
            }
            '.' => {
                // Match decimal separator before lax whitespace skip — when the separator is
                // space (or other WS), skip_ws would consume it and break parsing (DFDL-13-053R).
                let mut matched = match_decimal_separator(bytes, &mut pos, props);
                if !matched && lax {
                    skip_ws_respecting_decimal_sep(bytes, &mut pos, props);
                    matched = match_decimal_separator(bytes, &mut pos, props);
                }
                if !matched && !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                if matched {
                    saw_decimal = true;
                }
                i += 1;
            }
            ',' => {
                skip_ws(bytes, &mut pos, lax);
                if let Some(grp) = props.grouping_separator {
                    if !grp.is_empty() && text[pos..].starts_with(grp) {
                        pos += grp.len();
                    } else if !lax {
                        return Err(VmError::InvalidValue {
                            message: "textNumberPattern mismatch".into(),
                        });
                    }
                }
                i += 1;
            }
            'E' | 'e' => {
                if props.exponent_chars.contains(chars[i]) {
                    skip_ws(bytes, &mut pos, lax);
                    if pos < bytes.len() {
                        let b = bytes[pos];
                        if b == b'E' || b == b'e' || props.exponent_chars.contains(b as char) {
                            pos += 1;
                            in_exponent = true;
                            if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
                                if bytes[pos] == b'-' {
                                    negative = true;
                                }
                                pos += 1;
                            }
                            i += 1;
                            continue;
                        }
                    }
                }
                // fall through literal
                let lit = chars[i];
                skip_ws(bytes, &mut pos, lax);
                if pos + lit.len_utf8() <= bytes.len() {
                    let slice = core::str::from_utf8(&bytes[pos..pos + lit.len_utf8()]).ok();
                    if slice == Some(lit.to_string().as_str()) {
                        pos += lit.len_utf8();
                        i += 1;
                        continue;
                    }
                }
                if !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                i += 1;
            }
            ' ' => {
                if lax {
                    skip_ws(bytes, &mut pos, true);
                } else if pos < bytes.len() && is_ws(bytes[pos]) {
                    pos += 1;
                } else {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                i += 1;
            }
            'V' | 'P' => {
                // virtual decimal point / scaling position — not present in data
                i += 1;
            }
            ';' => {
                i += 1;
            }
            other => {
                skip_ws(bytes, &mut pos, lax);
                let lit = other;
                if pos + lit.len_utf8() <= bytes.len() {
                    let slice = core::str::from_utf8(&bytes[pos..pos + lit.len_utf8()]).ok();
                    if slice == Some(lit.to_string().as_str()) {
                        pos += lit.len_utf8();
                        i += 1;
                        continue;
                    }
                }
                if !lax {
                    return Err(VmError::InvalidValue {
                        message: "textNumberPattern mismatch".into(),
                    });
                }
                i += 1;
            }
        }
    }

    skip_ws(bytes, &mut pos, lax);
    if pos != bytes.len() {
        if lax {
            // allow trailing junk only if all digits consumed — strict about leftover
            let rest = core::str::from_utf8(&bytes[pos..]).unwrap_or("");
            if rest.chars().any(|c| c.is_ascii_digit()) {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
        } else {
            return Err(VmError::InvalidValue {
                message: "textNumberPattern mismatch".into(),
            });
        }
    }

    let int_norm = normalize_int_digits(&int_digits);
    let mut out = if saw_decimal || !frac_digits.is_empty() {
        if frac_digits.is_empty() {
            int_norm.clone()
        } else {
            format!("{}.{}", int_norm, frac_digits)
        }
    } else {
        int_norm
    };

    if let Some(exp) = exponent {
        if !exp.is_empty() {
            out.push('E');
            out.push_str(&exp);
        }
    }

    if negative && !out.starts_with('-') {
        out.insert(0, '-');
    }

    Ok(out)
}

fn normalize_int_digits(digits: &str) -> String {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lax_pattern_with_spaces() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("           0052    ", "    0000    ", &props).unwrap();
        assert_eq!(n, "52");
    }

    #[test]
    fn exponent_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("006.54E9", "000.0#E0", &props).unwrap();
        assert_eq!(n, "6.54E9");
    }

    #[test]
    fn strict_pad_exponent_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("006.54E9", "000.0#E0", &props).unwrap();
        assert_eq!(n, "6.54E9");
    }

    #[test]
    fn lax_optional_quoted_prefix() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("1234", "'$'0000", &props).unwrap();
        assert_eq!(n, "1234");
        let n2 = parse_standard_text_number("1234", "'optional:'0000", &props).unwrap();
        assert_eq!(n2, "1234");
    }

    #[test]
    fn lax_space_decimal_separator() {
        let dec = vec![" ".into()];
        let lax_props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let strict_props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let strict =
            parse_standard_text_number("$5 00", "'$'#0.00", &strict_props).unwrap();
        assert_eq!(strict, "5.00", "strict baseline got `{strict}`");
        let n = parse_standard_text_number("$5 00", "'$'#0.00", &lax_props).unwrap();
        assert_eq!(n, "5.00", "lax parse got `{n}` (strict=`{strict}`)");
        let n2 = parse_standard_text_number("$5 50", "'$'#0.00", &lax_props).unwrap();
        assert_eq!(n2, "5.50", "lax parse got `{n2}`");
    }

    #[test]
    fn space_decimal_separator() {
        let dec = vec![" ".into()];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("$5 00", "'$'#0.00", &props).unwrap();
        assert_eq!(n, "5.00");
    }

    #[test]
    fn quoted_dollar_float() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("$49.99", "'$'##0.00", &props).unwrap();
        assert_eq!(n, "49.99");
    }

    #[test]
    fn hex_charset_sign_pattern() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &dec,
            grouping_separator: None,
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("D123", "'C'000;'D'000", &props).unwrap();
        assert_eq!(n, "-123");
    }

    #[test]
    fn p_pattern_left() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("123", "PP000", &props).unwrap();
        assert_eq!(n, "0.00123");
    }

    #[test]
    fn p_pattern_right() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            decimal_separators: &dec,
            ..TextNumberFormatProps::default()
        };
        let n = parse_standard_text_number("123", "000PP", &props).unwrap();
        assert_eq!(n, "12300");
    }

    #[test]
    fn hex_charset_sign_pattern_strict() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: None,
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("D123", "'C'000;'D'000", &props).unwrap();
        assert_eq!(n, "-123");
    }

    fn strict_four_zeros() {
        let dec = default_decimal_separators();
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("1988", "0000", &props).unwrap();
        assert_eq!(n, "1988");
    }
}
