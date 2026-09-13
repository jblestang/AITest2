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
    pub decimal_separators: &'a [char],
    pub grouping_separator: Option<char>,
    pub exponent_chars: &'a str,
    pub pad_character: Option<char>,
}

impl Default for TextNumberFormatProps<'_> {
    fn default() -> Self {
        Self {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &['.'],
            grouping_separator: Some(','),
            exponent_chars: "E",
            pad_character: Some('0'),
        }
    }
}

/// Parse data against a DFDL text number pattern; returns a canonical numeric string for `parse()` / `parse_float`.
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

    for (idx, sub) in subpatterns.iter().enumerate() {
        let mut scratch = work.clone();
        match match_subpattern(&mut scratch, sub, props, lax, idx > 0) {
            Ok(num) => return Ok(num),
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
        let ch = bytes[*pos] as char;
        if props.decimal_separators.contains(&ch) {
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
    for dec in props.decimal_separators {
        let ds = dec.to_string();
        if *pos + ds.len() <= bytes.len() {
            let slice = core::str::from_utf8(&bytes[*pos..*pos + ds.len()]).ok();
            if slice == Some(ds.as_str()) {
                *pos += ds.len();
                return true;
            }
        }
        if *dec == '.' && *pos < bytes.len() && bytes[*pos] == b'.' {
            *pos += 1;
            return true;
        }
    }
    false
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
            } else if lax {
                // lax: quoted literal may be omitted (DFDL-13-052R)
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
                if digit_count > max_digits && !lax {
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
                if props.grouping_separator == Some(',') {
                    if pos < bytes.len() && bytes[pos] == b',' {
                        pos += 1;
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
            'V' => {
                // virtual decimal point — digits before/after split like simple V patterns
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
        let props = TextNumberFormatProps::default();
        let n = parse_standard_text_number("           0052    ", "    0000    ", &props).unwrap();
        assert_eq!(n, "52");
    }

    #[test]
    fn exponent_pattern() {
        let props = TextNumberFormatProps::default();
        let n = parse_standard_text_number("006.54E9", "000.0#E0", &props).unwrap();
        assert_eq!(n, "6.54E9");
    }

    #[test]
    fn strict_pad_exponent_pattern() {
        let dec = ['.'];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(','),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("006.54E9", "000.0#E0", &props).unwrap();
        assert_eq!(n, "6.54E9");
    }

    #[test]
    fn lax_optional_quoted_prefix() {
        let props = TextNumberFormatProps::default();
        let n = parse_standard_text_number("1234", "'$'0000", &props).unwrap();
        assert_eq!(n, "1234");
        let n2 = parse_standard_text_number("1234", "'optional:'0000", &props).unwrap();
        assert_eq!(n2, "1234");
    }

    #[test]
    fn lax_space_decimal_separator() {
        let dec = [' '];
        let lax_props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Lax,
            decimal_separators: &dec,
            grouping_separator: Some(','),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let strict_props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(','),
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
        let dec = [' '];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(','),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("$5 00", "'$'#0.00", &props).unwrap();
        assert_eq!(n, "5.00");
    }

    #[test]
    fn quoted_dollar_float() {
        let dec = ['.'];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(','),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("$49.99", "'$'##0.00", &props).unwrap();
        assert_eq!(n, "49.99");
    }

    fn strict_four_zeros() {
        let dec = ['.'];
        let props = TextNumberFormatProps {
            check_policy: BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(','),
            exponent_chars: "E",
            pad_character: Some('0'),
        };
        let n = parse_standard_text_number("1988", "0000", &props).unwrap();
        assert_eq!(n, "1988");
    }
}
