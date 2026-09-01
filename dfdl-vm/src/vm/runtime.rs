use super::encoding::{
    character_span_byte_length, count_characters, decode_text_bytes, read_character_bytes,
};
use crate::ir::{IrPrefixLength, IrProgram, IrProps, StringId, StringPool};
use crate::schema::{
    encode_delimiter, match_delimiter, match_length_pattern, BinaryNumberRep, ByteOrder, LengthKind,
    LengthUnits, Representation, TextTrimKind,
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
        match match_delimiter(&self.data[self.pos..], pattern) {
            Some(n) => {
                if n > 0 {
                    self.advance(n);
                }
                true
            }
            None => false,
        }
    }
}

pub(crate) fn encoding_name<'a>(
    props: &IrProps,
    strings: &'a StringPool,
) -> Result<&'a str, crate::error::VmError> {
    strings.get(props.encoding)
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
        String | HexBinary | Decimal | DateTime | Complex => 0,
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
    require_delimiter: bool,
    parent_terminator: Option<&str>,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;

    if props.length_kind == LengthKind::Delimited {
        let bytes =
            read_until_delimiters(cursor, props, strings, require_delimiter, parent_terminator)?;
        return decode_binary_scalar(kind, &bytes, props, strings);
    }

    if props.length_kind == LengthKind::Prefixed {
        let bytes = read_prefixed_payload(cursor, props, strings)?;
        return decode_binary_scalar(kind, &bytes, props, strings);
    }

    if props.length_units == LengthUnits::Bits {
        let len = binary_bit_length(cursor, kind, props, strings)?;
        if len == 0 && kind != ValueKind::String && kind != ValueKind::HexBinary {
            return Err(VmError::InvalidValue {
                message: "zero-length scalar".into(),
            });
        }
        let bytes = read_length_span(cursor, len, LengthUnits::Bits, "utf-8")?;
        return decode_binary_scalar(kind, &bytes, props, strings);
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

    decode_binary_scalar(kind, &bytes, props, strings)
}

fn decode_binary_scalar(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::ir::ValueKind;
    use crate::value::DfdlValue;

    if kind == ValueKind::Decimal {
        let le = props.byte_order == ByteOrder::LittleEndian;
        let value = binary_payload_to_u64(props.binary_number_rep, bytes, le)?;
        return Ok(DfdlValue::Decimal(format_virtual_decimal(
            value,
            props.binary_decimal_virtual_point,
        )));
    }
    if kind == ValueKind::DateTime {
        return decode_binary_datetime(bytes, props, strings);
    }

    match props.binary_number_rep {
        BinaryNumberRep::Binary => {
            decode_binary_bytes(kind, bytes, props.byte_order == ByteOrder::LittleEndian)
        }
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed => {
            decode_bcd_number(kind, bytes, props.byte_order == ByteOrder::LittleEndian)
        }
        BinaryNumberRep::PackedBcd => {
            decode_packed_bcd_number(kind, bytes, props.byte_order == ByteOrder::LittleEndian)
        }
    }
}

fn binary_payload_to_u64(
    rep: BinaryNumberRep,
    bytes: &[u8],
    le: bool,
) -> Result<u64, crate::error::VmError> {
    match rep {
        BinaryNumberRep::Binary => Ok(decode_unsigned_binary_bytes(bytes, le)),
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed => bcd_bytes_to_u64(bytes, le),
        BinaryNumberRep::PackedBcd => packed_bcd_bytes_to_u64(bytes, le),
    }
}

fn decode_unsigned_binary_bytes(bytes: &[u8], le: bool) -> u64 {
    let mut value = 0u64;
    if le {
        for (i, byte) in bytes.iter().enumerate() {
            value |= (*byte as u64) << (i * 8);
        }
    } else {
        for byte in bytes {
            value = (value << 8) | (*byte as u64);
        }
    }
    value
}

fn bcd_bytes_to_u64(bytes: &[u8], le: bool) -> Result<u64, crate::error::VmError> {
    use crate::error::VmError;
    let ordered = order_bytes(bytes, le);
    let mut value = 0u64;
    for b in ordered {
        let hi = (b >> 4) & 0x0f;
        let lo = b & 0x0f;
        if hi > 9 || lo > 9 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("invalid BCD byte `0x{b:02x}`"),
            });
        }
        value = value
            .checked_mul(100)
            .and_then(|v| v.checked_add(hi as u64 * 10 + lo as u64))
            .ok_or(VmError::InvalidValue {
                message: "BCD value overflow".into(),
            })?;
    }
    Ok(value)
}

