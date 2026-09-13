//! Standard text number unparsing (`textNumberPattern` + rounding).

use crate::error::VmError;
use crate::schema::{TextNumberRounding, TextNumberRoundingMode};
use crate::vm::text_number::{
    grouping_segment_slot_counts, pattern_without_quoted_regions, TextNumberFormatProps,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub(crate) struct TextNumberRoundingProps<'a> {
    pub rounding: TextNumberRounding,
    pub mode: TextNumberRoundingMode,
    pub increment: &'a str,
}

fn parse_quoted_literal(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) != Some(&'\'') {
        return None;
    }
    let mut j = i + 1;
    let mut lit = String::new();
    while j < chars.len() {
        if chars[j] == '\'' {
            if j + 1 < chars.len() && chars[j + 1] == '\'' {
                lit.push('\'');
                j += 2;
                continue;
            }
            return Some((lit, j + 1));
        }
        lit.push(chars[j]);
        j += 1;
    }
    None
}

fn expand_doubled_apostrophes(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            if let Some((lit, ni)) = parse_quoted_literal(&chars, i) {
                out.push_str(&lit);
                i = ni;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn find_e_index_bare(bare: &str) -> Option<usize> {
    bare.find(['E', 'e'])
}

#[derive(Debug, Clone)]
struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    scale: usize,
}

fn trim_trailing_zeros(d: &mut Decimal) {
    while d.scale > 0 && d.digits.last() == Some(&0) {
        d.digits.pop();
        d.scale -= 1;
    }
    while d.digits.len() > 1 && d.digits[0] == 0 && d.scale == 0 {
        d.digits.remove(0);
    }
    if d.digits.is_empty() {
        d.digits.push(0);
    }
}

fn parse_mantissa_decimal(raw: &str) -> Result<Decimal, VmError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(VmError::InvalidValue {
            message: "empty decimal".into(),
        });
    }
    let mut negative = false;
    let mut work = s;
    if work.starts_with('-') {
        negative = true;
        work = &work[1..];
    } else if work.starts_with('+') {
        work = &work[1..];
    }
    let (int_part, frac_part) = work.split_once('.').unwrap_or((work, ""));
    if !int_part.is_empty() && !int_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(VmError::InvalidValue {
            message: "invalid decimal".into(),
        });
    }
    if !frac_part.is_empty() && !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(VmError::InvalidValue {
            message: "invalid decimal".into(),
        });
    }
    let mut digits: Vec<u8> = int_part
        .trim_start_matches('0')
        .bytes()
        .map(|b| b - b'0')
        .collect();
    let scale = frac_part.len();
    for b in frac_part.bytes() {
        digits.push(b - b'0');
    }
    if digits.is_empty() {
        digits.push(0);
    }
    let mut d = Decimal {
        negative,
        digits,
        scale,
    };
    trim_trailing_zeros(&mut d);
    Ok(d)
}

fn parse_decimal_value(raw: &str) -> Result<Decimal, VmError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(VmError::InvalidValue {
            message: "empty decimal".into(),
        });
    }
    let mut negative = false;
    let mut work = s;
    if work.starts_with('-') {
        negative = true;
        work = &work[1..];
    } else if work.starts_with('+') {
        work = &work[1..];
    }
    let (mant, exp) = if let Some(idx) = work.find(['E', 'e']) {
        (&work[..idx], Some(&work[idx + 1..]))
    } else {
        (work, None)
    };
    let mantissa_text = if negative {
        format!("-{mant}")
    } else {
        mant.to_string()
    };
    let mut d = parse_mantissa_decimal(&mantissa_text)?;
    if let Some(exp_str) = exp {
        let exp_val: i32 = exp_str.parse().map_err(|_| VmError::InvalidValue {
            message: "invalid exponent".into(),
        })?;
        if exp_val >= 0 {
            let shift = exp_val as usize;
            if shift >= d.scale {
                d.digits.extend(core::iter::repeat(0).take(shift - d.scale));
                d.scale = 0;
            } else {
                d.scale -= shift;
            }
        } else {
            d.scale += (-exp_val) as usize;
        }
        trim_trailing_zeros(&mut d);
    }
    Ok(d)
}

