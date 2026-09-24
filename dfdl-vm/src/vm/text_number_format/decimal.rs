use crate::error::VmError;
use crate::schema::TextNumberRoundingMode;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub(crate) struct Decimal {
    pub negative: bool,
    pub digits: Vec<u8>,
    pub scale: usize,
}

pub(crate) fn trim_trailing_zeros(d: &mut Decimal) {
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

pub(crate) fn parse_mantissa_decimal(raw: &str) -> Result<Decimal, VmError> {
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

pub(crate) fn parse_decimal_value(raw: &str) -> Result<Decimal, VmError> {
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
                d.digits.extend(core::iter::repeat_n(0, shift - d.scale));
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

pub(crate) struct SciParts {
    pub mantissa: Decimal,
    pub exponent: i32,
}

pub(crate) fn parse_sci_or_decimal(raw: &str) -> Result<(Decimal, Option<SciParts>), VmError> {
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

pub(crate) fn decimal_to_string(d: &Decimal) -> String {
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
        (d.digits[..split].to_vec(), d.digits[split..].to_vec())
    };
    let mut s = int_digits
        .iter()
        .map(|n| (b'0' + n) as char)
        .collect::<String>();
    s.push('.');
    s.extend(frac_digits.iter().map(|n| (b'0' + n) as char));
    if d.negative {
        s.insert(0, '-');
    }
    s
}

pub(crate) fn cmp_mag(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    a.cmp(b)
}

pub(crate) fn sub_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
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

pub(crate) fn add_mag(a: &[u8], b: &[u8]) -> Vec<u8> {
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

pub(crate) fn div_mod_mag(dividend: &[u8], divisor: &[u8]) -> (Vec<u8>, Vec<u8>) {
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

pub(crate) fn should_round_up(
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

pub(crate) fn mul_mags(a: &[u8], b: &[u8]) -> Vec<u8> {
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

pub(crate) fn scaled_integer(d: &Decimal, target_scale: usize) -> Vec<u8> {
    let s = decimal_to_string(&Decimal {
        negative: false,
        digits: d.digits.clone(),
        scale: d.scale,
    });
    let (int_part, frac_part) = s.split_once('.').unwrap_or((&s, ""));
    let mut frac = frac_part.to_string();
    if frac.len() < target_scale {
        frac.extend(core::iter::repeat_n('0', target_scale - frac.len()));
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

pub(crate) fn from_scaled_integer(digits: Vec<u8>, scale: usize, negative: bool) -> Decimal {
    let mut d = Decimal {
        negative,
        digits,
        scale,
    };
    trim_trailing_zeros(&mut d);
    d
}

pub(crate) fn decimal_is_zero(d: &Decimal) -> bool {
    d.digits.iter().all(|&x| x == 0)
}