fn packed_bcd_bytes_to_u64(bytes: &[u8], le: bool) -> Result<u64, crate::error::VmError> {
    use crate::error::VmError;
    if bytes.is_empty() {
        return Err(VmError::InvalidValue {
            message: "empty packed BCD".into(),
        });
    }
    let ordered = order_bytes(bytes, le);
    let mut value = 0u64;
    for (i, b) in ordered.iter().enumerate() {
        let hi = (b >> 4) & 0x0f;
        let lo = b & 0x0f;
        if i + 1 == ordered.len() {
            if hi > 9 {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid packed BCD digit `0x{hi:x}`"),
                });
            }
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(hi as u64))
                .ok_or(VmError::InvalidValue {
                    message: "packed BCD value overflow".into(),
                })?;
        } else {
            if hi > 9 || lo > 9 {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid packed BCD byte `0x{b:02x}`"),
                });
            }
            value = value
                .checked_mul(100)
                .and_then(|v| v.checked_add(hi as u64 * 10 + lo as u64))
                .ok_or(VmError::InvalidValue {
                    message: "packed BCD value overflow".into(),
                })?;
        }
    }
    Ok(value)
}

fn order_bytes(bytes: &[u8], le: bool) -> Vec<u8> {
    if le {
        bytes.iter().copied().rev().collect()
    } else {
        bytes.to_vec()
    }
}

fn format_virtual_decimal(value: u64, virtual_point: u32) -> alloc::string::String {
    if virtual_point == 0 {
        return value.to_string();
    }
    let scale = 10u64.pow(virtual_point);
    let whole = value / scale;
    let frac = value % scale;
    alloc::format!("{whole}.{frac:0width$}", width = virtual_point as usize)
}

fn bcd_digit_string(bytes: &[u8], le: bool) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for b in order_bytes(bytes, le) {
        out.push(char::from(b'0' + ((b >> 4) & 0x0f) as u8));
        out.push(char::from(b'0' + (b & 0x0f) as u8));
    }
    out
}

fn packed_digit_string(bytes: &[u8], le: bool) -> alloc::string::String {
    let ordered = order_bytes(bytes, le);
    let mut out = alloc::string::String::new();
    for (i, b) in ordered.iter().enumerate() {
        let hi = (b >> 4) & 0x0f;
        out.push(char::from(b'0' + hi as u8));
        if i + 1 < ordered.len() {
            let lo = b & 0x0f;
            out.push(char::from(b'0' + lo as u8));
        }
    }
    while out.starts_with('0') && out.len() > 1 {
        out.remove(0);
    }
    out
}

fn decode_binary_datetime(
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::value::DfdlValue;

    let le = props.byte_order == ByteOrder::LittleEndian;
    let rep = props.binary_calendar_rep;
    let digits = match rep {
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed => bcd_digit_string(bytes, le),
        BinaryNumberRep::PackedBcd => packed_digit_string(bytes, le),
        BinaryNumberRep::Binary => {
            return Err(VmError::InvalidValue {
                message: "binary dateTime requires BCD representation".into(),
            });
        }
    };
    let pat_id = props.calendar_pattern.ok_or(VmError::InvalidValue {
        message: "dateTime missing calendarPattern".into(),
    })?;
    let pattern = strings.get(pat_id)?;
    Ok(DfdlValue::DateTime(format_calendar_pattern(&digits, pattern)?))
}

fn format_calendar_pattern(
    digits: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;

    let mut di = 0usize;
    let mut fields = alloc::collections::BTreeMap::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        let field: alloc::string::String = digits.chars().skip(di).take(width).collect();
        if field.len() != width {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "calendar `{pattern}` expected {width} digits for `{c}`, got `{field}`"
                ),
            });
        }
        di += width;
        fields.insert(c, field);
        i += width;
    }
    let year = fields.get(&'y').ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing year"),
    })?;
    let month = fields.get(&'M').ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing month"),
    })?;
    let day = fields.get(&'d').ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing day"),
    })?;
    let hour = fields.get(&'H').ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing hour"),
    })?;
    let minute = fields.get(&'m').ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing minute"),
    })?;
    let second = fields.get(&'s').ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing second"),
    })?;
    Ok(alloc::format!("{year}-{month}-{day}T{hour}:{minute}:{second}"))
}

