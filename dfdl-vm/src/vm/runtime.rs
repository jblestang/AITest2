use crate::ir::{IrProgram, IrProps, StringId, StringPool};
use crate::schema::{
    encode_delimiter, match_delimiter, match_length_pattern, ByteOrder, LengthKind, LengthUnits,
    Representation, TextTrimKind,
};
use alloc::string::ToString;
use alloc::vec::Vec;
use core::iter;

/// Runtime configuration shared by encoder and decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// When true, the decoder rejects input with leftover bytes after the root value.
    pub strict_eos: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self { strict_eos: true }
    }
}

/// Read/write cursor over a byte slice or output buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub bit_buffer: u8,
    pub bit_count: u8,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bit_count: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0 && self.bit_count == 0
    }

    pub fn advance(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.data.len());
    }

    pub fn slice(&self, n: usize) -> Option<&[u8]> {
        if self.remaining() >= n {
            Some(&self.data[self.pos..self.pos + n])
        } else {
            None
        }
    }

    pub fn read_bytes(&mut self, n: usize) -> Option<Vec<u8>> {
        let slice = self.slice(n)?;
        let out = slice.to_vec();
        self.advance(n);
        Some(out)
    }

    pub fn consume_delimiter(&mut self, pattern: &str) -> bool {
        if pattern.is_empty() {
            return false;
        }
        if let Some(n) = match_delimiter(&self.data[self.pos..], pattern) {
            if n > 0 {
                self.advance(n);
                return true;
            }
        }
        false
    }
}

pub(crate) struct VmContext<'a> {
    pub program: &'a IrProgram,
    pub config: RuntimeConfig,
}

impl<'a> VmContext<'a> {
    pub fn strings(&self) -> &StringPool {
        &self.program.strings
    }
}

pub(crate) fn type_size(kind: crate::ir::ValueKind) -> usize {
    use crate::ir::ValueKind::*;
    match kind {
        Boolean => 1,
        Byte | UnsignedByte => 1,
        Short | UnsignedShort => 2,
        Int | UnsignedInt | Float => 4,
        Long | Double => 8,
        String | HexBinary | Complex => 0,
    }
}

fn pattern_str(strings: &StringPool, id: StringId) -> Result<&str, crate::error::VmError> {
    strings.get(id)
}

pub(crate) fn read_binary_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;

    if props.length_units == LengthUnits::Bits {
        return Err(VmError::UnsupportedOperation {
            op: "bit-level decode".into(),
        });
    }

    if props.length_kind == LengthKind::Delimited {
        let bytes = read_until_delimiters(cursor, props, strings)?;
        return decode_binary_bytes(kind, &bytes, props.byte_order == ByteOrder::LittleEndian);
    }

    let size = binary_byte_length(cursor, kind, props, strings)?;

    if size == 0 && kind != ValueKind::String && kind != ValueKind::HexBinary {
        return Err(VmError::InvalidValue {
            message: "zero-length scalar".into(),
        });
    }

    let bytes = if size == 0 {
        Vec::new()
    } else {
        cursor.read_bytes(size).ok_or(VmError::UnexpectedEof)?
    };

    decode_binary_bytes(kind, &bytes, props.byte_order == ByteOrder::LittleEndian)
}

fn binary_byte_length(
    cursor: &Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;

    match props.length_kind {
        LengthKind::Fixed => Ok(props.length.unwrap_or(type_size(kind) as u64) as usize),
        LengthKind::Implicit => Ok(type_size(kind)),
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            Ok(len as usize)
        }
        LengthKind::Pattern => {
            let id = props.length_pattern.ok_or(VmError::InvalidValue {
                message: "pattern length missing lengthPattern".into(),
            })?;
            let pat = pattern_str(strings, id)?;
            match_length_pattern(&cursor.data[cursor.pos..], pat).ok_or(VmError::InvalidValue {
                message: alloc::format!("pattern `{pat}` mismatch"),
            })
        }
        LengthKind::EndOfParent => Ok(cursor.remaining()),
        LengthKind::Prefixed => Err(VmError::UnsupportedOperation {
            op: alloc::format!(
                "lengthKind `{}` on binary scalar",
                length_kind_name(props.length_kind)
            ),
        }),
        LengthKind::Delimited => Err(VmError::InvalidValue {
            message: "delimited binary handled before byte length".into(),
        }),
    }
}

