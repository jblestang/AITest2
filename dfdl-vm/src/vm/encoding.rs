use crate::error::VmError;
use crate::schema::EncodingErrorPolicy;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn normalize_encoding_name(name: &str) -> Option<&'static str> {
    if eq_ascii_ignore_case(name, "utf-16be") || eq_ascii_ignore_case(name, "utf_16be") {
        Some("utf-16be")
    } else if eq_ascii_ignore_case(name, "utf-8") || eq_ascii_ignore_case(name, "utf8") {
        Some("utf-8")
    } else if eq_ascii_ignore_case(name, "ascii") || eq_ascii_ignore_case(name, "us-ascii") {
        Some("ascii")
    } else {
        None
    }
}

fn eq_ascii_ignore_case(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

pub(crate) fn encode_document_text(text: &str, encoding: &str) -> Result<Vec<u8>, VmError> {
    match normalize_encoding_name(encoding) {
        Some("utf-8") | Some("ascii") => Ok(text.as_bytes().to_vec()),
        Some("utf-16be") => Ok(encode_utf16be(text)),
        _ => Err(VmError::UnsupportedOperation {
            op: alloc::format!("document encoding `{encoding}`"),
        }),
    }
}

pub(crate) fn decode_text_bytes(
    bytes: &[u8],
    encoding: &str,
    policy: EncodingErrorPolicy,
) -> Result<String, VmError> {
    match normalize_encoding_name(encoding) {
        Some("utf-8") => decode_utf8_text(bytes, policy),
        Some("ascii") => {
            if bytes.iter().any(|b| *b > 0x7f) {
                return Err(VmError::InvalidValue {
                    message: "invalid ASCII".into(),
                });
            }
            Ok(bytes.iter().map(|b| *b as char).collect())
        }
        Some("utf-16be") => decode_utf16be(bytes),
        _ => Err(VmError::UnsupportedOperation {
            op: alloc::format!("text decoding for encoding `{encoding}`"),
        }),
    }
}

pub(crate) fn character_span_byte_length(
    char_count: usize,
    encoding: &str,
) -> Result<usize, VmError> {
    match normalize_encoding_name(encoding) {
        Some("utf-8") | Some("ascii") => Ok(char_count),
        Some("utf-16be") => char_count
            .checked_mul(2)
            .ok_or(VmError::InvalidValue {
                message: "character span overflow".into(),
            }),
        _ => Err(VmError::UnsupportedOperation {
            op: alloc::format!("character span bytes for encoding `{encoding}`"),
        }),
    }
}

pub(crate) fn count_characters(
    bytes: &[u8],
    encoding: &str,
    policy: EncodingErrorPolicy,
) -> Result<usize, VmError> {
    match normalize_encoding_name(encoding) {
        Some("utf-8") => count_utf8_characters(bytes, policy),
        Some("ascii") => Ok(bytes.len()),
        Some("utf-16be") => {
            if bytes.len() % 2 != 0 {
                return Err(VmError::InvalidValue {
                    message: "invalid UTF-16BE byte length".into(),
                });
            }
            Ok(bytes.len() / 2)
        }
        _ => Err(VmError::UnsupportedOperation {
            op: alloc::format!("character counting for encoding `{encoding}`"),
        }),
    }
}

pub(crate) fn read_character_bytes(
    data: &[u8],
    pos: &mut usize,
    n: usize,
    encoding: &str,
    policy: EncodingErrorPolicy,
) -> Result<Vec<u8>, VmError> {
    let start = *pos;
    match normalize_encoding_name(encoding) {
        Some("utf-8") => {
            let mut count = 0usize;
            while count < n && *pos < data.len() {
                let width = read_one_utf8_char(data, *pos, policy)?.1;
                *pos += width;
                count += 1;
            }
            if count < n {
                return Err(VmError::UnexpectedEof);
            }
        }
        Some("ascii") => {
            if *pos + n > data.len() {
                return Err(VmError::UnexpectedEof);
            }
            *pos += n;
        }
        Some("utf-16be") => {
            let bytes = n.checked_mul(2).ok_or(VmError::InvalidValue {
                message: "character span overflow".into(),
            })?;
            if *pos + bytes > data.len() {
                return Err(VmError::UnexpectedEof);
            }
            *pos += bytes;
        }
        _ => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("character reading for encoding `{encoding}`"),
            });
        }
    }
    Ok(data[start..*pos].to_vec())
}