fn decode_bcd_number(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    le: bool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    bcd_value_to_dfdl(kind, bcd_bytes_to_u64(bytes, le)?)
}

fn decode_packed_bcd_number(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    le: bool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    bcd_value_to_dfdl(kind, packed_bcd_bytes_to_u64(bytes, le)?)
}

fn bcd_value_to_dfdl(
    kind: crate::ir::ValueKind,
    value: u64,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    macro_rules! fit {
        ($t:ty, $cons:expr) => {{
            <$t>::try_from(value).map($cons).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("BCD value `{value}` out of range"),
            })
        }};
    }

    match kind {
        Byte => fit!(i8, DfdlValue::Byte),
        UnsignedByte => fit!(u8, DfdlValue::UnsignedByte),
        Short => fit!(i16, DfdlValue::Short),
        UnsignedShort => fit!(u16, DfdlValue::UnsignedShort),
        Int => fit!(i32, DfdlValue::Int),
        UnsignedInt => fit!(u32, DfdlValue::UnsignedInt),
        Long => fit!(i64, DfdlValue::Long),
        other => Err(VmError::TypeMismatch {
            expected: alloc::format!("BCD number for `{other:?}`"),
        }),
    }
}

fn binary_bit_length(
    cursor: &Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;

    match props.length_kind {
        LengthKind::Fixed => Ok(props.length.unwrap_or((type_size(kind) * 8) as u64) as usize),
        LengthKind::Implicit => Ok(type_size(kind) * 8),
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
        LengthKind::EndOfParent => Ok(cursor.remaining().saturating_mul(8)),
        LengthKind::Prefixed | LengthKind::Delimited => Err(VmError::InvalidValue {
            message: "bit length handled before binary_bit_length".into(),
        }),
    }
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
            op: "prefixed binary scalar handled in read_binary_scalar".into(),
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
            let size = core::mem::size_of::<$t>();
            let mut buf = [0u8; core::mem::size_of::<$t>()];
            let n = size.min(bytes.len());
            let src = &bytes[bytes.len() - n..];
            if le {
                buf[..n].copy_from_slice(src);
            } else {
                buf[(size - n)..].copy_from_slice(src);
            }
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
        HexBinary => Ok(DfdlValue::HexBinary(bytes.to_vec())),
        Decimal | DateTime | String | Complex => Err(VmError::TypeMismatch {
            expected: "binary scalar".into(),
        }),
    }
}

pub(crate) fn read_text_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    parent_terminator: Option<&str>,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let raw = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed text missing length".into(),
            })? as usize;
            read_length_span(cursor, len, props.length_units, encoding_name(props, strings)?)?
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit text missing length".into(),
            })? as usize;
            read_length_span(cursor, len, props.length_units, encoding_name(props, strings)?)?
        }
        LengthKind::Delimited => {
            read_until_delimiters(cursor, props, strings, require_delimiter, parent_terminator)?
        }
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
        LengthKind::Implicit => {
            if is_numeric_text_kind(kind) {
                read_numeric_token(cursor)
            } else {
                read_until_delimiters(cursor, props, strings, false, parent_terminator)?
            }
        }
        LengthKind::Prefixed => read_prefixed_payload(cursor, props, strings)?,
        LengthKind::EndOfParent => {
            let rest = cursor.data[cursor.pos..].to_vec();
            cursor.pos = cursor.data.len();
            rest
        }
    };

    let text = decode_text_bytes(&raw, encoding_name(props, strings)?)?;
    let trimmed = trim_text_value(&text, kind, props.text_trim_kind, props, strings);

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
        Decimal => Ok(DfdlValue::Decimal(trimmed.into())),
        DateTime => Ok(DfdlValue::DateTime(trimmed.into())),
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

