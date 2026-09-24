use super::decimal::{
    add_mag, div_mod_mag, from_scaled_integer, mul_mags, parse_mantissa_decimal, scaled_integer,
    should_round_up, trim_trailing_zeros, Decimal,
};
use crate::error::VmError;
use crate::schema::TextNumberRoundingMode;

pub(crate) fn apply_rounding_increment_to_decimal(
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

pub(crate) fn round_to_max_fraction_digits(
    d: &mut Decimal,
    max_frac: usize,
    mode: TextNumberRoundingMode,
) {
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

pub(crate) fn trim_insignificant_fraction(d: &mut Decimal) {
    trim_trailing_zeros(d);
}