struct SciParts {
    mantissa: Decimal,
    exponent: i32,
}

fn parse_sci_or_decimal(raw: &str) -> Result<(Decimal, Option<SciParts>), VmError> {
    let s = raw.trim();
    let body = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('+'))
        .unwrap_or(s);
    if let Some(idx) = body.find(['E', 'e']) {
        let exp_str = &body[idx + 1..];
        let exp: i32 = exp_str.parse().map_err(|_| VmError::InvalidValue {
            message: "invalid exponent".into(),
        })?;
        let mant_str = &body[..idx];
        let mut mantissa = parse_mantissa_decimal(mant_str)?;
        mantissa.negative = s.starts_with('-');
        return Ok((
            parse_decimal_value(raw)?,
            Some(SciParts {
                mantissa,
                exponent: exp,
            }),
        ));
    }
    Ok((parse_decimal_value(raw)?, None))
}

fn decimal_to_string(d: &Decimal) -> String {
    if d.scale == 0 {
        let mut s: String = d.digits.iter().map(|n| (b'0' + n) as char).collect();
        if d.negative {
            s.insert(0, '-');
        }
        return s;
    }
    let split = d.digits.len().saturating_sub(d.scale);
    let (int_digits, frac_digits) = if split == 0 {
        (vec![0u8], d.digits.clone())
    } else {
        (
            d.digits[..split].to_vec(),
            d.digits[split..].to_vec(),
        )
    };
    let mut s = int_digits.iter().map(|n| (b'0' + n) as char).collect::<String>();
    s.push('.');
    s.extend(frac_digits.iter().map(|n| (b'0' + n) as char));
    if d.negative {
        s.insert(0, '-');
    }
    s
}

fn unscaled(d: &Decimal) -> (Vec<u8>, bool) {
    let s = decimal_to_string(d);
    let mag: String = s
        .trim_start_matches('-')
        .chars()
        .filter(|c| *c != '.')
        .collect();
    let mut digits: Vec<u8> = mag.bytes().map(|b| b - b'0').collect();
    while digits.len() > 1 && digits[0] == 0 {
        digits.remove(0);
    }
    if digits.is_empty() {
        digits.push(0);
    }
    (digits, d.negative)
}

fn from_unscaled(digits: Vec<u8>, negative: bool) -> Decimal {
    let mut d = Decimal {
        negative,
        digits,
        scale: 0,
    };
    trim_trailing_zeros(&mut d);
    d
}

fn cmp_mag(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    a.cmp(b)
}

fn sub_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    debug_assert!(cmp_mag(a, b) != core::cmp::Ordering::Less);
    let n = a.len();
    let m = b.len();
    let mut out = a.to_vec();
    let mut borrow = 0i32;
    for i in 0..n {
        let ai = n - 1 - i;
        let bi = if i < m { m - 1 - i } else { usize::MAX };
        let bval = if bi == usize::MAX { 0 } else { b[bi] as i32 };
        let mut v = out[ai] as i32 - bval - borrow;
        if v < 0 {
            v += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out[ai] = v as u8;
    }
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    out
}

fn add_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut ai = a.len();
    let mut bi = b.len();
    let mut carry = 0u16;
    let mut out = Vec::new();
    while ai > 0 || bi > 0 || carry > 0 {
        let da = if ai > 0 {
            ai -= 1;
            a[ai]
        } else {
            0
        };
        let db = if bi > 0 {
            bi -= 1;
            b[bi]
        } else {
            0
        };
        let sum = da as u16 + db as u16 + carry;
        out.push((sum % 10) as u8);
        carry = sum / 10;
    }
    out.reverse();
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    if out.is_empty() {
        out.push(0);
    }
    out
}