fn enclosing_delimiter_patterns(
    props: &IrProps,
    strings: &StringPool,
    parent_terminator: Option<&str>,
) -> Result<alloc::vec::Vec<alloc::string::String>, crate::error::VmError> {
    let mut patterns = non_empty_delimiter_patterns(props, strings)?;
    if let Some(term) = parent_terminator {
        if !term.is_empty() && !patterns.iter().any(|p| p == term) {
            patterns.push(term.to_string());
        }
    }
    Ok(patterns)
}

fn read_until_delimiters(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    parent_terminator: Option<&str>,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let patterns = enclosing_delimiter_patterns(props, strings, parent_terminator)?;
    if patterns.is_empty() {
        if require_delimiter {
            return Err(VmError::InvalidValue {
                message: "delimited field missing enclosing delimiter".into(),
            });
        }
        let rest = cursor.data[cursor.pos..].to_vec();
        cursor.pos = cursor.data.len();
        return Ok(rest);
    }
    read_until_any_delimiter(cursor, &patterns, require_delimiter)
}

pub(crate) fn read_until_separator(
    cursor: &mut Cursor<'_>,
    separator: &str,
    require_delimiter: bool,
) -> Result<Vec<u8>, crate::error::VmError> {
    read_until_any_delimiter(cursor, &[separator.to_string()], require_delimiter)
}

pub(crate) fn read_length_span(
    cursor: &mut Cursor<'_>,
    len: usize,
    units: LengthUnits,
    encoding: &str,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    match units {
        LengthUnits::Bytes => cursor
            .read_bytes(len)
            .ok_or(VmError::UnexpectedEof),
        LengthUnits::Characters => {
            let mut pos = cursor.pos;
            let bytes = read_character_bytes(cursor.data, &mut pos, len, encoding)?;
            cursor.pos = pos;
            Ok(bytes)
        }
        LengthUnits::Bits => {
            if cursor.bit_count != 0 {
                return Err(VmError::UnsupportedOperation {
                    op: "unaligned bit-level length span".into(),
                });
            }
            let byte_len = len.div_ceil(8);
            cursor
                .read_bytes(byte_len)
                .ok_or(VmError::UnexpectedEof)
        }
    }
}

pub(crate) fn read_delimited_bytes(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    parent_terminator: Option<&str>,
) -> Result<Vec<u8>, crate::error::VmError> {
    read_until_delimiters(cursor, props, strings, require_delimiter, parent_terminator)
}

fn read_until_any_delimiter(
    cursor: &mut Cursor<'_>,
    delimiters: &[alloc::string::String],
    require_delimiter: bool,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
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
    if require_delimiter {
        return Err(VmError::InvalidValue {
            message: "delimited field missing enclosing delimiter".into(),
        });
    }
    Ok(cursor.data[start..].to_vec())
}

