//! Packed, BCD, and IBM4690 decimal decoding (aligned with Apache Daffodil DecimalUtils).
use crate::error::VmError;
use crate::schema::BinaryNumberCheckPolicy;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub(crate) struct PackedSignCodes {
    positive: Vec<u8>,
    negative: Vec<u8>,
    unsigned: Vec<u8>,
    zero_sign: Vec<u8>,
}

impl PackedSignCodes {
    pub(crate) fn parse(spec: &str, policy: BinaryNumberCheckPolicy) -> Result<Self, VmError> {
        let compact: String = spec.chars().filter(|c| !c.is_whitespace()).collect();
        let chars: Vec<char> = compact.chars().collect();
        if chars.len() != 4 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("binaryPackedSignCodes must have 4 characters, got `{spec}`"),
            });
        }
        // Daffodil uses `char - 55` for A-F hex letters (C -> 0x0C).
        let code = |c: char| -> u8 {
            if c.is_ascii_digit() {
                c as u8 - b'0'
            } else {
                c as u8 - 55
            }
        };
        let (p, n, u, z) = (code(chars[0]), code(chars[1]), code(chars[2]), code(chars[3]));
        Ok(match policy {
            BinaryNumberCheckPolicy::Strict => Self {
                positive: vec![p],
                negative: vec![n],
                unsigned: vec![u],
                zero_sign: vec![z],
            },
            BinaryNumberCheckPolicy::Lax => Self {
                positive: vec![p, 0x0a, 0x0c, 0x0e, 0x0f],
                negative: vec![n, 0x0b, 0x0d],
                unsigned: vec![u, 0x0f],
                zero_sign: vec![z, 0x0a, 0x0c, 0x0e, 0x0f, 0x00],
            },
        })
    }

    fn sign_valid(&self, nibble: u8, negative: &mut bool) -> Result<(), VmError> {
        if self.negative.contains(&nibble) {
            *negative = true;
            return Ok(());
        }
        if self.positive.contains(&nibble)
            || self.unsigned.contains(&nibble)
            || self.zero_sign.contains(&nibble)
        {
            return Ok(());
        }
        Err(VmError::InvalidValue {
            message: alloc::format!("Invalid sign nibble: {nibble}"),
        })
    }
}

pub(crate) fn order_bytes(bytes: &[u8], le: bool) -> Vec<u8> {
    if le {
        bytes.iter().copied().rev().collect()
    } else {
        bytes.to_vec()
    }
}

pub(crate) fn bcd_to_digit_string(bytes: &[u8], le: bool) -> Result<String, VmError> {
    let mut out = String::new();
    for b in order_bytes(bytes, le) {
        let hi = b >> 4;
        let lo = b & 0x0f;
        if hi > 9 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("Invalid high nibble: {hi}"),
            });
        }
        if lo > 9 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("Invalid low nibble: {lo}"),
            });
        }
        out.push(char::from(b'0' + hi));
        out.push(char::from(b'0' + lo));
    }
    Ok(out)
}

pub(crate) fn packed_to_digit_string(
    bytes: &[u8],
    le: bool,
    codes: &PackedSignCodes,
) -> Result<(bool, String), VmError> {
    let num = order_bytes(bytes, le);
    if num.is_empty() {
        return Err(VmError::InvalidValue {
            message: "empty packed BCD".into(),
        });
    }
    let sign_nibble = num[num.len() - 1] & 0x0f;
    let mut negative = false;
    codes.sign_valid(sign_nibble, &mut negative)?;

    let mut out = String::with_capacity(num.len() * 2);
    for b in &num[..num.len() - 1] {
        let hi = b >> 4;
        let lo = b & 0x0f;
        if hi > 9 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("Invalid high nibble: {hi}"),
            });
        }
        if lo > 9 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("Invalid low nibble: {lo}"),
            });
        }
        out.push(char::from(b'0' + hi));
        out.push(char::from(b'0' + lo));
    }
    let last_digit = num[num.len() - 1] >> 4;
    if last_digit > 9 {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Invalid high nibble: {last_digit}"),
        });
    }
    out.push(char::from(b'0' + last_digit));
    Ok((negative, out))
}

pub(crate) fn ibm4690_to_digit_string(bytes: &[u8], le: bool) -> Result<(bool, String), VmError> {
    let num = order_bytes(bytes, le);
    let mut out = String::new();
    let mut negative = false;
    let mut in_digits = false;

    for b in num {
        let high = b >> 4;
        if high > 9 {
            out.push('0');
            if high == 0x0d && !in_digits {
                negative = true;
                in_digits = true;
            } else if high != 0x0f || in_digits {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Invalid high nibble: {high}"),
                });
            }
        } else {
            in_digits = true;
            out.push(char::from(b'0' + high));
        }

        let low = b & 0x0f;
        if low > 9 {
            out.push('0');
            if low == 0x0d && !in_digits {
                negative = true;
                in_digits = true;
            } else if low != 0x0f || in_digits {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Invalid low nibble: {low}"),
                });
            }
        } else {
            in_digits = true;
            out.push(char::from(b'0' + low));
        }
    }
    Ok((negative, out))
}