fn decode_binary_bytes(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    le: bool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    macro_rules! int {
        ($t:ty) => {{
            let mut buf = [0u8; core::mem::size_of::<$t>()];
            let n = core::mem::size_of::<$t>().min(bytes.len());
            buf[..n].copy_from_slice(&bytes[bytes.len() - n..]);
            if le {
                <$t>::from_le_bytes(buf)
            } else {
                <$t>::from_be_bytes(buf)
            }
        }};
    }

    match kind {
        Boolean => Ok(DfdlValue::Boolean(bytes.last().copied().unwrap_or(0) != 0)),
        Byte => Ok(DfdlValue::Byte(int!(i8))),
        UnsignedByte => Ok(DfdlValue::UnsignedByte(int!(u8))),
        Short => Ok(DfdlValue::Short(int!(i16))),
        UnsignedShort => Ok(DfdlValue::UnsignedShort(int!(u16))),
        Int => Ok(DfdlValue::Int(int!(i32))),
        UnsignedInt => Ok(DfdlValue::UnsignedInt(int!(u32))),
        Long => Ok(DfdlValue::Long(int!(i64))),
        Float => Ok(DfdlValue::Float(f32::from_bits(int!(u32)))),
        Double => Ok(DfdlValue::Double(f64::from_bits(int!(u64)))),
        String | HexBinary | Complex => Err(VmError::TypeMismatch {
            expected: "binary scalar".into(),
        }),
    }
}

pub(crate) fn read_text_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let raw = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed text missing length".into(),
            })? as usize;
            cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)?
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit text missing length".into(),
            })? as usize;
            cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)?
        }
        LengthKind::Delimited => read_until_delimiters(cursor, props, strings)?,
        LengthKind::Pattern => {
            let id = props.length_pattern.ok_or(VmError::InvalidValue {
                message: "pattern length missing lengthPattern".into(),
            })?;
            let pat = pattern_str(strings, id)?;
            let len = match_length_pattern(&cursor.data[cursor.pos..], pat).ok_or(
                VmError::InvalidValue {
                    message: alloc::format!("pattern `{pat}` mismatch"),
                },
            )?;
            cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)?
        }
        LengthKind::EndOfParent => {
            let rest = cursor.data[cursor.pos..].to_vec();
            cursor.pos = cursor.data.len();
            rest
        }
        LengthKind::Implicit => {
            let rest = cursor.data[cursor.pos..].to_vec();
            cursor.pos = cursor.data.len();
            rest
        }
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("text lengthKind `{}`", length_kind_name(other)),
            });
        }
    };

    let text = core::str::from_utf8(&raw).map_err(|_| VmError::InvalidValue {
        message: "invalid UTF-8".into(),
    })?;
    let trimmed = trim_text(text, props.text_trim_kind);

    match kind {
        Boolean => parse_text_boolean(trimmed, props, strings).map(DfdlValue::Boolean),
        Byte => parse_int(trimmed).map(DfdlValue::Byte),
        UnsignedByte => parse_int(trimmed).map(DfdlValue::UnsignedByte),
        Short => parse_int(trimmed).map(DfdlValue::Short),
        UnsignedShort => parse_int(trimmed).map(DfdlValue::UnsignedShort),
        Int => parse_int(trimmed).map(DfdlValue::Int),
        UnsignedInt => parse_int(trimmed).map(DfdlValue::UnsignedInt),
        Long => parse_int(trimmed).map(DfdlValue::Long),
        Float => parse_float(trimmed).map(|v| DfdlValue::Float(v as f32)),
        Double => parse_float(trimmed).map(DfdlValue::Double),
        String => Ok(DfdlValue::String(trimmed.into())),
        HexBinary => decode_hex(trimmed).map(DfdlValue::HexBinary),
        Complex => Err(VmError::TypeMismatch {
            expected: "complex".into(),
        }),
    }
}

fn parse_text_boolean(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    if let Some(id) = props.text_boolean_true_rep {
        if trimmed == strings.get(id)? {
            return Ok(true);
        }
    }
    if let Some(id) = props.text_boolean_false_rep {
        if trimmed == strings.get(id)? {
            return Ok(false);
        }
    }
    match trimmed {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("invalid boolean `{trimmed}`"),
        }),
    }
}