fn mul_mag_small(a: &[u8], n: u8) -> Vec<u8> {
    if n == 0 {
        return vec![0];
    }
    let mut carry = 0u16;
    let mut out = vec![0u8; a.len() + 1];
    let out_len = out.len();
    for (i, &d) in a.iter().rev().enumerate() {
        let prod = d as u16 * n as u16 + carry;
        out[out_len - 1 - i] = (prod % 10) as u8;
        carry = prod / 10;
    }
    if carry > 0 {
        out[0] = carry as u8;
    } else {
        out.remove(0);
    }
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    out
}

fn digits_to_u128(d: &[u8]) -> Option<u128> {
    let mut acc = 0u128;
    for &digit in d {
        acc = acc.checked_mul(10)?.checked_add(digit as u128)?;
    }
    Some(acc)
}

fn u128_to_digits(mut n: u128) -> Vec<u8> {
    if n == 0 {
        return vec![0];
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push((n % 10) as u8);
        n /= 10;
    }
    out.reverse();
    out
}

fn div_mod_mag(dividend: &[u8], divisor: &[u8]) -> (Vec<u8>, Vec<u8>) {
    if divisor.iter().all(|&d| d == 0) {
        return (vec![0], dividend.to_vec());
    }
    if cmp_mag(dividend, divisor) == core::cmp::Ordering::Less {
        return (vec![0], dividend.to_vec());
    }
    if let (Some(a), Some(b)) = (digits_to_u128(dividend), digits_to_u128(divisor)) {
        return (u128_to_digits(a / b), u128_to_digits(a % b));
    }
    let mut rem = dividend.to_vec();
    let mut quotient = vec![0u8];
    while cmp_mag(&rem, divisor) != core::cmp::Ordering::Less {
        rem = sub_mag(&rem, divisor);
        quotient = add_mag(&quotient, &[1]);
    }
    (quotient, rem)
}

fn double_mag(d: &[u8]) -> Vec<u8> {
    let mut carry = 0u16;
    let mut out = vec![0u8; d.len() + 1];
    let out_len = out.len();
    for (i, &digit) in d.iter().rev().enumerate() {
        let sum = digit as u16 * 2 + carry;
        out[out_len - 1 - i] = (sum % 10) as u8;
        carry = sum / 10;
    }
    if carry > 0 {
        out[0] = carry as u8;
    } else {
        out.remove(0);
    }
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    out
}

fn should_round_up(
    rem: &[u8],
    divisor: &[u8],
    mode: TextNumberRoundingMode,
    negative: bool,
    quotient_last: u8,
) -> bool {
    if rem.iter().all(|&d| d == 0) {
        return false;
    }
    let twice_rem = double_mag(rem);
    let cmp = cmp_mag(&twice_rem, divisor);
    let is_half = cmp == core::cmp::Ordering::Equal;
    let gt_half = cmp == core::cmp::Ordering::Greater;
    let _ = divisor;
    match mode {
        TextNumberRoundingMode::RoundUnnecessary => false,
        TextNumberRoundingMode::RoundUp => !negative,
        TextNumberRoundingMode::RoundDown => negative,
        TextNumberRoundingMode::RoundCeiling => !negative,
        TextNumberRoundingMode::RoundFloor => negative,
        TextNumberRoundingMode::RoundHalfUp => gt_half || is_half,
        TextNumberRoundingMode::RoundHalfDown => gt_half,
        TextNumberRoundingMode::RoundHalfEven => {
            if gt_half {
                true
            } else if is_half {
                quotient_last % 2 == 1
            } else {
                false
            }
        }
    }
}

fn mul_mags(a: &[u8], b: &[u8]) -> Vec<u8> {
    if a.iter().all(|&d| d == 0) || b.iter().all(|&d| d == 0) {
        return vec![0];
    }
    if let (Some(x), Some(y)) = (digits_to_u128(a), digits_to_u128(b)) {
        return u128_to_digits(x.saturating_mul(y));
    }
    let mut acc = vec![0u8];
    for &bd in b {
        for _ in 0..bd {
            acc = add_mag(&acc, a);
        }
    }
    acc
}