pub(crate) fn encode_packed_bcd_magnitude(
    magnitude: u64,
    negative: bool,
    width: usize,
    le: bool,
    codes: &PackedSignCodes,
) -> Result<Vec<u8>, VmError> {
    if width == 0 {
        return Err(VmError::InvalidValue {
            message: "zero-width packed BCD".into(),
        });
    }
    let mut digits = if magnitude == 0 {
        "0".to_string()
    } else {
        magnitude.to_string()
    };
    let digit_slots = width * 2 - 1;
    while digits.len() < digit_slots {
        digits.insert(0, '0');
    }
    if digits.len() > digit_slots {
        digits = digits[digits.len() - digit_slots..].to_string();
    }
    let sign = if magnitude == 0 {
        *codes.zero_sign.first().unwrap_or(&0x0c)
    } else if negative {
        *codes.negative.first().unwrap_or(&0x0d)
    } else {
        *codes.positive.first().unwrap_or(&0x0c)
    };
    let mut bytes = vec![0u8; width];
    let digit_bytes = digits.as_bytes();
    let mut di = 0usize;
    for i in 0..width.saturating_sub(1) {
        let hi = digit_bytes[di] - b'0';
        di += 1;
        let lo = if di < digit_bytes.len() {
            digit_bytes[di] - b'0'
        } else {
            0
        };
        di += 1;
        if hi > 9 || lo > 9 {
            return Err(VmError::InvalidValue {
                message: "invalid packed BCD digit".into(),
            });
        }
        bytes[i] = (hi << 4) | lo;
    }
    let last_digit = digit_bytes[di] - b'0';
    if last_digit > 9 {
        return Err(VmError::InvalidValue {
            message: "invalid packed BCD digit".into(),
        });
    }
    bytes[width - 1] = (last_digit << 4) | sign;
    if le {
        bytes.reverse();
    }
    Ok(bytes)
}

pub(crate) fn encode_ibm4690_magnitude(
    magnitude: u64,
    negative: bool,
    width: usize,
    le: bool,
) -> Result<Vec<u8>, VmError> {
    if width == 0 {
        return Err(VmError::InvalidValue {
            message: "zero-width IBM4690".into(),
        });
    }
    let digits = if magnitude == 0 {
        "0".to_string()
    } else {
        magnitude.to_string()
    };
    let mut nibbles: Vec<u8> = Vec::new();
    if negative {
        nibbles.push(0x0d);
    }
    for c in digits.chars() {
        let d = c as u8 - b'0';
        if d > 9 {
            return Err(VmError::InvalidValue {
                message: "invalid IBM4690 digit".into(),
            });
        }
        nibbles.push(d);
    }
    while nibbles.len() < width * 2 {
        nibbles.insert(0, 0x0f);
    }
    if nibbles.len() > width * 2 {
        nibbles = nibbles[nibbles.len() - width * 2..].to_vec();
    }
    let mut bytes = vec![0u8; width];
    for (i, pair) in nibbles.chunks(2).enumerate() {
        let hi = pair[0];
        let lo = pair.get(1).copied().unwrap_or(0x0f);
        bytes[i] = (hi << 4) | lo;
    }
    if le {
        bytes.reverse();
    }
    Ok(bytes)
}

pub(crate) fn digits_to_u64(digits: &str) -> Result<u64, VmError> {
    let trimmed = digits.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    trimmed.parse::<u64>().map_err(|_| VmError::InvalidValue {
        message: alloc::format!("invalid decimal digits `{digits}`"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_sign_c_d_f_c() {
        let codes = PackedSignCodes::parse("C D F C", BinaryNumberCheckPolicy::Strict).unwrap();
        let (neg, digits) = packed_to_digit_string(&[0x01, 0x98, 0x8c], false, &codes).unwrap();
        assert!(!neg);
        assert!(digits.ends_with("1988"), "digits={digits}");
        let (neg, digits) = packed_to_digit_string(&[0x12, 0x3d], false, &codes).unwrap();
        assert!(neg);
        assert_eq!(digits, "123");
        let enc = encode_packed_bcd_magnitude(123, true, 2, false, &codes).unwrap();
        assert_eq!(enc, vec![0x12, 0x3d]);
        let ibm = encode_ibm4690_magnitude(123, true, 2, false).unwrap();
        assert_eq!(ibm, vec![0xd1, 0x23]);
    }
}