pub(crate) fn consume_enclosing_delimiter(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    parent_terminator: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if cursor.is_empty() {
        return Ok(());
    }
    // Stop boundary matched during read — parent group consumes its own terminator.
    if let Some(term) = parent_terminator {
        if !term.is_empty() {
            if let Some(n) = match_delimiter(&cursor.data[cursor.pos..], term) {
                if n > 0 {
                    return Ok(());
                }
            }
        }
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

pub(crate) fn prefixed_payload_byte_length(
    data: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let mut cursor = Cursor::new(data);
    let span = read_prefixed_span(&mut cursor, props, strings)?;
    match props.length_units {
        LengthUnits::Bytes => Ok(span),
        LengthUnits::Bits => span
            .checked_div(8)
            .ok_or(VmError::InvalidValue {
                message: "prefixed bit span not byte-aligned".into(),
            }),
        LengthUnits::Characters => {
            character_span_byte_length(span, encoding_name(props, strings)?)
        }
    }
}

pub(crate) fn read_prefixed_payload(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Vec<u8>, crate::error::VmError> {
    let span = read_prefixed_span(cursor, props, strings)?;
    read_length_span(
        cursor,
        span,
        props.length_units,
        encoding_name(props, strings)?,
    )
}

fn read_prefixed_span(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let prefix = props
        .prefix_length
        .as_deref()
        .ok_or(VmError::InvalidValue {
            message: "prefixed field missing prefixLengthType".into(),
        })?;
    let prefix_start = cursor.pos;
    let value = read_prefix_integer_value(cursor, prefix, strings)?;
    let prefix_units =
        consumed_length_units(cursor, prefix_start, props.length_units, props, strings)?;
    let mut span = usize_from_u64(value)?;
    if props.prefix_includes_prefix_length {
        span = span.checked_sub(prefix_units).ok_or(VmError::InvalidValue {
            message: "prefixed length smaller than prefix field".into(),
        })?;
    }
    Ok(span)
}

fn consumed_length_units(
    cursor: &Cursor<'_>,
    start: usize,
    units: LengthUnits,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let bytes = cursor.pos.saturating_sub(start);
    match units {
        LengthUnits::Bytes => Ok(bytes),
        LengthUnits::Characters => {
            count_characters(&cursor.data[start..cursor.pos], encoding_name(props, strings)?)
        }
        LengthUnits::Bits => {
            if cursor.bit_count != 0 {
                return Err(VmError::UnsupportedOperation {
                    op: "unaligned bit prefix measurement".into(),
                });
            }
            Ok(bytes.saturating_mul(8))
        }
    }
}

fn read_prefix_integer_value(
    cursor: &mut Cursor<'_>,
    prefix: &IrPrefixLength,
    strings: &StringPool,
) -> Result<u64, crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::Representation;
    let raw = read_prefix_field_payload(cursor, prefix.kind, &prefix.props, strings)?;
    match prefix.props.representation {
        Representation::Text => {
            let text = core::str::from_utf8(&raw).map_err(|_| VmError::InvalidValue {
                message: "invalid UTF-8 in prefix".into(),
            })?;
            let trimmed = trim_numeric_text(
                text,
                prefix.props.text_trim_kind,
                pad_char_from_props(&prefix.props, strings),
            );
            parse_u64(trimmed)
        }
        Representation::Binary => Ok(decode_unsigned_bytes(
            &raw,
            prefix.props.byte_order == ByteOrder::LittleEndian,
        )),
    }
}

fn read_prefix_field_payload(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::Representation;
    match props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "prefix type missing length".into(),
            })? as usize;
            read_length_span(
                cursor,
                len,
                props.length_units,
                encoding_name(props, strings)?,
            )
        }
        LengthKind::Implicit => {
            if props.representation == Representation::Text {
                Ok(read_numeric_token(cursor))
            } else if props.length_units == LengthUnits::Bits {
                let len = binary_bit_length(cursor, kind, props, strings)?;
                read_length_span(cursor, len, LengthUnits::Bits, "utf-8")
            } else {
                let len = binary_byte_length(cursor, kind, props, strings)?;
                cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)
            }
        }
        LengthKind::Prefixed => read_prefixed_payload(cursor, props, strings),
        other => Err(VmError::UnsupportedOperation {
            op: alloc::format!("prefix lengthKind `{}`", length_kind_name(other)),
        }),
    }
}

fn decode_unsigned_bytes(bytes: &[u8], le: bool) -> u64 {
    let mut value = 0u64;
    if le {
        for (i, byte) in bytes.iter().enumerate() {
            value |= (*byte as u64) << (i * 8);
        }
    } else {
        for byte in bytes {
            value = (value << 8) | (*byte as u64);
        }
    }
    value
}

fn parse_u64(s: &str) -> Result<u64, crate::error::VmError> {
    s.parse().map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("invalid non-negative integer `{s}`"),
    })
}

fn usize_from_u64(v: u64) -> Result<usize, crate::error::VmError> {
    usize::try_from(v).map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("length value `{v}` out of range"),
    })
}

fn is_numeric_text_kind(kind: crate::ir::ValueKind) -> bool {
    use crate::ir::ValueKind::*;
    matches!(
        kind,
        Byte | UnsignedByte | Short | UnsignedShort | Int | UnsignedInt | Long | Float | Double
            | Decimal
    )
}

fn read_numeric_token(cursor: &mut Cursor<'_>) -> Vec<u8> {
    let start = cursor.pos;
    if cursor.pos < cursor.data.len() {
        let b = cursor.data[cursor.pos];
        if b == b'+' || b == b'-' {
            cursor.advance(1);
        }
    }
    while cursor.pos < cursor.data.len() && cursor.data[cursor.pos].is_ascii_digit() {
        cursor.advance(1);
    }
    cursor.data[start..cursor.pos].to_vec()
}

