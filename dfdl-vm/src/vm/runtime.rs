use crate::ir::{IrProgram, IrProps, StringPool};
use crate::schema::{ByteOrder, LengthKind, LengthUnits, Representation, TextTrimKind};
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

    pub fn match_prefix(&self, prefix: &[u8]) -> bool {
        self.slice(prefix.len())
            .map(|s| s == prefix)
            .unwrap_or(false)
    }

    pub fn consume_prefix(&mut self, prefix: &[u8]) -> bool {
        if self.match_prefix(prefix) {
            self.advance(prefix.len());
            true
        } else {
            false
        }
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

pub(crate) fn read_binary_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    if props.length_units == LengthUnits::Bits {
        return Err(VmError::UnsupportedOperation {
            op: "bit-level decode".into(),
        });
    }

    let size = match props.length_kind {
        LengthKind::Fixed => props.length.unwrap_or(type_size(kind) as u64) as usize,
        LengthKind::Implicit => type_size(kind),
        LengthKind::Explicit | LengthKind::Prefixed | LengthKind::Delimited => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("lengthKind `{}` on scalar", length_kind_name(props.length_kind)),
            });
        }
    };

    if size == 0 {
        return Err(VmError::InvalidValue {
            message: "zero-length scalar".into(),
        });
    }

    let bytes = cursor.read_bytes(size).ok_or(VmError::UnexpectedEof)?;
    let le = props.byte_order == ByteOrder::LittleEndian;

    macro_rules! int {
        ($t:ty, $read:ident) => {{
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

    let value = match kind {
        Boolean => DfdlValue::Boolean(bytes.last().copied().unwrap_or(0) != 0),
        Byte => DfdlValue::Byte(int!(i8, read)),
        UnsignedByte => DfdlValue::UnsignedByte(int!(u8, read)),
        Short => DfdlValue::Short(int!(i16, read)),
        UnsignedShort => DfdlValue::UnsignedShort(int!(u16, read)),
        Int => DfdlValue::Int(int!(i32, read)),
        UnsignedInt => DfdlValue::UnsignedInt(int!(u32, read)),
        Long => DfdlValue::Long(int!(i64, read)),
        Float => {
            let bits = int!(u32, read);
            DfdlValue::Float(f32::from_bits(bits))
        }
        Double => {
            let bits = int!(u64, read);
            DfdlValue::Double(f64::from_bits(bits))
        }
        String | HexBinary | Complex => unreachable!("non-scalar in read_binary_scalar"),
    };
    Ok(value)
}

pub(crate) fn read_text_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let raw = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed text missing length".into(),
            })? as usize;
            cursor
                .read_bytes(len)
                .ok_or(VmError::UnexpectedEof)?
        }
        LengthKind::Delimited => {
            let term = props.terminator.as_deref().ok_or(VmError::InvalidValue {
                message: "delimited text missing terminator".into(),
            })?;
            read_until_delimiter(cursor, term)?
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
        Boolean => {
            let v = matches!(trimmed, "true" | "1");
            Ok(DfdlValue::Boolean(v))
        }
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
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("lengthKind `{}` on scalar encode", length_kind_name(other)),
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
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let text = match (kind, value) {
        (Boolean, DfdlValue::Boolean(v)) => alloc::string::String::from(if *v { "true" } else { "false" }),
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
        LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed text missing length".into(),
            })? as usize;
            if payload.len() > len {
                payload.truncate(len);
            } else if payload.len() < len {
                payload.extend(iter::repeat(b' ').take(len - payload.len()));
            }
            out.extend_from_slice(&payload);
        }
        LengthKind::Delimited => {
            out.extend_from_slice(&payload);
        }
        LengthKind::Implicit => out.extend_from_slice(&payload),
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("text lengthKind `{}` encode", length_kind_name(other)),
            });
        }
    }
    Ok(())
}

fn read_until_delimiter(cursor: &mut Cursor<'_>, delimiter: &[u8]) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let start = cursor.pos;
    while cursor.remaining() >= delimiter.len() {
        if cursor.match_prefix(delimiter) {
            let end = cursor.pos;
            cursor.advance(delimiter.len());
            return Ok(cursor.data[start..end].to_vec());
        }
        cursor.advance(1);
    }
    Err(VmError::UnexpectedEof)
}

fn trim_text(input: &str, kind: TextTrimKind) -> &str {
    use crate::schema::TextTrimKind;
    match kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim => input.trim(),
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
    }
}

pub(crate) fn read_simple(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    if let Some(init) = &props.initiator {
        if !cursor.consume_prefix(init) {
            return Err(crate::error::VmError::InvalidValue {
                message: "initiator mismatch".into(),
            });
        }
    }
    let value = match props.representation {
        Representation::Binary => read_binary_scalar(cursor, kind, props)?,
        Representation::Text => read_text_scalar(cursor, kind, props)?,
    };
    if props.length_kind != LengthKind::Delimited {
        if let Some(term) = &props.terminator {
            if !cursor.consume_prefix(term) {
                return Err(crate::error::VmError::InvalidValue {
                    message: "terminator mismatch".into(),
                });
            }
        }
    }
    Ok(value)
}

pub(crate) fn write_simple(
    out: &mut alloc::vec::Vec<u8>,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    if let Some(init) = &props.initiator {
        out.extend_from_slice(init);
    }
    match props.representation {
        Representation::Binary => write_binary_scalar(out, value, kind, props)?,
        Representation::Text => write_text_scalar(out, value, kind, props)?,
    }
    if let Some(term) = &props.terminator {
        out.extend_from_slice(term);
    }
    Ok(())
}
