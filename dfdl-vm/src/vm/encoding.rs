use crate::error::VmError;
use crate::schema::{BitOrder, ByteOrder, EncodingErrorPolicy};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexCharsetOrder {
    MostSignificantByteFirst,
    LeastSignificantByteFirst,
}

const HEX_CHARSET_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Expand binary bytes (from a hex charset read) into the corresponding digit string.
pub(crate) fn hex_charset_payload_to_text(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX_CHARSET_DIGITS[(b >> 4) as usize] as char);
        s.push(HEX_CHARSET_DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

pub(crate) fn hex_charset_order(name: &str) -> Option<HexCharsetOrder> {
    if eq_ascii_ignore_case(name, "X-DFDL-HEX-MSBF") {
        Some(HexCharsetOrder::MostSignificantByteFirst)
    } else if eq_ascii_ignore_case(name, "X-DFDL-HEX-LSBF") {
        Some(HexCharsetOrder::LeastSignificantByteFirst)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BitsCharsetSpec {
    pub width: u8,
    pub alphabet: &'static str,
    pub bit_order: BitOrder,
}

pub(crate) fn bits_charset_spec(name: &str) -> Option<BitsCharsetSpec> {
    if eq_ascii_ignore_case(name, "X-DFDL-BITS-MSBF") {
        Some(BitsCharsetSpec {
            width: 1,
            alphabet: "01",
            bit_order: BitOrder::MostSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-BITS-LSBF") {
        Some(BitsCharsetSpec {
            width: 1,
            alphabet: "01",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-BASE4-MSBF") {
        Some(BitsCharsetSpec {
            width: 2,
            alphabet: "0123",
            bit_order: BitOrder::MostSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-BASE4-LSBF") {
        Some(BitsCharsetSpec {
            width: 2,
            alphabet: "0123",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-5-BIT-PACKED-LSBF") {
        Some(BitsCharsetSpec {
            width: 5,
            alphabet: "01234567ABCDEFGHJKLMNPQRSTUVWXYZ",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-OCTAL-LSBF") {
        Some(BitsCharsetSpec {
            width: 3,
            alphabet: "01234567",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-OCTAL-MSBF") {
        Some(BitsCharsetSpec {
            width: 3,
            alphabet: "01234567",
            bit_order: BitOrder::MostSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-3-BIT-DFI-336-DUI-001") {
        Some(BitsCharsetSpec {
            width: 3,
            alphabet: "12345678",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-3-BIT-DFI-746-DUI-002") {
        Some(BitsCharsetSpec {
            width: 3,
            alphabet: "ABCDEFGH",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-4-BIT-DFI-746-DUI-002") {
        Some(BitsCharsetSpec {
            width: 4,
            alphabet: "ABCDEFGHIJKLMNPQ",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-3-BIT-DFI-747-DUI-001") {
        Some(BitsCharsetSpec {
            width: 3,
            alphabet: "AEGHJKLM",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-5-BIT-DFI-769-DUI-002") {
        Some(BitsCharsetSpec {
            width: 5,
            alphabet: "01234567ABCDEFGHJKLMNPQRSTUVWXYZ",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-6-BIT-DFI-264-DUI-001") {
        Some(BitsCharsetSpec {
            width: 6,
            alphabet: " 123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}0",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-5-BIT-DFI-1661-DUI-001") {
        Some(BitsCharsetSpec {
            width: 5,
            alphabet: "\u{00A0}ABCDEFGHIJKLMNOPQRSTUVWXYZ\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}",
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else if eq_ascii_ignore_case(name, "X-DFDL-US-ASCII-7-BIT-PACKED")
        || eq_ascii_ignore_case(name, "us-ascii-7-bit-packed")
    {
        Some(BitsCharsetSpec {
            width: 7,
            alphabet: USASCII7_BIT_PACKED_ALPHABET,
            bit_order: BitOrder::LeastSignificantBitFirst,
        })
    } else {
        None
    }
}

/// Code units 0..=127 for `X-DFDL-US-ASCII-7-BIT-PACKED`.
const USASCII7_BIT_PACKED_ALPHABET: &str = {
    const BYTES: [u8; 128] = {
        let mut out = [0u8; 128];
        let mut i = 0usize;
        while i < 128 {
            out[i] = i as u8;
            i += 1;
        }
        out
    };
    match core::str::from_utf8(&BYTES) {
        Ok(s) => s,
        Err(_) => "",
    }
};

pub(crate) fn bits_charset_code_unit_width(name: &str) -> Option<u64> {
    bits_charset_spec(name).map(|s| s.width as u64)
}

fn read_bit_at(data: &[u8], bit_index: usize, order: BitOrder) -> u8 {
    let byte = data[bit_index / 8];
    let bit_in_byte = bit_index % 8;
    match order {
        BitOrder::MostSignificantBitFirst => (byte >> (7 - bit_in_byte)) & 1,
        BitOrder::LeastSignificantBitFirst => (byte >> bit_in_byte) & 1,
    }
}

fn write_bit_to_buffer(out: &mut Vec<u8>, bit_count: &mut u8, bit: u8, order: BitOrder) {
    match order {
        BitOrder::LeastSignificantBitFirst => {
            if *bit_count == 0 {
                out.push(0);
            }
            let idx = out.len() - 1;
            out[idx] |= (bit & 1) << *bit_count;
            *bit_count += 1;
            if *bit_count == 8 {
                *bit_count = 0;
            }
        }
        BitOrder::MostSignificantBitFirst => {
            if *bit_count == 0 {
                out.push(0);
            }
            let idx = out.len() - 1;
            out[idx] = (out[idx] << 1) | (bit & 1);
            *bit_count += 1;
            if *bit_count == 8 {
                *bit_count = 0;
            }
        }
    }
}

fn five_bit_packed_code(ch: char, spec: &BitsCharsetSpec) -> Option<usize> {
    if spec.width == 5 && spec.alphabet == "01234567ABCDEFGHJKLMNPQRSTUVWXYZ" {
        if ch == 'I' {
            return Some(1);
        }
        if ch == 'O' {
            return Some(0);
        }
    }
    spec.alphabet.find(ch)
}

pub(crate) fn encode_bits_charset_text(
    text: &str,
    spec: BitsCharsetSpec,
) -> Result<Vec<u8>, VmError> {
    let mut out = Vec::new();
    let mut bit_count = 0u8;
    for ch in text.chars() {
        let idx = five_bit_packed_code(ch, &spec).ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("character `{ch}` not in bits charset"),
        })? as u8;
        for i in 0..spec.width {
            let bit = match spec.bit_order {
                BitOrder::MostSignificantBitFirst => (idx >> (spec.width - 1 - i)) & 1,
                BitOrder::LeastSignificantBitFirst => (idx >> i) & 1,
            };
            write_bit_to_buffer(&mut out, &mut bit_count, bit, spec.bit_order);
        }
    }
    if bit_count != 0 {
        match spec.bit_order {
            BitOrder::MostSignificantBitFirst => {
                let idx = out.len() - 1;
                out[idx] <<= 8 - bit_count;
            }
            BitOrder::LeastSignificantBitFirst => {}
        }
    }
    Ok(out)
}

pub(crate) fn decode_bits_charset_payload(
    raw: &[u8],
    num_bits: usize,
    spec: BitsCharsetSpec,
) -> Result<String, VmError> {
    let mut out = String::new();
    let mut bit_pos = 0usize;
    while bit_pos + spec.width as usize <= num_bits {
        let mut idx = 0u8;
        for i in 0..spec.width {
            let bit = read_bit_at(raw, bit_pos, spec.bit_order);
            bit_pos += 1;
            match spec.bit_order {
                BitOrder::MostSignificantBitFirst => idx = (idx << 1) | bit,
                BitOrder::LeastSignificantBitFirst => idx |= bit << i,
            }
        }
        let ch = spec
            .alphabet
            .chars()
            .nth(idx as usize)
            .ok_or_else(|| VmError::InvalidValue {
                message: "invalid bits charset code unit".into(),
            })?;
        out.push(ch);
    }
    Ok(out)
}

/// Map DFDL `encoding` values that depend on `byteOrder` to concrete charset names used by the VM.
pub(crate) fn resolve_encoding_with_byte_order(name: &str, byte_order: ByteOrder) -> &str {
    if eq_ascii_ignore_case(name, "utf-16") || eq_ascii_ignore_case(name, "utf_16") {
        match byte_order {
            ByteOrder::LittleEndian => "utf-16le",
            ByteOrder::BigEndian => "utf-16be",
        }
    } else if eq_ascii_ignore_case(name, "utf-32") || eq_ascii_ignore_case(name, "utf_32") {
        // Default UTF-32 document encoding is big-endian unless byteOrder says otherwise.
        match byte_order {
            ByteOrder::LittleEndian => "utf-32le",
            ByteOrder::BigEndian => "utf-32be",
        }
    } else {
        name
    }
}

pub(crate) fn normalize_encoding_name(name: &str) -> Option<&'static str> {
    if eq_ascii_ignore_case(name, "utf-32be") || eq_ascii_ignore_case(name, "utf_32be") {
        Some("utf-32be")
    } else if eq_ascii_ignore_case(name, "utf-32le") || eq_ascii_ignore_case(name, "utf_32le") {
        Some("utf-32le")
    } else if eq_ascii_ignore_case(name, "utf-32") || eq_ascii_ignore_case(name, "utf_32") {
        Some("utf-32be")
    } else if eq_ascii_ignore_case(name, "utf-16be") || eq_ascii_ignore_case(name, "utf_16be") {
        Some("utf-16be")
    } else if eq_ascii_ignore_case(name, "utf-16le") || eq_ascii_ignore_case(name, "utf_16le") {
        Some("utf-16le")
    } else if eq_ascii_ignore_case(name, "utf-8") || eq_ascii_ignore_case(name, "utf8") {
        Some("utf-8")
    } else if eq_ascii_ignore_case(name, "ascii") || eq_ascii_ignore_case(name, "us-ascii") {
        Some("ascii")
    } else if eq_ascii_ignore_case(name, "iso-8859-1")
        || eq_ascii_ignore_case(name, "iso_8859-1")
        || eq_ascii_ignore_case(name, "latin1")
        || eq_ascii_ignore_case(name, "iso8859-1")
    {
        Some("iso-8859-1")
    } else if eq_ascii_ignore_case(name, "ebcdic-cp-us")
        || eq_ascii_ignore_case(name, "cp037")
        || eq_ascii_ignore_case(name, "ibm037")
        || eq_ascii_ignore_case(name, "ebcdic-cp037")
    {
        Some("ebcdic-cp-us")
    } else {
        None
    }
}

/// IBM037 (ebcdic-cp-us): EBCDIC byte → Unicode character (matches Daffodil BitsCharsetIBM037).
const EBCDIC037_DECODE: [char; 256] = [
    '\u{0000}', '\u{0001}', '\u{0002}', '\u{0003}', '\u{009C}', '\u{0009}', '\u{0086}', '\u{007F}',
    '\u{0097}', '\u{008D}', '\u{008E}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{000E}', '\u{000F}',
    '\u{0010}', '\u{0011}', '\u{0012}', '\u{0013}', '\u{009D}', '\u{0085}', '\u{0008}', '\u{0087}',
    '\u{0018}', '\u{0019}', '\u{0092}', '\u{008F}', '\u{001C}', '\u{001D}', '\u{001E}', '\u{001F}',
    '\u{0080}', '\u{0081}', '\u{0082}', '\u{0083}', '\u{0084}', '\u{000A}', '\u{0017}', '\u{001B}',
    '\u{0088}', '\u{0089}', '\u{008A}', '\u{008B}', '\u{008C}', '\u{0005}', '\u{0006}', '\u{0007}',
    '\u{0090}', '\u{0091}', '\u{0016}', '\u{0093}', '\u{0094}', '\u{0095}', '\u{0096}', '\u{0004}',
    '\u{0098}', '\u{0099}', '\u{009A}', '\u{009B}', '\u{0014}', '\u{0015}', '\u{009E}', '\u{001A}',
    ' ', '\u{00A0}', '\u{00E2}', '\u{00E4}', '\u{00E0}', '\u{00E1}', '\u{00E3}', '\u{00E5}',
    '\u{00E7}', '\u{00F1}', '\u{00A2}', '.', '<', '(', '+', '|', '&', '\u{00E9}', '\u{00EA}',
    '\u{00EB}', '\u{00E8}', '\u{00ED}', '\u{00EE}', '\u{00EF}', '\u{00EC}', '\u{00DF}', '!', '$',
    '*', ')', ';', '\u{00AC}', '-', '/', '\u{00C2}', '\u{00C4}', '\u{00C0}', '\u{00C1}',
    '\u{00C3}', '\u{00C5}', '\u{00C7}', '\u{00D1}', '\u{00A6}', ',', '%', '_', '>', '?',
    '\u{00F8}', '\u{00C9}', '\u{00CA}', '\u{00CB}', '\u{00C8}', '\u{00CD}', '\u{00CE}', '\u{00CF}',
    '\u{00CC}', '`', ':', '#', '@', '\'', '=', '"', '\u{00D8}', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
    'h', 'i', '\u{00AB}', '\u{00BB}', '\u{00F0}', '\u{00FD}', '\u{00FE}', '\u{00B1}', '\u{00B0}',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', '\u{00AA}', '\u{00BA}', '\u{00E6}', '\u{00B8}',
    '\u{00C6}', '\u{00A4}', '\u{00B5}', '~', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '\u{00A1}',
    '\u{00BF}', '\u{00D0}', '\u{00DD}', '\u{00DE}', '\u{00AE}', '^', '\u{00A3}', '\u{00A5}',
    '\u{00B7}', '\u{00A9}', '\u{00A7}', '\u{00B6}', '\u{00BC}', '\u{00BD}', '\u{00BE}', '[', ']',
    '\u{00AF}', '\u{00A8}', '\u{00B4}', '\u{00D7}', '{', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H',
    'I', '\u{00AD}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00F3}', '\u{00F5}', '}', 'J', 'K',
    'L', 'M', 'N', 'O', 'P', 'Q', 'R', '\u{00B9}', '\u{00FB}', '\u{00FC}', '\u{00F9}', '\u{00FA}',
    '\u{00FF}', '\\', '\u{00F7}', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '\u{00B2}', '\u{00D4}',
    '\u{00D6}', '\u{00D2}', '\u{00D3}', '\u{00D5}', '0', '1', '2', '3', '4', '5', '6', '7', '8',
    '9', '\u{00B3}', '\u{00DB}', '\u{00DC}', '\u{00D9}', '\u{00DA}', '\u{009F}',
];

/// ASCII (0..127) → EBCDIC CP037; `0xFF` = not representable.
const ASCII_TO_EBCDIC037: [u8; 128] = [
    0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0x40, 0x5A, 0x7F, 0x7B, 0x5B, 0x6C, 0x50, 0x7D, 0x4D, 0x5D, 0x5C, 0x4E, 0x6B, 0x60, 0x4B, 0x61,
    0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x5E, 0x4C, 0x7E, 0x6E, 0x6F,
    0x7C, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6,
    0xD7, 0xD8, 0xD9, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xAD, 0xE0, 0xBD, 0x5F, 0x6D,
    0x79, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9A, 0x9B, 0x9C, 0x9D, 0x9E, 0x9F, 0xA8, 0xA9, 0xC0, 0x6A, 0xD0, 0xA1, 0xFF,
];

fn encode_ebcdic_cp_us(text: &str) -> Result<Vec<u8>, VmError> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let cp = c as u32;
        if cp >= 128 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("character U+{cp:04X} not representable in EBCDIC CP037"),
            });
        }
        let eb = ASCII_TO_EBCDIC037[cp as usize];
        if eb == 0xFF {
            return Err(VmError::InvalidValue {
                message: alloc::format!("character `{c}` not representable in EBCDIC CP037"),
            });
        }
        out.push(eb);
    }
    Ok(out)
}

fn decode_ebcdic_cp_us(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| EBCDIC037_DECODE[b as usize])
        .collect()
}

fn encode_latin1(text: &str) -> Result<Vec<u8>, VmError> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let cp = c as u32;
        if cp > 0xff {
            return Err(VmError::InvalidValue {
                message: alloc::format!("character U+{cp:04X} not representable in ISO-8859-1"),
            });
        }
        out.push(cp as u8);
    }
    Ok(out)
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Daffodil `XMLUtils.remapXMLIllegalCharactersToPUA` (CR/CRLF → LF, other C0 → PUA).
pub(crate) fn remap_xml_illegal_characters_to_pua(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        let curr = chars[i];
        let next = if i + 1 < chars.len() {
            chars[i + 1]
        } else {
            '\0'
        };
        match curr {
            '\t' | '\n' => out.push(curr),
            '\r' if next == '\n' => {
                out.push('\n');
                i += 2;
                continue;
            }
            '\r' => out.push('\n'),
            c if (c as u32) < 0x20 => {
                if let Some(pua) = char::from_u32(c as u32 + 0xE000) {
                    out.push(pua);
                }
            }
            c if (0xE000..=0xF8FF).contains(&(c as u32)) => out.push(c),
            c if c as u32 >= 0xFFFE => out.push(c),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

#[allow(dead_code)]
pub(crate) fn is_iso8859_1_encoding(name: &str) -> bool {
    matches!(normalize_encoding_name(name), Some("iso-8859-1"))
}

/// Byte-oriented encodings where Daffodil remaps XML-illegal C0 controls ↔ PUA in the infoset.
pub(crate) fn uses_xml_illegal_char_remap(_encoding: &str) -> bool {
    true
}

/// Reverse of [`remap_xml_illegal_characters_to_pua`] for unparse / infoset input.
pub(crate) fn remap_pua_to_xml_illegal_characters(text: &str) -> String {
    text.chars()
        .map(|c| {
            let cp = c as u32;
            if (0xE000..=0xE01F).contains(&cp) {
                char::from_u32(cp - 0xE000).unwrap_or(c)
            } else if (0xE800..=0xEFFF).contains(&cp) {
                char::from_u32(cp - 0x1000).unwrap_or(c)
            } else if cp == 0xF0FE {
                '\u{FFFE}'
            } else if cp == 0xF0FF {
                '\u{FFFF}'
            } else {
                c
            }
        })
        .collect()
}

fn eq_ascii_ignore_case(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x.eq_ignore_ascii_case(&y))
}

pub(crate) fn encode_document_text(text: &str, encoding: &str) -> Result<Vec<u8>, VmError> {
    if let Some(spec) = bits_charset_spec(encoding) {
        return encode_bits_charset_text(text, spec);
    }
    match normalize_encoding_name(encoding) {
        Some("utf-8") => Ok(text.as_bytes().to_vec()),
        Some("ascii") => Ok(text.as_bytes().to_vec()),
        Some("iso-8859-1") => encode_latin1(text),
        Some("ebcdic-cp-us") => encode_ebcdic_cp_us(text),
        Some("utf-16be") => Ok(encode_utf16be(text)),
        Some("utf-16le") => Ok(encode_utf16le(text)),
        Some("utf-32be") => Ok(encode_utf32be(text)),
        Some("utf-32le") => Ok(encode_utf32le(text)),
        _ => Err(VmError::UnsupportedOperation {
            op: alloc::format!("document encoding `{encoding}`"),
        }),
    }
}

fn encode_utf32be(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 4);
    for ch in text.chars() {
        let u = ch as u32;
        out.push((u >> 24) as u8);
        out.push((u >> 16) as u8);
        out.push((u >> 8) as u8);
        out.push(u as u8);
    }
    out
}

fn encode_utf32le(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 4);
    for ch in text.chars() {
        let u = ch as u32;
        out.push(u as u8);
        out.push((u >> 8) as u8);
        out.push((u >> 16) as u8);
        out.push((u >> 24) as u8);
    }
    out
}

pub(crate) fn decode_text_bytes(
    bytes: &[u8],
    encoding: &str,
    policy: EncodingErrorPolicy,
) -> Result<String, VmError> {
    if let Some(spec) = bits_charset_spec(encoding) {
        let num_bits = bytes.len().saturating_mul(8);
        return decode_bits_charset_payload(bytes, num_bits, spec);
    }
    match normalize_encoding_name(encoding) {
        Some("utf-8") => decode_utf8_text(bytes, policy),
        Some("ascii") => {
            if bytes.iter().any(|b| *b > 0x7f) {
                if let Ok(text) = decode_utf8_text(bytes, policy) {
                    return Ok(text);
                }
                // Daffodil treats `encodingErrorPolicy="error"` as replace for ASCII (not fully implemented).
                if matches!(
                    policy,
                    EncodingErrorPolicy::Replace | EncodingErrorPolicy::Error
                ) {
                    return Ok(bytes
                        .iter()
                        .map(|&b| if b <= 0x7f { b as char } else { '\u{FFFD}' })
                        .collect());
                }
                return Err(VmError::InvalidValue {
                    message: "invalid ASCII".into(),
                });
            }
            Ok(bytes.iter().map(|b| *b as char).collect())
        }
        Some("utf-32be") => decode_utf32(bytes, false),
        Some("utf-16be") => decode_utf16(bytes, false),
        Some("utf-16le") => decode_utf16(bytes, true),
        Some("iso-8859-1") => Ok(decode_latin1(bytes)),
        Some("ebcdic-cp-us") => Ok(decode_ebcdic_cp_us(bytes)),
        _ => Err(VmError::UnsupportedOperation {
            op: alloc::format!("text decoding for encoding `{encoding}`"),
        }),
    }
}

/// Bytes that belong to a delimited text field when reading to end-of-scope.
///
/// Variable-width encodings only consume whole characters; any trailing partial code unit
/// (e.g. one byte of UTF-16) remains on the cursor for parent framing or strict EOS.
pub(crate) fn delimited_payload_byte_length(available_bytes: usize, encoding: &str) -> usize {
    match normalize_encoding_name(encoding) {
        Some("utf-16be") | Some("utf-16le") => available_bytes - (available_bytes % 2),
        _ => available_bytes,
    }
}

pub(crate) fn character_span_byte_length(
    char_count: usize,
    encoding: &str,
) -> Result<usize, VmError> {
    match normalize_encoding_name(encoding) {
        Some("utf-8") | Some("ascii") | Some("iso-8859-1") | Some("ebcdic-cp-us") => Ok(char_count),
        Some("utf-16be") | Some("utf-16le") => {
            char_count.checked_mul(2).ok_or(VmError::InvalidValue {
                message: "character span overflow".into(),
            })
        }
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
    if let Some(spec) = bits_charset_spec(encoding) {
        let num_bits = bytes.len().saturating_mul(8);
        return Ok(num_bits / spec.width as usize);
    }
    match normalize_encoding_name(encoding) {
        Some("utf-8") => count_utf8_characters(bytes, policy),
        Some("ascii") | Some("iso-8859-1") | Some("ebcdic-cp-us") => Ok(bytes.len()),
        Some("utf-16be") => count_utf16_code_units(bytes, "UTF-16BE"),
        Some("utf-16le") => count_utf16_code_units(bytes, "UTF-16LE"),
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
        Some("ascii") | Some("iso-8859-1") | Some("ebcdic-cp-us") => {
            if *pos + n > data.len() {
                return Err(VmError::UnexpectedEof);
            }
            *pos += n;
        }
        Some("utf-16be") | Some("utf-16le") => {
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
            (2, ((b0 & 0x1F) as u32) << 6 | ((b1 & 0x3F) as u32))
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
                ((b0 & 0x0F) as u32) << 12 | ((b1 & 0x3F) as u32) << 6 | ((b2 & 0x3F) as u32),
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

    let ch = char::from_u32(code_point).ok_or(VmError::InvalidValue {
        message: "invalid unicode code point".into(),
    })?;
    Ok((ch, width))
}

fn decode_utf8_text(bytes: &[u8], policy: EncodingErrorPolicy) -> Result<String, VmError> {
    match policy {
        EncodingErrorPolicy::Error => {
            core::str::from_utf8(bytes)
                .map(str::to_string)
                .map_err(|_| VmError::InvalidValue {
                    message: "invalid UTF-8".into(),
                })
        }
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

fn count_utf16_code_units(bytes: &[u8], label: &str) -> Result<usize, VmError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(VmError::InvalidValue {
            message: alloc::format!("invalid {label} byte length"),
        });
    }
    Ok(bytes.len() / 2)
}

fn decode_utf32(bytes: &[u8], le: bool) -> Result<String, VmError> {
    let label = if le { "UTF-32LE" } else { "UTF-32BE" };
    let usable = bytes.len() - (bytes.len() % 4);
    let trailing = &bytes[usable..];
    let bytes = &bytes[..usable];
    let mut out = String::with_capacity(bytes.len() / 4 + if trailing.is_empty() { 0 } else { 1 });
    for chunk in bytes.as_chunks::<4>().0 {
        let unit = if le {
            (chunk[0] as u32)
                | ((chunk[1] as u32) << 8)
                | ((chunk[2] as u32) << 16)
                | ((chunk[3] as u32) << 24)
        } else {
            ((chunk[0] as u32) << 24)
                | ((chunk[1] as u32) << 16)
                | ((chunk[2] as u32) << 8)
                | chunk[3] as u32
        };
        let ch = char::from_u32(unit).ok_or(VmError::InvalidValue {
            message: alloc::format!("invalid {label} code unit `0x{unit:08x}`"),
        })?;
        out.push(ch);
    }
    if !trailing.is_empty() {
        out.push('\u{FFFD}');
    }
    Ok(out)
}

fn decode_utf16(bytes: &[u8], le: bool) -> Result<String, VmError> {
    let label = if le { "UTF-16LE" } else { "UTF-16BE" };
    let usable = bytes.len() - (bytes.len() % 2);
    let bytes = &bytes[..usable];
    let mut out = String::with_capacity(bytes.len() / 2);
    for chunk in bytes.as_chunks::<2>().0 {
        let unit = if le {
            (chunk[0] as u32) | ((chunk[1] as u32) << 8)
        } else {
            ((chunk[0] as u32) << 8) | chunk[1] as u32
        };
        let ch = char::from_u32(unit).ok_or(VmError::InvalidValue {
            message: alloc::format!("invalid {label} code unit `0x{unit:04x}`"),
        })?;
        out.push(ch);
    }
    Ok(out)
}

fn encode_utf16le(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for ch in text.chars() {
        let unit = ch as u32;
        out.push((unit & 0xff) as u8);
        out.push((unit >> 8) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::EncodingErrorPolicy;

    #[test]
    fn resolve_utf16_encoding_uses_byte_order() {
        use crate::schema::ByteOrder;
        assert_eq!(
            resolve_encoding_with_byte_order("utf-16", ByteOrder::BigEndian),
            "utf-16be"
        );
        assert_eq!(
            resolve_encoding_with_byte_order("UTF-16", ByteOrder::LittleEndian),
            "utf-16le"
        );
        let be = decode_text_bytes(
            &[0x00, b'A', 0x00, b'B'],
            resolve_encoding_with_byte_order("utf-16", ByteOrder::BigEndian),
            EncodingErrorPolicy::Error,
        )
        .unwrap();
        assert_eq!(be, "AB");
    }

    #[test]
    fn delimited_payload_byte_length_utf16_excludes_orphan_byte() {
        assert_eq!(
            delimited_payload_byte_length(3, "utf-16be"),
            2,
            "field value is one code unit; orphan stays on stream"
        );
        assert_eq!(delimited_payload_byte_length(4, "utf-16be"), 4);
        assert_eq!(delimited_payload_byte_length(5, "utf-8"), 5);
    }

    #[test]
    fn replace_policy_decodes_malformed_utf8_to_replacement_char() {
        let bytes = [0xC2, 0xC2];
        let text = decode_utf8_text(&bytes, EncodingErrorPolicy::Replace).unwrap();
        assert_eq!(text, "\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn replace_policy_counts_malformed_bytes_as_characters() {
        let bytes = [0xC0, 0xA0];
        let count = count_utf8_characters(&bytes, EncodingErrorPolicy::Replace).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn ebcdic_cp_us_document_text_roundtrip_ascii() {
        let bytes = encode_ebcdic_cp_us("y876543012").unwrap();
        assert_eq!(
            bytes,
            [0xA8, 0xF8, 0xF7, 0xF6, 0xF5, 0xF4, 0xF3, 0xF0, 0xF1, 0xF2]
        );
        assert_eq!(decode_ebcdic_cp_us(&bytes), "y876543012");
    }

    #[test]
    fn ebcdic_cp_us_b5_overpunch_char() {
        let bytes = [0xB5, 0xF8, 0xF7, 0xF6, 0xF5, 0xF4, 0xF3, 0xF0, 0xF1, 0xF2];
        let text = decode_ebcdic_cp_us(&bytes);
        let first = text.chars().next().unwrap();
        assert_eq!(
            first as u32, 0x00A7,
            "expected section sign, got U+{:04X}",
            first as u32
        );
        use crate::vm::zoned_text::{zoned_to_number, OverpunchLocation, TextZonedSignStyle};
        let num =
            zoned_to_number(&text, TextZonedSignStyle::Ebcdic, OverpunchLocation::Start).unwrap();
        assert_eq!(num, "-5876543012");
    }
}