fn scaled_integer(d: &Decimal, target_scale: usize) -> Vec<u8> {
    let s = decimal_to_string(&Decimal {
        negative: false,
        digits: d.digits.clone(),
        scale: d.scale,
    });
    let (int_part, frac_part) = s.split_once('.').unwrap_or((&s, ""));
    let mut frac = frac_part.to_string();
    if frac.len() < target_scale {
        frac.extend(core::iter::repeat('0').take(target_scale - frac.len()));
    } else if frac.len() > target_scale {
        frac.truncate(target_scale);
    }
    let combined = format!("{int_part}{frac}");
    let mut digits: Vec<u8> = combined.bytes().map(|b| b - b'0').collect();
    while digits.len() > 1 && digits[0] == 0 {
        digits.remove(0);
    }
    if digits.is_empty() {
        digits.push(0);
    }
    digits
}

fn from_scaled_integer(digits: Vec<u8>, scale: usize, negative: bool) -> Decimal {
    let mut d = Decimal {
        negative,
        digits,
        scale,
    };
    trim_trailing_zeros(&mut d);
    d
}

fn apply_rounding_increment_to_decimal(
    d: &Decimal,
    increment: &str,
    mode: TextNumberRoundingMode,
) -> Result<Decimal, VmError> {
    if increment.is_empty() || increment == "0" {
        return Ok(d.clone());
    }
    let inc = parse_mantissa_decimal(increment)?;
    let target_scale = core::cmp::max(d.scale, inc.scale);
    let v_mag = scaled_integer(d, target_scale);
    let i_mag = scaled_integer(&inc, target_scale);
    if i_mag.iter().all(|&x| x == 0) {
        return Ok(d.clone());
    }
    let (mut q, rem) = div_mod_mag(&v_mag, &i_mag);
    let q_last = *q.last().unwrap_or(&0);
    if should_round_up(&rem, &i_mag, mode, d.negative, q_last) {
        q = add_mag(&q, &[1]);
    }
    let product = mul_mags(&q, &i_mag);
    Ok(from_scaled_integer(product, target_scale, d.negative))
}

fn decimal_is_zero(d: &Decimal) -> bool {
    d.digits.iter().all(|&x| x == 0)
}

fn needs_fraction_display(d: &Decimal, min_frac: usize) -> bool {
    if min_frac > 0 {
        return true;
    }
    if d.scale == 0 {
        return false;
    }
    let split = d.digits.len().saturating_sub(d.scale);
    d.digits[split..].iter().any(|&x| x != 0)
}

fn trim_insignificant_fraction(d: &mut Decimal) {
    trim_trailing_zeros(d);
}

fn format_grouped_integer(digits: &[u8], positive_pattern: &str, sep: &str) -> String {
    if digits.is_empty() {
        return "0".into();
    }
    let bare = pattern_without_quoted_regions(positive_pattern);
    let int_part = bare
        .split(['.', 'E', 'e', ';'])
        .next()
        .unwrap_or(bare.as_str());
    if !int_part.contains(',') {
        return digits.iter().map(|n| (b'0' + n) as char).collect();
    }
    let segs = grouping_segment_slot_counts(int_part);
    if segs.is_empty() {
        return digits.iter().map(|n| (b'0' + n) as char).collect();
    }
    let mut idx = digits.len();
    let mut groups: Vec<Vec<u8>> = Vec::new();
    for &seg in segs.iter().rev() {
        if idx == 0 {
            break;
        }
        let take = seg.min(idx);
        let start = idx - take;
        groups.push(digits[start..idx].to_vec());
        idx = start;
    }
    if idx > 0 {
        groups.push(digits[0..idx].to_vec());
    }
    groups.reverse();
    groups
        .iter()
        .map(|g| g.iter().map(|n| (b'0' + n) as char).collect::<String>())
        .collect::<Vec<_>>()
        .join(sep)
}