/// Read one encoded character and return `(decoded_char, byte_width)`.
pub(crate) fn read_one_utf8_char(
    data: &[u8],
    pos: usize,
    policy: EncodingErrorPolicy,
) -> Result<(char, usize), VmError> {
    if pos >= data.len() {
        return Err(VmError::UnexpectedEof);
    }
    let b0 = data[pos];
    if b0 < 0x80 {
        return Ok((b0 as char, 1));
    }

    let (width, code_point) = match b0 {
        0xC0..=0xDF => {
            if pos + 1 >= data.len() {
                return malformed_utf8(1, policy);
            }
            let b1 = data[pos + 1];
            if b1 & 0xC0 != 0x80 {
                return malformed_utf8(1, policy);
            }
            (
                2,
                ((b0 & 0x1F) as u32) << 6 | ((b1 & 0x3F) as u32),
            )
        }
        0xE0..=0xEF => {
            if pos + 2 >= data.len() {
                return malformed_utf8(1, policy);
            }
            let b1 = data[pos + 1];
            let b2 = data[pos + 2];
            if b1 & 0xC0 != 0x80 || b2 & 0xC0 != 0x80 {
                return malformed_utf8(1, policy);
            }
            (
                3,
                ((b0 & 0x0F) as u32) << 12
                    | ((b1 & 0x3F) as u32) << 6
                    | ((b2 & 0x3F) as u32),
            )
        }
        0xF0..=0xF4 => {
            if pos + 3 >= data.len() {
                return malformed_utf8(1, policy);
            }
            let b1 = data[pos + 1];
            let b2 = data[pos + 2];
            let b3 = data[pos + 3];
            if b1 & 0xC0 != 0x80 || b2 & 0xC0 != 0x80 || b3 & 0xC0 != 0x80 {
                return malformed_utf8(1, policy);
            }
            (
                4,
                ((b0 & 0x07) as u32) << 18
                    | ((b1 & 0x3F) as u32) << 12
                    | ((b2 & 0x3F) as u32) << 6
                    | ((b3 & 0x3F) as u32),
            )
        }
        _ => return malformed_utf8(1, policy),
    };

    if code_point > 0x10FFFF
        || (0xD800..=0xDFFF).contains(&code_point)
        || is_overlong(code_point, width)
    {
        return malformed_utf8(width, policy);
    }

    Ok((char::from_u32(code_point).unwrap(), width))
}

fn decode_utf8_text(bytes: &[u8], policy: EncodingErrorPolicy) -> Result<String, VmError> {
    match policy {
        EncodingErrorPolicy::Error => core::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid UTF-8".into(),
            }),
        EncodingErrorPolicy::Replace => {
            let mut out = String::new();
            let mut pos = 0usize;
            while pos < bytes.len() {
                let (ch, width) = read_one_utf8_char(bytes, pos, policy)?;
                out.push(ch);
                pos += width;
            }
            Ok(out)
        }
    }
}

fn count_utf8_characters(bytes: &[u8], policy: EncodingErrorPolicy) -> Result<usize, VmError> {
    let mut count = 0usize;
    let mut pos = 0usize;
    while pos < bytes.len() {
        let width = read_one_utf8_char(bytes, pos, policy)?.1;
        pos += width;
        count += 1;
    }
    Ok(count)
}

fn malformed_utf8(width: usize, policy: EncodingErrorPolicy) -> Result<(char, usize), VmError> {
    match policy {
        EncodingErrorPolicy::Replace => Ok(('\u{FFFD}', width)),
        EncodingErrorPolicy::Error => Err(VmError::InvalidValue {
            message: "Malformed UTF-8 data".into(),
        }),
    }
}

fn is_overlong(code_point: u32, width: usize) -> bool {
    width > min_utf8_width(code_point)
}

fn min_utf8_width(code_point: u32) -> usize {
    if code_point < 0x80 {
        1
    } else if code_point < 0x800 {
        2
    } else if code_point < 0x1_0000 {
        3
    } else {
        4
    }
}

fn encode_utf16be(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for ch in text.chars() {
        let unit = ch as u32;
        out.push((unit >> 8) as u8);
        out.push((unit & 0xff) as u8);
    }
    out
}

fn decode_utf16be(bytes: &[u8]) -> Result<String, VmError> {
    if bytes.len() % 2 != 0 {
        return Err(VmError::InvalidValue {
            message: "invalid UTF-16BE byte length".into(),
        });
    }
    let mut out = String::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let unit = ((chunk[0] as u32) << 8) | chunk[1] as u32;
        let ch = char::from_u32(unit).ok_or(VmError::InvalidValue {
            message: alloc::format!("invalid UTF-16BE code unit `0x{unit:04x}`"),
        })?;
        out.push(ch);
    }
    Ok(out)
}
