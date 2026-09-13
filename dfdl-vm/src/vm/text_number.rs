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

fn skip_pad(bytes: &[u8], pos: &mut usize, pad: Option<char>, lax: bool) {
    if !lax {
        return;
    }
    loop {
        skip_ws(bytes, pos, true);
        if *pos >= bytes.len() {
            break;
        }
        let ch = bytes[*pos] as char;
        if Some(ch) == pad {
            *pos += ch.len_utf8();
            continue;
        }
        if pad == Some('0') && bytes[*pos] == b'0' {
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
            let start = i;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: format!("invalid textNumberPattern `{pattern}`"),
                });
            }
            let lit: String = chars[start..i].iter().collect();
            i += 1;
            skip_ws(bytes, &mut pos, lax);
            if !text[pos..].starts_with(&lit) {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            pos += lit.len();
            continue;
        }

        match chars[i] {
            '0' | '#' => {
                let mut j = i;
                let mut min_digits = 0usize;
                let mut max_digits = 0usize;
                while j < chars.len() && matches!(chars[j], '0' | '#') {
                    if chars[j] == '0' {
                        min_digits += 1;
                    }
                    max_digits += 1;
                    j += 1;
                }
                i = j;

                skip_pad(bytes, &mut pos, props.pad_character, lax);
                if !in_exponent && !saw_decimal {
                    for dec in props.decimal_separators {
                        let ds = dec.to_string();
                        if text[pos..].starts_with(&ds)
                            || (*dec == '.' && pos < bytes.len() && bytes[pos] == b'.')
                        {
                            pos += ds.len().max(1);
                            saw_decimal = true;
                            break;
                        }
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
                skip_pad(bytes, &mut pos, props.pad_character, lax);
                i += 1;
            }
            '.' => {
                if !props.decimal_separators.iter().any(|&d| d == '.') {
                    // pattern dot when decimal sep is custom — treat as literal below
                }
                skip_ws(bytes, &mut pos, lax);
                let dec = props.decimal_separators.first().copied().unwrap_or('.');
                if pos < bytes.len() {
                    let ch = bytes[pos] as char;
                    if ch == dec || ch == '.' {
                        pos += ch.len_utf8();
                        saw_decimal = true;
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