pub(crate) fn write_binary_scalar(
    out: &mut alloc::vec::Vec<u8>,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    if props.length_units == LengthUnits::Bits {
        return Err(VmError::UnsupportedOperation {
            op: "bit-level encode".into(),
        });
    }

    let le = props.byte_order == ByteOrder::LittleEndian;
    let size = match props.length_kind {
        LengthKind::Fixed => props.length.unwrap_or(type_size(kind) as u64) as usize,
        LengthKind::Implicit => type_size(kind),
        LengthKind::Explicit => props.length.unwrap_or(type_size(kind) as u64) as usize,
        LengthKind::Pattern | LengthKind::EndOfParent | LengthKind::Delimited | LengthKind::Prefixed => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!(
                    "lengthKind `{}` on binary scalar encode",
                    length_kind_name(props.length_kind)
                ),
            });
        }
    };

    let mut bytes = alloc::vec::Vec::new();
    match (kind, value) {
        (Boolean, DfdlValue::Boolean(v)) => bytes.push(u8::from(*v)),
        (Byte, DfdlValue::Byte(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (Short, DfdlValue::Short(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (Int, DfdlValue::Int(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (Long, DfdlValue::Long(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (Float, DfdlValue::Float(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (Double, DfdlValue::Double(v)) => bytes.extend_from_slice(&v.to_be_bytes()),
        (expected, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{expected:?}"),
            });
        }
    }

    if le {
        bytes.reverse();
    }
    if bytes.len() < size {
        let pad = size - bytes.len();
        let pad_byte = 0u8;
        if le {
            bytes.splice(0..0, iter::repeat(pad_byte).take(pad));
        } else {
            bytes.extend(iter::repeat(pad_byte).take(pad));
        }
    } else if bytes.len() > size {
        bytes = bytes[bytes.len() - size..].to_vec();
    }
    out.extend_from_slice(&bytes);
    Ok(())
}

pub(crate) fn write_text_scalar(
    out: &mut alloc::vec::Vec<u8>,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let text = match (kind, value) {
        (Boolean, DfdlValue::Boolean(v)) => {
            if *v {
                match props.text_boolean_true_rep {
                    Some(id) => strings.get(id)?.to_string(),
                    None => alloc::string::String::from("true"),
                }
            } else {
                match props.text_boolean_false_rep {
                    Some(id) => strings.get(id)?.to_string(),
                    None => alloc::string::String::from("false"),
                }
            }
        }
        (Byte, DfdlValue::Byte(v)) => alloc::format!("{v}"),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => alloc::format!("{v}"),
        (Short, DfdlValue::Short(v)) => alloc::format!("{v}"),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => alloc::format!("{v}"),
        (Int, DfdlValue::Int(v)) => alloc::format!("{v}"),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => alloc::format!("{v}"),
        (Long, DfdlValue::Long(v)) => alloc::format!("{v}"),
        (Float, DfdlValue::Float(v)) => alloc::format!("{v}"),
        (Double, DfdlValue::Double(v)) => alloc::format!("{v}"),
        (String, DfdlValue::String(v)) => v.clone(),
        (HexBinary, DfdlValue::HexBinary(v)) => encode_hex(v),
        (expected, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{expected:?}"),
            });
        }
    };

    let mut payload = text.into_bytes();
    match props.length_kind {
        LengthKind::Fixed | LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed/explicit text missing length".into(),
            })? as usize;
            if payload.len() > len {
                payload.truncate(len);
            } else if payload.len() < len {
                payload.extend(iter::repeat(b' ').take(len - payload.len()));
            }
            out.extend_from_slice(&payload);
        }
        LengthKind::Delimited | LengthKind::Pattern | LengthKind::Implicit | LengthKind::EndOfParent => {
            out.extend_from_slice(&payload);
        }
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("text lengthKind `{}` encode", length_kind_name(other)),
            });
        }
    }
    Ok(())
}

fn delimiter_pattern_ids(props: &IrProps) -> alloc::vec::Vec<StringId> {
    let mut ids = alloc::vec::Vec::new();
    if let Some(t) = props.terminator {
        ids.push(t);
    }
    if let Some(s) = props.separator {
        if !ids.contains(&s) {
            ids.push(s);
        }
    }
    ids
}

fn non_empty_delimiter_patterns(
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<alloc::string::String>, crate::error::VmError> {
    let mut patterns = alloc::vec::Vec::new();
    for id in delimiter_pattern_ids(props) {
        let pat = strings.get(id)?;
        if pat.is_empty() {
            continue;
        }
        patterns.push(pat.to_string());
        // Daffodil delimiter literals like `a aab` also match the suffix token (`aab`).
        if let Some((_, suffix)) = pat.rsplit_once(' ') {
            if !suffix.is_empty() && !patterns.iter().any(|p| p == suffix) {
                patterns.push(suffix.to_string());
            }
        }
    }
    Ok(patterns)
}

fn read_until_delimiters(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let patterns = non_empty_delimiter_patterns(props, strings)?;
    if patterns.is_empty() {
        return Err(VmError::InvalidValue {
            message: "delimited field missing enclosing delimiter".into(),
        });
    }
    read_until_any_delimiter(cursor, &patterns)
}

fn read_until_any_delimiter(
    cursor: &mut Cursor<'_>,
    delimiters: &[alloc::string::String],
) -> Result<Vec<u8>, crate::error::VmError> {
    let start = cursor.pos;
    while cursor.remaining() > 0 {
        for delim in delimiters {
            if let Some(n) = match_delimiter(&cursor.data[cursor.pos..], delim) {
                if n > 0 {
                    return Ok(cursor.data[start..cursor.pos].to_vec());
                }
            }
        }
        cursor.advance(1);
    }
    Ok(cursor.data[start..].to_vec())
}

pub(crate) fn consume_enclosing_delimiter(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if cursor.is_empty() {
        return Ok(());
    }
    for pat in non_empty_delimiter_patterns(props, strings)? {
        if let Some(n) = match_delimiter(&cursor.data[cursor.pos..], &pat) {
            if n > 0 {
                cursor.advance(n);
                return Ok(());
            }
        }
    }
    Err(VmError::InvalidValue {
        message: "delimiter mismatch".into(),
    })
}

fn trim_text(input: &str, kind: TextTrimKind) -> &str {
    use crate::schema::TextTrimKind;
    match kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim | TextTrimKind::PadChar => input.trim(),
        TextTrimKind::Left => input.trim_start(),
        TextTrimKind::Right => input.trim_end(),
    }
}