fn pad_char_from_props<'a>(props: &IrProps, strings: &'a StringPool) -> Option<&'a str> {
    props
        .text_number_pad_character
        .and_then(|id| strings.get(id).ok())
}

fn pad_char_for_kind<'a>(
    props: &IrProps,
    strings: &'a StringPool,
    kind: crate::ir::ValueKind,
) -> Option<&'a str> {
    use crate::ir::ValueKind::*;
    if matches!(kind, String) {
        if let Some(id) = props.text_string_pad_character {
            if let Ok(ch) = strings.get(id) {
                return Some(ch);
            }
        }
    }
    pad_char_from_props(props, strings)
}

fn trim_text_value<'a>(
    input: &'a str,
    kind: crate::ir::ValueKind,
    trim_kind: crate::schema::TextTrimKind,
    props: &IrProps,
    strings: &StringPool,
) -> &'a str {
    trim_numeric_text(
        input,
        trim_kind,
        pad_char_for_kind(props, strings, kind),
    )
}

fn trim_numeric_text<'a>(input: &'a str, kind: TextTrimKind, pad: Option<&str>) -> &'a str {
    match kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim => input.trim(),
        TextTrimKind::Left => input.trim_start(),
        TextTrimKind::Right => input.trim_end(),
        TextTrimKind::PadChar => trim_pad_char(input, pad.unwrap_or(" ")),
    }
}

fn trim_pad_char<'a>(input: &'a str, pad: &str) -> &'a str {
    if pad.is_empty() {
        return input;
    }
    let mut start = 0usize;
    let mut end = input.len();
    while start < end && input[start..].starts_with(pad) {
        start += pad.len();
    }
    while end > start && input[..end].ends_with(pad) {
        end -= pad.len();
    }
    &input[start..end]
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

pub(crate) fn consume_alignment(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthUnits;

    if props.alignment == 0 {
        return Ok(());
    }
    if props.alignment_units != LengthUnits::Bytes {
        return Err(VmError::UnsupportedOperation {
            op: "non-byte alignment".into(),
        });
    }
    let align = props.alignment as usize;
    if align <= 1 {
        return Ok(());
    }
    let skip = (align - (cursor.pos % align)) % align;
    if skip == 0 {
        return Ok(());
    }
    if cursor.pos + skip > cursor.data.len() {
        return Err(VmError::UnexpectedEof);
    }
    if props.fill_byte != 0 {
        for byte in &cursor.data[cursor.pos..cursor.pos + skip] {
            if *byte != props.fill_byte {
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "alignment fill expected 0x{:02X}, got 0x{:02X}",
                        props.fill_byte,
                        byte
                    ),
                });
            }
        }
    }
    cursor.advance(skip);
    Ok(())
}

pub(crate) fn read_simple(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    parent_terminator: Option<&str>,
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
    let require_enclosing = require_delimiter || props.terminator.is_some();
    use crate::ir::ValueKind;
    use crate::schema::Representation;
    let use_text =
        props.representation == Representation::Text || matches!(kind, ValueKind::String);
    let value = if use_text {
        read_text_scalar(
            cursor,
            kind,
            props,
            strings,
            require_enclosing,
            parent_terminator,
        )?
    } else {
        read_binary_scalar(
            cursor,
            kind,
            props,
            strings,
            require_enclosing,
            parent_terminator,
        )?
    };
    if props.length_kind == LengthKind::Delimited {
        consume_enclosing_delimiter(cursor, props, strings, parent_terminator)?;
    } else if let Some(id) = props.terminator {
        let pat = strings.get(id)?;
        if !pat.is_empty() && !cursor.consume_delimiter(pat) {
            return Err(VmError::InvalidValue {
                message: if cursor.is_empty() {
                    alloc::format!("terminator `{pat}` not found")
                } else {
                    alloc::format!("terminator mismatch: expected `{pat}`").into()
                },
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
        Decimal => Some(DfdlValue::Decimal(raw.into())),
        DateTime => Some(DfdlValue::DateTime(raw.into())),
        String => Some(DfdlValue::String(raw.into())),
        HexBinary => decode_hex(raw).ok().map(DfdlValue::HexBinary),
        Complex => None,
    }
}