fn fraction_digit_bounds(positive_bare: &str) -> (usize, usize) {
    let head = positive_bare
        .split(['E', 'e', ';'])
        .next()
        .unwrap_or(positive_bare);
    let Some(dot) = head.find('.') else {
        return (0, 0);
    };
    let frac = &head[dot + 1..];
    let min = frac.chars().filter(|&c| c == '0').count();
    let max = frac.chars().filter(|&c| matches!(c, '0' | '#')).count();
    (min, max)
}

fn fraction_round_increments(
    round_digit: u8,
    tail: &[u8],
    mode: TextNumberRoundingMode,
    q_last: u8,
    negative: bool,
) -> bool {
    let tail_has = tail.iter().any(|&d| d > 0);
    match mode {
        TextNumberRoundingMode::RoundUnnecessary => false,
        TextNumberRoundingMode::RoundUp => !negative,
        TextNumberRoundingMode::RoundDown => negative,
        TextNumberRoundingMode::RoundCeiling => !negative,
        TextNumberRoundingMode::RoundFloor => negative,
        TextNumberRoundingMode::RoundHalfUp => round_digit > 5 || (round_digit == 5 && tail_has),
        TextNumberRoundingMode::RoundHalfDown => round_digit > 5,
        TextNumberRoundingMode::RoundHalfEven => {
            round_digit > 5 || (round_digit == 5 && (tail_has || q_last % 2 == 1))
        }
    }
}

fn round_to_max_fraction_digits(d: &mut Decimal, max_frac: usize, mode: TextNumberRoundingMode) {
    if d.scale <= max_frac {
        return;
    }
    let split = d.digits.len().saturating_sub(d.scale);
    let round_idx = split + max_frac;
    if round_idx >= d.digits.len() {
        d.scale = max_frac;
        return;
    }
    let round_digit = d.digits[round_idx];
    let tail = &d.digits[round_idx + 1..];
    let q_last = if round_idx > 0 {
        d.digits[round_idx - 1]
    } else {
        0
    };
    let mut new_digits = d.digits[..round_idx].to_vec();
    if fraction_round_increments(round_digit, tail, mode, q_last, d.negative) {
        let mut carry = 1u16;
        for i in (0..new_digits.len()).rev() {
            let sum = new_digits[i] as u16 + carry;
            new_digits[i] = (sum % 10) as u8;
            carry = sum / 10;
            if carry == 0 {
                break;
            }
        }
        if carry > 0 {
            new_digits.insert(0, 1);
        }
    }
    d.digits = new_digits;
    d.scale = max_frac;
    trim_trailing_zeros(d);
}

fn normalize_sci_mantissa(d: &Decimal) -> SciParts {
    if d.digits == [0] {
        return SciParts {
            mantissa: Decimal {
                negative: d.negative,
                digits: vec![0],
                scale: 0,
            },
            exponent: 0,
        };
    }
    let s = decimal_to_string(d);
    let mut work = if d.negative { s[1..].to_string() } else { s };
    if !work.contains('.') {
        work.push('.');
    }
    let digits_only: String = work.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut mant_digits: Vec<u8> = digits_only.bytes().map(|b| b - b'0').collect();
    while mant_digits.len() > 1 && mant_digits[0] == 0 {
        mant_digits.remove(0);
    }
    let first_nz = mant_digits.iter().position(|&x| x > 0).unwrap_or(0);
    let exp_adjust = if d.scale == 0 {
        mant_digits.len() as i32 - 1 - first_nz as i32
    } else {
        let int_len = d.digits.len().saturating_sub(d.scale);
        (int_len as i32 - 1) - (d.scale as i32)
    };
    let mut m = mant_digits[first_nz..].to_vec();
    if m.is_empty() {
        m.push(0);
    }
    let scale = if m.len() > 1 { m.len() - 1 } else { 0 };
    SciParts {
        mantissa: Decimal {
            negative: d.negative,
            digits: m,
            scale,
        },
        exponent: exp_adjust,
    }
}