fn parse_int<T: core::str::FromStr>(s: &str) -> Result<T, crate::error::VmError> {
    s.parse().map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("invalid integer `{s}`"),
    })
}

fn parse_float(s: &str) -> Result<f64, crate::error::VmError> {
    s.parse().map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("invalid float `{s}`"),
    })
}

fn decode_hex(s: &str) -> Result<Vec<u8>, crate::error::VmError> {
    if s.len() % 2 != 0 {
        return Err(crate::error::VmError::InvalidValue {
            message: "invalid hexBinary".into(),
        });
    }
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = (chunk[0] as char).to_digit(16).ok_or_else(|| crate::error::VmError::InvalidValue {
            message: "invalid hexBinary".into(),
        })?;
        let lo = (chunk[1] as char).to_digit(16).ok_or_else(|| crate::error::VmError::InvalidValue {
            message: "invalid hexBinary".into(),
        })?;
        out.push((hi << 4 | lo) as u8);
    }
    Ok(out)
}

fn encode_hex(bytes: &[u8]) -> alloc::string::String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = alloc::string::String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn length_kind_name(kind: LengthKind) -> &'static str {
    match kind {
        LengthKind::Implicit => "implicit",
        LengthKind::Explicit => "explicit",
        LengthKind::Fixed => "fixed",
        LengthKind::Delimited => "delimited",
        LengthKind::Prefixed => "prefixed",
        LengthKind::Pattern => "pattern",
        LengthKind::EndOfParent => "endOfParent",
    }
}

pub(crate) fn read_simple(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;

    if let Some(id) = props.initiator {
        let pat = strings.get(id)?;
        if !pat.is_empty() && !cursor.consume_delimiter(pat) {
            return Err(VmError::InvalidValue {
                message: "initiator mismatch".into(),
            });
        }
    }
    let value = match props.representation {
        Representation::Binary => read_binary_scalar(cursor, kind, props, strings)?,
        Representation::Text => read_text_scalar(cursor, kind, props, strings)?,
    };
    if props.length_kind == LengthKind::Delimited {
        consume_enclosing_delimiter(cursor, props, strings)?;
    } else if let Some(id) = props.terminator {
        let pat = strings.get(id)?;
        if !pat.is_empty() && !cursor.consume_delimiter(pat) {
            return Err(VmError::InvalidValue {
                message: "terminator mismatch".into(),
            });
        }
    }
    Ok(value)
}

pub(crate) fn write_simple(
    out: &mut alloc::vec::Vec<u8>,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    if let Some(id) = props.initiator {
        out.extend(encode_delimiter(strings.get(id)?));
    }
    match props.representation {
        Representation::Binary => write_binary_scalar(out, value, kind, props)?,
        Representation::Text => write_text_scalar(out, value, kind, props, strings)?,
    }
    if let Some(id) = props.terminator {
        out.extend(encode_delimiter(strings.get(id)?));
    }
    Ok(())
}

pub(crate) fn default_value_for(
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Option<crate::value::DfdlValue> {
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let raw = props.default_value.and_then(|id| strings.get(id).ok())?;
    match kind {
        Boolean => parse_text_boolean(raw, props, strings).ok().map(DfdlValue::Boolean),
        Byte => parse_int(raw).ok().map(DfdlValue::Byte),
        UnsignedByte => parse_int(raw).ok().map(DfdlValue::UnsignedByte),
        Short => parse_int(raw).ok().map(DfdlValue::Short),
        UnsignedShort => parse_int(raw).ok().map(DfdlValue::UnsignedShort),
        Int => parse_int(raw).ok().map(DfdlValue::Int),
        UnsignedInt => parse_int(raw).ok().map(DfdlValue::UnsignedInt),
        Long => parse_int(raw).ok().map(DfdlValue::Long),
        Float => parse_float(raw).ok().map(|v| DfdlValue::Float(v as f32)),
        Double => parse_float(raw).ok().map(DfdlValue::Double),
        String => Some(DfdlValue::String(raw.into())),
        HexBinary => decode_hex(raw).ok().map(DfdlValue::HexBinary),
        Complex => None,
    }
}