fn format_exponent(exp: i32, exp_pattern: &str) -> String {
    let bare = exp_pattern;
    let mut min_digits = bare.chars().filter(|&c| c == '0').count();
    if min_digits == 0 {
        min_digits = 1;
    }
    let show_plus = bare.contains('+') && exp >= 0;
    let abs = exp.unsigned_abs();
    let mut digits = format!("{abs}");
    while digits.len() < min_digits {
        digits.insert(0, '0');
    }
    if show_plus {
        format!("+{digits}")
    } else if exp < 0 {
        format!("-{digits}")
    } else {
        digits
    }
}

fn int_pattern_ends_at_dot(chars: &[char], start: usize) -> Option<usize> {
    let mut j = start;
    while j < chars.len() {
        match chars[j] {
            '0' | '#' | ',' => j += 1,
            '.' => return Some(j),
            '\'' => {
                if let Some((_, ni)) = parse_quoted_literal(chars, j) {
                    j = ni;
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }
    None
}

fn format_mantissa_pattern(
    pattern: &str,
    d: &Decimal,
    props: &TextNumberFormatProps<'_>,
    min_frac: usize,
) -> Result<String, VmError> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    let dec_sep = props
        .decimal_separators
        .first()
        .map(|s| s.as_str())
        .unwrap_or(".");
    let grp_sep = props.grouping_separator.unwrap_or(",");

    let split = d.digits.len().saturating_sub(d.scale);
    let (int_digits, frac_digits) = if d.scale == 0 {
        (d.digits.clone(), Vec::new())
    } else if split == 0 {
        (vec![0u8], d.digits.clone())
    } else {
        (d.digits[..split].to_vec(), d.digits[split..].to_vec())
    };

    let mut int_idx = 0usize;
    let mut frac_idx = 0usize;
    let mut saw_decimal = false;
    let bare = pattern_without_quoted_regions(pattern);
    let ends_with_dot = bare.ends_with('.');

    while i < chars.len() {
        if chars[i] == '\'' {
            if let Some((lit, ni)) = parse_quoted_literal(&chars, i) {
                out.push_str(&lit);
                i = ni;
                continue;
            }
        }
        if matches!(chars[i], 'E' | 'e') {
            break;
        }
        match chars[i] {
            '0' | '#' => {
                let mut j = i;
                let mut max = 0usize;
                let mut min = 0usize;
                while j < chars.len() && matches!(chars[j], '0' | '#') {
                    if chars[j] == '0' {
                        min += 1;
                    }
                    max += 1;
                    j += 1;
                }
                i = j;
                let stops_at_dot =
                    int_pattern_ends_at_dot(&chars, i.saturating_sub(max)).is_some() && !saw_decimal;
                if stops_at_dot && !saw_decimal {
                    let dot_at = int_pattern_ends_at_dot(&chars, i.saturating_sub(max)).unwrap();
                    let avail = int_digits.len().saturating_sub(int_idx);
                    let need = core::cmp::max(min, avail);
                    let pad = need.saturating_sub(avail);
                    let mut chunk = Vec::new();
                    for _ in 0..pad {
                        chunk.push(0u8);
                    }
                    chunk.extend_from_slice(&int_digits[int_idx..]);
                    out.push_str(&format_grouped_integer(&chunk, pattern, grp_sep));
                    int_idx = int_digits.len();
                    out.push_str(dec_sep);
                    saw_decimal = true;
                    i = dot_at + 1;
                    continue;
                }
                if saw_decimal {
                    let mut emitted = 0usize;
                    while frac_idx < frac_digits.len() && emitted < max {
                        let digit = frac_digits[frac_idx];
                        let remaining = &frac_digits[frac_idx..];
                        if min == 0
                            && remaining.iter().all(|&d| d == 0)
                            && emitted >= min
                        {
                            break;
                        }
                        if min == 0 && emitted >= min {
                            let rest_significant = remaining.iter().any(|&d| d != 0);
                            if !rest_significant {
                                break;
                            }
                        }
                        out.push((b'0' + digit) as char);
                        frac_idx += 1;
                        emitted += 1;
                    }
                    while emitted < min {
                        out.push('0');
                        emitted += 1;
                    }
                } else if int_idx >= int_digits.len() {
                    // Integer digits already emitted; skip redundant int slots.
                } else {
                    let avail = int_digits.len().saturating_sub(int_idx);
                    let need = core::cmp::max(min, avail);
                    let pad = need.saturating_sub(avail);
                    let mut chunk = Vec::new();
                    for _ in 0..pad {
                        chunk.push(0u8);
                    }
                    chunk.extend_from_slice(&int_digits[int_idx..]);
                    out.push_str(&format_grouped_integer(&chunk, pattern, grp_sep));
                    int_idx = int_digits.len();
                }
            }
            '.' => {
                if !needs_fraction_display(d, min_frac) {
                    i += 1;
                    while i < chars.len() && matches!(chars[i], '0' | '#') {
                        i += 1;
                    }
                    continue;
                }
                saw_decimal = true;
                out.push_str(dec_sep);
                i += 1;
            }
            ',' => {
                if !saw_decimal && int_idx < int_digits.len() {
                    out.push_str(grp_sep);
                }
                i += 1;
            }
            other => {
                out.push(other);
                if saw_decimal && other.is_ascii_digit() {
                    if frac_idx < frac_digits.len()
                        && (b'0' + frac_digits[frac_idx]) as char == other
                    {
                        frac_idx += 1;
                    }
                }
                i += 1;
            }
        }
    }

    if ends_with_dot && !saw_decimal {
        out.push_str(dec_sep);
    } else if !saw_decimal && min_frac > 0 {
        out.push_str(dec_sep);
        for _ in 0..min_frac {
            out.push('0');
        }
    }

    Ok(out)
}

pub(crate) fn text_number_value_is_zero(value: &str) -> bool {
    parse_decimal_value(value.trim())
        .map(|d| decimal_is_zero(&d))
        .unwrap_or(false)
}

pub(crate) fn text_standard_zero_unparse(value: &str, raw_zero_rep: &str) -> Option<String> {
    if !text_number_value_is_zero(value) {
        return None;
    }
    let reps = crate::schema::parse_text_standard_zero_rep_list(raw_zero_rep);
    if reps.is_empty() {
        return None;
    }
    Some(reps[0].clone())
}

pub(crate) fn format_standard_text_number(
    value: &str,
    pattern: &str,
    props: &TextNumberFormatProps<'_>,
    rounding: TextNumberRoundingProps<'_>,
) -> Result<String, VmError> {
    let pattern_expanded = expand_doubled_apostrophes(pattern);
    let positive = pattern_expanded
        .split(';')
        .next()
        .unwrap_or(pattern_expanded.as_str());
    let bare_positive = pattern_without_quoted_regions(positive);
    let sci = find_e_index_bare(&bare_positive).is_some();
    let (min_frac, max_frac) = fraction_digit_bounds(&bare_positive);

    let negative_input = value.trim().starts_with('-');
    let increment = rounding.increment.trim();
    let use_explicit = matches!(rounding.rounding, TextNumberRounding::Explicit)
        && !increment.is_empty()
        && increment != "0";

    if sci {
        let parts = match parse_sci_or_decimal(value.trim())? {
            (_, Some(p)) => p,
            (d, None) => normalize_sci_mantissa(&d),
        };
        let mut mant = parts.mantissa.clone();
        if use_explicit {
            mant = apply_rounding_increment_to_decimal(&mant, increment, rounding.mode)?;
        }
        round_to_max_fraction_digits(&mut mant, max_frac, rounding.mode);
        let exp_idx = find_e_index_bare(&bare_positive).unwrap_or(bare_positive.len());
        let mant_pattern = &positive[..positive
            .char_indices()
            .nth(exp_idx)
            .map(|(i, _)| i)
            .unwrap_or(positive.len())];
        let exp_pattern = &positive[positive
            .char_indices()
            .nth(exp_idx)
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(positive.len())..];
        let exp_char = props.exponent_chars.chars().next().unwrap_or('E');
        let mut formatted = format_mantissa_pattern(mant_pattern, &mant, props, min_frac)?;
        if negative_input && !formatted.starts_with('-') {
            formatted.insert(0, '-');
        }
        formatted.push(exp_char);
        formatted.push_str(&format_exponent(parts.exponent, exp_pattern));
        return Ok(formatted);
    }

    let mut d = parse_decimal_value(value.trim())?;
    if use_explicit {
        d = apply_rounding_increment_to_decimal(&d, increment, rounding.mode)?;
    }
    if matches!(rounding.mode, TextNumberRoundingMode::RoundUnnecessary) {
        let before = decimal_to_string(&d);
        let mut probe = d.clone();
        round_to_max_fraction_digits(&mut probe, max_frac, rounding.mode);
        if decimal_to_string(&probe) != before {
            return Err(VmError::InvalidValue {
                message: "Unparse Error. rounding required with roundUnnecessary".into(),
            });
        }
    } else {
        round_to_max_fraction_digits(&mut d, max_frac, rounding.mode);
    }
    trim_insignificant_fraction(&mut d);

    let mut formatted = format_mantissa_pattern(positive, &d, props, min_frac)?;
    if negative_input && !formatted.starts_with('-') {
        formatted.insert(0, '-');
    }
    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::TextNumberRoundingMode;
    use crate::vm::text_number::TextNumberFormatProps;

    #[test]
    fn mantissa_only_double_zero() {
        let dec = alloc::vec![alloc::string::String::from(".")];
        let fmt = TextNumberFormatProps {
            check_policy: crate::schema::BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: None,
            exponent_chars: "E",
            pad_character: Some('0'),
            ignore_case: false,
        };
        let d = super::parse_decimal_value("1236").unwrap();
        let out = super::format_mantissa_pattern("##00", &d, &fmt, 0).unwrap();
        assert_eq!(out, "1236");
    }

    #[test]
    fn increment_round_1235_5() {
        let d = super::parse_mantissa_decimal("1235.5").unwrap();
        assert_eq!(d.digits, vec![1, 2, 3, 5, 5]);
        assert_eq!(d.scale, 1);
        assert_eq!(super::decimal_to_string(&d), "1235.5");
        let rounded = super::apply_rounding_increment_to_decimal(
            &d,
            "1",
            TextNumberRoundingMode::RoundHalfEven,
        )
        .unwrap();
        assert_eq!(super::decimal_to_string(&rounded), "1236");
    }

    #[test]
    fn pattern_double_zero_suffix() {
        let dec = alloc::vec![alloc::string::String::from(".")];
        let fmt = TextNumberFormatProps {
            check_policy: crate::schema::BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ignore_case: false,
        };
        let rounding = TextNumberRoundingProps {
            rounding: TextNumberRounding::Explicit,
            mode: TextNumberRoundingMode::RoundHalfEven,
            increment: "1",
        };
        let out =
            format_standard_text_number("1235.5", "##00", &fmt, rounding).unwrap();
        assert_eq!(out, "1236");
    }

    #[test]
    fn round_increment_and_pattern() {
        let dec = alloc::vec![alloc::string::String::from(".")];
        let fmt = TextNumberFormatProps {
            check_policy: crate::schema::BinaryNumberCheckPolicy::Strict,
            decimal_separators: &dec,
            grouping_separator: Some(","),
            exponent_chars: "E",
            pad_character: Some('0'),
            ignore_case: false,
        };
        let rounding = TextNumberRoundingProps {
            rounding: TextNumberRounding::Explicit,
            mode: TextNumberRoundingMode::RoundHalfEven,
            increment: "0.02",
        };
        let out = format_standard_text_number("0.128", "#,##0.02#", &fmt, rounding).unwrap();
        assert_eq!(out, "0.12");
    }
}
