use super::encoding::{
    character_span_byte_length, count_characters, decode_text_bytes, encode_document_text,
    read_character_bytes,
};
use crate::length_validate::{validate_data_length_vm, validate_signed_one_bit_length_vm, DaffodilTunables};
use crate::ir::{IrPrefixLength, IrProgram, IrProps, StringId, StringPool};
use crate::schema::{
    encode_delimiter, match_delimiter, match_length_pattern, BinaryNumberRep, BitOrder, ByteOrder,
    LengthKind, LengthUnits, Representation, TextNumberJustification, TextTrimKind,
};
use alloc::string::ToString;
use alloc::vec;
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
    /// When set, absolute bit index (from start of `data`) that must not be read past.
    pub frame_bit_limit: Option<usize>,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bit_count: 0,
            frame_bit_limit: None,
        }
    }

    pub fn with_frame_bits(data: &'a [u8], frame_bits: usize) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bit_count: 0,
            frame_bit_limit: Some(frame_bits),
        }
    }

    pub fn absolute_bit_index(&self) -> usize {
        self.pos * 8 + self.bit_count as usize
    }

    pub fn is_frame_consumed(&self) -> bool {
        match self.frame_bit_limit {
            Some(limit) => self.absolute_bit_index() >= limit,
            None => self.remaining() == 0 && self.bit_count == 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.is_frame_consumed()
    }

    pub fn advance(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.data.len());
        self.bit_count = 0;
    }

    pub fn slice(&self, n: usize) -> Option<&[u8]> {
        if self.bit_count != 0 {
            return None;
        }
        if self.remaining() >= n {
            Some(&self.data[self.pos..self.pos + n])
        } else {
            None
        }
    }

    pub fn read_bytes(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.bit_count != 0 {
            return None;
        }
        let slice = self.slice(n)?;
        let out = slice.to_vec();
        self.advance(n);
        Some(out)
    }

    pub fn skip_stream_bits(
        &mut self,
        n: usize,
        bit_order: BitOrder,
    ) -> Result<(), crate::error::VmError> {
        for _ in 0..n {
            let _ = self.read_stream_bit(bit_order)?;
        }
        Ok(())
    }

    pub fn skip_to_bit_index(
        &mut self,
        target: usize,
        bit_order: BitOrder,
    ) -> Result<(), crate::error::VmError> {
        let current = self.absolute_bit_index();
        if target > current {
            self.skip_stream_bits(target - current, bit_order)?;
        }
        Ok(())
    }

    pub fn read_stream_bits(
        &mut self,
        n: usize,
        bit_order: BitOrder,
    ) -> Result<u64, crate::error::VmError> {
        use crate::error::VmError;
        if n == 0 {
            return Ok(0);
        }
        if n > 64 {
            return Err(VmError::InvalidValue {
                message: alloc::format!("cannot read more than 64 stream bits at once ({n})"),
            });
        }
        let mut value = 0u64;
        match bit_order {
            BitOrder::MostSignificantBitFirst => {
                for _ in 0..n {
                    value = (value << 1) | self.read_stream_bit(bit_order)?;
                }
            }
            BitOrder::LeastSignificantBitFirst => {
                for i in 0..n {
                    value |= self.read_stream_bit(bit_order)? << i;
                }
            }
        }
        Ok(value)
    }

    pub fn read_stream_bits_as_bytes(
        &mut self,
        n: usize,
        bit_order: BitOrder,
    ) -> Result<Vec<u8>, crate::error::VmError> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let byte_len = n.div_ceil(8);
        let mut out = vec![0u8; byte_len];
        for i in 0..n {
            let bit = self.read_stream_bit(bit_order)? as u8;
            match bit_order {
                BitOrder::LeastSignificantBitFirst => {
                    out[i / 8] |= bit << (i % 8);
                }
                BitOrder::MostSignificantBitFirst => {
                    out[i / 8] |= bit << (7 - (i % 8));
                }
            }
        }
        Ok(out)
    }

    fn read_stream_bit(&mut self, bit_order: BitOrder) -> Result<u64, crate::error::VmError> {
        use crate::error::VmError;
        if let Some(limit) = self.frame_bit_limit {
            if self.absolute_bit_index() >= limit {
                return Err(VmError::UnexpectedEof);
            }
        }
        if self.pos >= self.data.len() {
            return Err(VmError::UnexpectedEof);
        }
        let byte = self.data[self.pos];
        let bit = match bit_order {
            BitOrder::MostSignificantBitFirst => (byte >> (7 - self.bit_count)) & 1,
            BitOrder::LeastSignificantBitFirst => (byte >> self.bit_count) & 1,
        };
        self.bit_count += 1;
        if self.bit_count == 8 {
            self.bit_count = 0;
            self.pos += 1;
        }
        Ok(bit as u64)
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

pub(crate) fn encode_absolute_bit_index(out: &[u8], bit_count: u8) -> usize {
    out.len() * 8 + bit_count as usize
}

pub(crate) fn write_stream_bit(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    bit: u8,
    bit_order: BitOrder,
) {
    match bit_order {
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
            out[idx] |= (bit & 1) << (7 - *bit_count);
            *bit_count += 1;
            if *bit_count == 8 {
                *bit_count = 0;
            }
        }
    }
}

pub(crate) fn write_stream_bits(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: u64,
    n: usize,
    bit_order: BitOrder,
) {
    for i in 0..n {
        let bit = match bit_order {
            BitOrder::MostSignificantBitFirst => ((value >> (n - 1 - i)) & 1) as u8,
            BitOrder::LeastSignificantBitFirst => ((value >> i) & 1) as u8,
        };
        write_stream_bit(out, bit_count, bit, bit_order);
    }
}

pub(crate) fn write_byte_aligned(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    bytes: &[u8],
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if *bit_count != 0 {
        return Err(VmError::InvalidValue {
            message: "unaligned byte write".into(),
        });
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_bits_from_stream(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    src: &[u8],
    n: usize,
    bit_order: BitOrder,
) -> Result<(), crate::error::VmError> {
    let mut cursor = Cursor::new(src);
    for _ in 0..n {
        let bit = cursor.read_stream_bit(bit_order)?;
        write_stream_bit(out, bit_count, bit as u8, bit_order);
    }
    Ok(())
}

fn payload_bit_length(payload: &[u8], payload_bit_count: u8) -> usize {
    encode_absolute_bit_index(payload, payload_bit_count)
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
        return decode_binary_scalar(kind, &bytes, props, strings, None);
    }

    if props.length_kind == LengthKind::Prefixed {
        let bytes = read_prefixed_payload(cursor, props, strings)?;
        return decode_binary_scalar(kind, &bytes, props, strings, None);
    }

    if props.length_units == LengthUnits::Bits {
        let len = binary_bit_length(cursor, kind, props, strings)?;
        if len == 0 && kind != ValueKind::String && kind != ValueKind::HexBinary {
            return Err(VmError::InvalidValue {
                message: "zero-length scalar".into(),
            });
        }
        if kind == ValueKind::String || kind == ValueKind::HexBinary {
            let bytes = cursor.read_stream_bits_as_bytes(len, props.bit_order)?;
            return decode_binary_scalar(kind, &bytes, props, strings, None);
        }
        let raw = cursor.read_stream_bits(len, props.bit_order)?;
        return decode_binary_from_raw_bits(kind, raw, len, props, strings);
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

    decode_binary_scalar(kind, &bytes, props, strings, None)
}

fn decode_binary_scalar(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
    bit_width: Option<usize>,
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
            decode_binary_bytes(kind, bytes, props.byte_order == ByteOrder::LittleEndian, bit_width)
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

fn parse_virtual_decimal(text: &str, virtual_point: u32) -> Result<u64, crate::error::VmError> {
    use crate::error::VmError;
    let trimmed = text.trim();
    if virtual_point == 0 {
        return parse_u64(trimmed);
    }
    let scale = 10u64.pow(virtual_point);
    if let Some((whole, frac)) = trimmed.split_once('.') {
        let w = parse_u64(whole.trim())?;
        let mut frac_part = frac.trim().to_string();
        if frac_part.len() > virtual_point as usize {
            frac_part.truncate(virtual_point as usize);
        } else {
            while frac_part.len() < virtual_point as usize {
                frac_part.push('0');
            }
        }
        let f = parse_u64(&frac_part)?;
        w.checked_mul(scale)
            .and_then(|v| v.checked_add(f))
            .ok_or(VmError::InvalidValue {
                message: "decimal value overflow".into(),
            })
    } else {
        parse_u64(trimmed)?
            .checked_mul(scale)
            .ok_or(VmError::InvalidValue {
                message: "decimal value overflow".into(),
            })
    }
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

fn encode_binary_datetime(
    value: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;

    let pat_id = props.calendar_pattern.ok_or(VmError::InvalidValue {
        message: "dateTime missing calendarPattern".into(),
    })?;
    let pattern = strings.get(pat_id)?;
    let digits = datetime_to_calendar_digits(value, pattern)?;
    let le = props.byte_order == ByteOrder::LittleEndian;
    match props.binary_calendar_rep {
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed => {
            let width = digits.len().div_ceil(2);
            digits_to_bcd_bytes(&digits, width, le)
        }
        BinaryNumberRep::PackedBcd => digits_to_packed_bcd_bytes(&digits, le),
        BinaryNumberRep::Binary => Err(VmError::InvalidValue {
            message: "binary dateTime requires BCD representation".into(),
        }),
    }
}

fn datetime_to_calendar_digits(
    value: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;

    let (date, time) = value.split_once('T').ok_or(VmError::InvalidValue {
        message: alloc::format!("invalid dateTime `{value}`"),
    })?;
    let (year, month, day) = parse_date_parts(date)?;
    let (hour, minute, second) = parse_time_parts(time)?;
    let mut out = alloc::string::String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        let field = match c {
            'y' => year.clone(),
            'M' => month.clone(),
            'd' => day.clone(),
            'H' => hour.clone(),
            'm' => minute.clone(),
            's' => second.clone(),
            other => {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("unsupported calendar field `{other}`"),
                });
            }
        };
        if field.len() != width {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "calendar `{pattern}` expected {width} digits for `{c}`, got `{field}`"
                ),
            });
        }
        out.push_str(&field);
        i += width;
    }
    Ok(out)
}

fn parse_date_parts(date: &str) -> Result<(alloc::string::String, alloc::string::String, alloc::string::String), crate::error::VmError> {
    use crate::error::VmError;
    let mut parts = date.split('-');
    let year = parts
        .next()
        .ok_or(VmError::InvalidValue {
            message: "date missing year".into(),
        })?
        .to_string();
    let month = parts
        .next()
        .ok_or(VmError::InvalidValue {
            message: "date missing month".into(),
        })?
        .to_string();
    let day = parts
        .next()
        .ok_or(VmError::InvalidValue {
            message: "date missing day".into(),
        })?
        .to_string();
    Ok((year, month, day))
}

fn parse_time_parts(time: &str) -> Result<(alloc::string::String, alloc::string::String, alloc::string::String), crate::error::VmError> {
    use crate::error::VmError;
    let mut parts = time.split(':');
    let hour = parts
        .next()
        .ok_or(VmError::InvalidValue {
            message: "time missing hour".into(),
        })?
        .to_string();
    let minute = parts
        .next()
        .ok_or(VmError::InvalidValue {
            message: "time missing minute".into(),
        })?
        .to_string();
    let second = parts
        .next()
        .ok_or(VmError::InvalidValue {
            message: "time missing second".into(),
        })?
        .to_string();
    Ok((hour, minute, second))
}

fn digits_to_bcd_bytes(
    digits: &str,
    width: usize,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let mut padded = digits.to_string();
    while padded.len() < width * 2 {
        padded.insert(0, '0');
    }
    if padded.len() > width * 2 {
        padded = padded[padded.len() - width * 2..].to_string();
    }
    let mut bytes = alloc::vec::Vec::with_capacity(width);
    for chunk in padded.as_bytes().chunks(2) {
        let hi = chunk[0].wrapping_sub(b'0');
        let lo = chunk.get(1).copied().unwrap_or(b'0').wrapping_sub(b'0');
        if hi > 9 || lo > 9 {
            return Err(VmError::InvalidValue {
                message: "invalid BCD digit".into(),
            });
        }
        bytes.push((hi << 4) | lo);
    }
    if le {
        bytes.reverse();
    }
    Ok(bytes)
}

fn digits_to_packed_bcd_bytes(
    digits: &str,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let mut d = digits.to_string();
    if d.len() % 2 == 0 {
        d.insert(0, '0');
    }
    let width = (d.len() + 1).div_ceil(2);
    let mut bytes = alloc::vec![0u8; width];
    for (i, chunk) in d.as_bytes().chunks(2).enumerate() {
        if i >= width {
            break;
        }
        let hi = chunk[0].wrapping_sub(b'0');
        let lo = chunk
            .get(1)
            .copied()
            .unwrap_or(b'0')
            .wrapping_sub(b'0');
        if hi > 9 || lo > 9 {
            return Err(VmError::InvalidValue {
                message: "invalid packed BCD digit".into(),
            });
        }
        bytes[i] = (hi << 4) | lo;
    }
    bytes[width - 1] = (bytes[width - 1] & 0xf0) | 0x0c;
    if le {
        bytes.reverse();
    }
    Ok(bytes)
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
            validate_data_length_vm(kind, len, LengthUnits::Bits)?;
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
            validate_data_length_vm(kind, len, LengthUnits::Bytes)?;
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

fn decode_binary_from_raw_bits(
    kind: crate::ir::ValueKind,
    raw: u64,
    bit_width: usize,
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    if kind == Decimal {
        return Ok(DfdlValue::Decimal(format_virtual_decimal(
            raw,
            props.binary_decimal_virtual_point,
        )));
    }
    if kind == DateTime {
        let bytes = stream_bits_to_bytes(raw, bit_width, props.byte_order);
        return decode_binary_datetime(&bytes, props, strings);
    }

    macro_rules! unsigned {
        ($t:ty, $cons:expr) => {{
            <$t>::try_from(raw).map($cons).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("bit value `{raw}` out of range"),
            })
        }};
    }

    match kind {
        Boolean => Ok(DfdlValue::Boolean(raw != 0)),
        Byte => {
            let v = if bit_width == 1 {
                raw as i8
            } else {
                sign_extend_u64(raw, bit_width) as i8
            };
            Ok(DfdlValue::Byte(v))
        }
        UnsignedByte => unsigned!(u8, DfdlValue::UnsignedByte),
        Short => {
            let v = if bit_width == 1 {
                raw as i16
            } else {
                sign_extend_u64(raw, bit_width) as i16
            };
            Ok(DfdlValue::Short(v))
        }
        UnsignedShort => unsigned!(u16, DfdlValue::UnsignedShort),
        Int => {
            let v = if bit_width == 1 {
                raw as i32
            } else {
                sign_extend_u64(raw, bit_width) as i32
            };
            Ok(DfdlValue::Int(v))
        }
        UnsignedInt => unsigned!(u32, DfdlValue::UnsignedInt),
        Long => {
            let v = if bit_width == 1 {
                raw as i64
            } else {
                sign_extend_u64(raw, bit_width)
            };
            Ok(DfdlValue::Long(v))
        }
        Float => Ok(DfdlValue::Float(f32::from_bits(raw as u32))),
        Double => Ok(DfdlValue::Double(f64::from_bits(raw))),
        Decimal | DateTime => unreachable!("handled above"),
        String | HexBinary | Complex => Err(VmError::TypeMismatch {
            expected: "binary scalar".into(),
        }),
    }
}

fn stream_bits_to_bytes(value: u64, num_bits: usize, byte_order: ByteOrder) -> Vec<u8> {
    let byte_len = num_bits.div_ceil(8);
    let mut out = vec![0u8; byte_len];
    let mut v = value;
    match byte_order {
        ByteOrder::LittleEndian => {
            for byte in out.iter_mut() {
                *byte = (v & 0xff) as u8;
                v >>= 8;
            }
        }
        ByteOrder::BigEndian => {
            for byte in out.iter_mut().rev() {
                *byte = (v & 0xff) as u8;
                v >>= 8;
            }
        }
    }
    out
}

fn sign_extend_u64(value: u64, bits: usize) -> i64 {
    if bits == 0 {
        return 0;
    }
    let sign = 1u64 << (bits - 1);
    if value & sign != 0 {
        let mask = (1u64 << bits) - 1;
        (value | (!mask)) as i64
    } else {
        value as i64
    }
}

fn decode_binary_bytes(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    le: bool,
    bit_width: Option<usize>,
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
        Byte => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Byte(decode_unsigned_binary_bytes(bytes, le) as i8))
            } else if let Some(bits) = bit_width {
                Ok(DfdlValue::Byte(sign_extend_u64(decode_unsigned_binary_bytes(bytes, le), bits) as i8))
            } else {
                Ok(DfdlValue::Byte(int!(i8)))
            }
        }
        UnsignedByte => Ok(DfdlValue::UnsignedByte(int!(u8))),
        Short => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Short(decode_unsigned_binary_bytes(bytes, le) as i16))
            } else if let Some(bits) = bit_width {
                Ok(DfdlValue::Short(sign_extend_u64(decode_unsigned_binary_bytes(bytes, le), bits) as i16))
            } else {
                Ok(DfdlValue::Short(int!(i16)))
            }
        }
        UnsignedShort => Ok(DfdlValue::UnsignedShort(int!(u16))),
        Int => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Int(decode_unsigned_binary_bytes(bytes, le) as i32))
            } else if let Some(bits) = bit_width {
                Ok(DfdlValue::Int(sign_extend_u64(decode_unsigned_binary_bytes(bytes, le), bits) as i32))
            } else {
                Ok(DfdlValue::Int(int!(i32)))
            }
        }
        UnsignedInt => Ok(DfdlValue::UnsignedInt(int!(u32))),
        Long => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Long(decode_unsigned_binary_bytes(bytes, le) as i64))
            } else if let Some(bits) = bit_width {
                Ok(DfdlValue::Long(sign_extend_u64(
                    decode_unsigned_binary_bytes(bytes, le),
                    bits,
                )))
            } else {
                Ok(DfdlValue::Long(int!(i64)))
            }
        }
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
            read_length_span(cursor, len, props.length_units, encoding_name(props, strings)?, props.bit_order)?
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit text missing length".into(),
            })? as usize;
            read_length_span(cursor, len, props.length_units, encoding_name(props, strings)?, props.bit_order)?
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
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    if props.length_kind == LengthKind::Prefixed {
        let payload = encode_binary_payload_bytes(value, kind, props, strings)?;
        return write_prefixed_bytes(out, bit_count, &payload, props, strings);
    }

    if props.length_units == LengthUnits::Bits {
        let n = binary_encode_bit_length(kind, props, tunables)?;
        if n == 0 && kind != String && kind != HexBinary {
            return Err(VmError::InvalidValue {
                message: "zero-length scalar encode".into(),
            });
        }
        let raw = scalar_to_raw_bits(value, kind, props, n)?;
        write_stream_bits(out, bit_count, raw, n, props.bit_order);
        return Ok(());
    }

    let le = props.byte_order == ByteOrder::LittleEndian;
    let size = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.unwrap_or(type_size(kind) as u64);
            validate_data_length_vm(kind, len, LengthUnits::Bytes)?;
            validate_signed_one_bit_length_vm(kind, len, LengthUnits::Bytes, tunables)?;
            len as usize
        }
        LengthKind::Implicit => type_size(kind),
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            validate_data_length_vm(kind, len, LengthUnits::Bytes)?;
            validate_signed_one_bit_length_vm(kind, len, LengthUnits::Bytes, tunables)?;
            len as usize
        }
        LengthKind::Pattern | LengthKind::EndOfParent | LengthKind::Delimited => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!(
                    "lengthKind `{}` on binary scalar encode",
                    length_kind_name(props.length_kind)
                ),
            });
        }
        LengthKind::Prefixed => unreachable!(),
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
        (Decimal, DfdlValue::Decimal(v)) => {
            let raw = parse_virtual_decimal(v, props.binary_decimal_virtual_point)?;
            bytes = stream_bits_to_bytes(raw, size.saturating_mul(8), props.byte_order);
        }
        (expected, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{expected:?}"),
            });
        }
    }

    if le && kind != Decimal {
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
    write_byte_aligned(out, bit_count, &bytes)?;
    Ok(())
}

fn binary_encode_bit_length(
    kind: crate::ir::ValueKind,
    props: &IrProps,
    tunables: &DaffodilTunables,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    match props.length_kind {
        LengthKind::Fixed => Ok(props.length.unwrap_or((type_size(kind) * 8) as u64) as usize),
        LengthKind::Implicit => Ok(type_size(kind) * 8),
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            validate_data_length_vm(kind, len, LengthUnits::Bits)?;
            validate_signed_one_bit_length_vm(kind, len, LengthUnits::Bits, tunables)?;
            Ok(len as usize)
        }
        other => Err(VmError::UnsupportedOperation {
            op: alloc::format!("lengthKind `{}` on bit encode", length_kind_name(other)),
        }),
    }
}

fn scalar_to_raw_bits(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    bit_width: usize,
) -> Result<u64, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    fn signed_raw(value: i64, bit_width: usize) -> u64 {
        if bit_width == 0 {
            return 0;
        }
        if bit_width >= 64 {
            return value as u64;
        }
        let mask = (1u64 << bit_width) - 1;
        (value as u64) & mask
    }

    match (kind, value) {
        (Boolean, DfdlValue::Boolean(v)) => Ok(u64::from(*v)),
        (Byte, DfdlValue::Byte(v)) => Ok(signed_raw(*v as i64, bit_width)),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => Ok(*v as u64),
        (Short, DfdlValue::Short(v)) => Ok(signed_raw(*v as i64, bit_width)),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => Ok(*v as u64),
        (Int, DfdlValue::Int(v)) => Ok(signed_raw(*v as i64, bit_width)),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => Ok(*v as u64),
        (Long, DfdlValue::Long(v)) => Ok(signed_raw(*v, bit_width)),
        (Float, DfdlValue::Float(v)) => Ok(v.to_bits() as u64),
        (Double, DfdlValue::Double(v)) => Ok(v.to_bits()),
        (Decimal, DfdlValue::Decimal(v)) => {
            parse_virtual_decimal(v, props.binary_decimal_virtual_point)
        }
        (expected, _) => Err(VmError::TypeMismatch {
            expected: alloc::format!("{expected:?}"),
        }),
    }
}

pub(crate) fn write_text_scalar(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
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
        (Decimal, DfdlValue::Decimal(v)) => v.clone(),
        (DateTime, DfdlValue::DateTime(v)) => v.clone(),
        (String, DfdlValue::String(v)) => v.clone(),
        (HexBinary, DfdlValue::HexBinary(v)) => encode_hex(v),
        (expected, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{expected:?}"),
            });
        }
    };

    let text = apply_min_length_pad(&text, props, strings, kind);

    if props.length_kind == LengthKind::Prefixed {
        let encoded = encode_document_text(&text, encoding_name(props, strings)?)?;
        return write_prefixed_bytes(out, bit_count, &encoded, props, strings);
    }

    let encoding = encoding_name(props, strings)?;
    let payload = match props.length_kind {
        LengthKind::Fixed | LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed/explicit text missing length".into(),
            })? as usize;
            pad_text_field(&text, len, props.length_units, props, strings, kind, encoding)?
        }
        LengthKind::Delimited | LengthKind::Pattern | LengthKind::Implicit | LengthKind::EndOfParent => {
            encode_document_text(&text, encoding)?
        }
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("text lengthKind `{}` encode", length_kind_name(other)),
            });
        }
    };
    write_byte_aligned(out, bit_count, &payload)?;
    Ok(())
}

fn apply_min_length_pad(
    text: &str,
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
) -> alloc::string::String {
    use crate::ir::ValueKind;
    use crate::schema::TextStringJustification;

    let Some(min_len) = props.min_length else {
        return text.to_string();
    };
    if kind != ValueKind::String {
        return text.to_string();
    }
    let min_len = min_len as usize;
    let current = text.chars().count();
    if current >= min_len {
        return text.to_string();
    }
    let pad_char = pad_char_for_kind(props, strings, kind).unwrap_or(" ");
    let pad_ch = pad_char.chars().next().unwrap_or(' ');
    let pad_count = min_len - current;
    match props.text_string_justification {
        TextStringJustification::Right => {
            let mut out = alloc::string::String::new();
            for _ in 0..pad_count {
                out.push(pad_ch);
            }
            out.push_str(text);
            out
        }
        TextStringJustification::Center => {
            let left = pad_count / 2;
            let right = pad_count - left;
            let mut out = alloc::string::String::new();
            for _ in 0..left {
                out.push(pad_ch);
            }
            out.push_str(text);
            for _ in 0..right {
                out.push(pad_ch);
            }
            out
        }
        TextStringJustification::Left => {
            let mut out = text.to_string();
            for _ in 0..pad_count {
                out.push(pad_ch);
            }
            out
        }
    }
}

fn pad_text_field(
    text: &str,
    len: usize,
    units: LengthUnits,
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
    encoding: &str,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::{LengthUnits, TextStringJustification};

    let pad_char = pad_char_for_kind(props, strings, kind).unwrap_or(" ");
    let pad_byte = pad_char.chars().next().unwrap_or(b' ' as char) as u8;

    match units {
        LengthUnits::Bytes => {
            let mut bytes = text.as_bytes().to_vec();
            if bytes.len() > len {
                bytes.truncate(len);
                return Ok(bytes);
            }
            let pad_count = len - bytes.len();
            match props.text_string_justification {
                TextStringJustification::Right => {
                    bytes.splice(0..0, iter::repeat(pad_byte).take(pad_count));
                }
                TextStringJustification::Center => {
                    let left = pad_count / 2;
                    let right = pad_count - left;
                    bytes.splice(0..0, iter::repeat(pad_byte).take(left));
                    bytes.extend(iter::repeat(pad_byte).take(right));
                }
                TextStringJustification::Left => {
                    bytes.extend(iter::repeat(pad_byte).take(pad_count));
                }
            }
            Ok(bytes)
        }
        LengthUnits::Characters => {
            let current = count_characters(text.as_bytes(), encoding)?;
            if current > len {
                return Err(VmError::InvalidValue {
                    message: "text value too long for explicit character length".into(),
                });
            }
            let mut padded = text.to_string();
            let pad_count = len - current;
            let pad_str: alloc::string::String = pad_char.chars().take(1).collect();
            match props.text_string_justification {
                TextStringJustification::Right => {
                    for _ in 0..pad_count {
                        padded.insert_str(0, &pad_str);
                    }
                }
                TextStringJustification::Center => {
                    let left = pad_count / 2;
                    let right = pad_count - left;
                    for _ in 0..left {
                        padded.insert_str(0, &pad_str);
                    }
                    for _ in 0..right {
                        padded.push_str(&pad_str);
                    }
                }
                TextStringJustification::Left => {
                    for _ in 0..pad_count {
                        padded.push_str(&pad_str);
                    }
                }
            }
            encode_document_text(&padded, encoding)
        }
        LengthUnits::Bits => Err(VmError::UnsupportedOperation {
            op: "explicit text bit length encode".into(),
        }),
    }
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
    read_until_any_delimiter(cursor, &patterns, require_delimiter, &patterns)
}

pub(crate) fn read_until_separator(
    cursor: &mut Cursor<'_>,
    separator: &str,
    require_delimiter: bool,
) -> Result<Vec<u8>, crate::error::VmError> {
    let patterns = [separator.to_string()];
    read_until_any_delimiter(cursor, &patterns, require_delimiter, &patterns)
}

pub(crate) fn read_length_span(
    cursor: &mut Cursor<'_>,
    len: usize,
    units: LengthUnits,
    encoding: &str,
    bit_order: BitOrder,
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
            cursor.bit_count = 0;
            Ok(bytes)
        }
        LengthUnits::Bits => cursor.read_stream_bits_as_bytes(len, bit_order),
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
    patterns_for_error: &[alloc::string::String],
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
        let terms = patterns_for_error
            .iter()
            .map(|p| alloc::format!("`{p}`"))
            .collect::<alloc::vec::Vec<_>>()
            .join(", ");
        return Err(VmError::InvalidValue {
            message: alloc::format!("terminator {terms} not found"),
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
        props.bit_order,
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
    let value = match prefix.props.representation {
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
    }?;
    validate_prefix_facets(value, prefix)?;
    Ok(value)
}

fn validate_prefix_facets(value: u64, prefix: &IrPrefixLength) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if let Some(min) = prefix.min_inclusive {
        if (value as i64) < min {
            return Err(VmError::InvalidValue {
                message: alloc::format!("failed check: facet minInclusive ({min})"),
            });
        }
    }
    if let Some(max) = prefix.max_inclusive {
        if (value as i64) > max {
            return Err(VmError::InvalidValue {
                message: alloc::format!("failed check: facet maxInclusive ({max})"),
            });
        }
    }
    Ok(())
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
                props.bit_order,
            )
        }
        LengthKind::Implicit => {
            if props.representation == Representation::Text {
                Ok(read_numeric_token(cursor))
            } else if props.length_units == LengthUnits::Bits {
                let len = binary_bit_length(cursor, kind, props, strings)?;
                cursor.read_stream_bits_as_bytes(len, props.bit_order)
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

pub(crate) fn write_alignment(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthUnits;

    if props.alignment == 0 {
        return Ok(());
    }
    if props.alignment_units == LengthUnits::Bits {
        let align = props.alignment as usize;
        if align <= 1 {
            return Ok(());
        }
        let pos = encode_absolute_bit_index(out, *bit_count);
        let skip = (align - (pos % align)) % align;
        for _ in 0..skip {
            write_stream_bit(out, bit_count, props.fill_byte & 1, props.bit_order);
        }
        return Ok(());
    }
    if props.alignment_units != LengthUnits::Bytes {
        return Err(VmError::UnsupportedOperation {
            op: "non-byte alignment encode".into(),
        });
    }
    write_byte_aligned(out, bit_count, &[])?;
    let align = props.alignment as usize;
    if align <= 1 {
        return Ok(());
    }
    let skip = (align - (out.len() % align)) % align;
    if skip > 0 {
        out.extend(iter::repeat(props.fill_byte).take(skip));
    }
    Ok(())
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
    if props.alignment_units == LengthUnits::Bits {
        let align = props.alignment as usize;
        if align <= 1 {
            return Ok(());
        }
        let pos = cursor.absolute_bit_index();
        let skip = (align - (pos % align)) % align;
        if skip > 0 {
            cursor.skip_stream_bits(skip, props.bit_order)?;
        }
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

fn encode_binary_payload_bytes(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let le = props.byte_order == ByteOrder::LittleEndian;
    match (kind, value) {
        (String, DfdlValue::String(v)) => Ok(v.as_bytes().to_vec()),
        (HexBinary, DfdlValue::HexBinary(v)) => Ok(v.clone()),
        (Boolean, DfdlValue::Boolean(v)) => Ok(alloc::vec![u8::from(*v)]),
        (Byte, DfdlValue::Byte(v)) => encode_integer_binary(*v as i64, kind, props, le),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le)
        }
        (Short, DfdlValue::Short(v)) => encode_integer_binary(*v as i64, kind, props, le),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le)
        }
        (Int, DfdlValue::Int(v)) => encode_integer_binary(*v as i64, kind, props, le),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le)
        }
        (Long, DfdlValue::Long(v)) => encode_integer_binary(*v, kind, props, le),
        (Float, DfdlValue::Float(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le)
        }
        (Double, DfdlValue::Double(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le)
        }
        (Decimal, DfdlValue::Decimal(v)) => {
            let raw = parse_virtual_decimal(v, props.binary_decimal_virtual_point)?;
            encode_unsigned_binary(raw, kind, props, le)
        }
        (DateTime, DfdlValue::DateTime(v)) => encode_binary_datetime(v, props, strings),
        (expected, _) => Err(VmError::TypeMismatch {
            expected: alloc::format!("{expected:?}"),
        }),
    }
}

fn encode_integer_binary(
    value: i64,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    if props.binary_number_rep == BinaryNumberRep::Binary {
        let width = binary_payload_width(value.unsigned_abs(), kind, props);
        return Ok(int_bytes(value, width, le));
    }
    encode_unsigned_binary(value.unsigned_abs(), kind, props, le)
}

fn encode_unsigned_binary(
    value: u64,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    let width = binary_payload_width(value, kind, props);
    encode_binary_number_u64(value, props.binary_number_rep, width, le)
}

fn binary_payload_width(value: u64, kind: crate::ir::ValueKind, props: &IrProps) -> usize {
    if props.length_kind == LengthKind::Prefixed {
        auto_width_for_rep(value, props.binary_number_rep)
    } else {
        type_size(kind)
    }
}

fn auto_width_for_rep(value: u64, rep: BinaryNumberRep) -> usize {
    let digits = if value == 0 {
        1usize
    } else {
        value.ilog10() as usize + 1
    };
    match rep {
        BinaryNumberRep::Binary => minimal_byte_width(value),
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed => digits.div_ceil(2),
        BinaryNumberRep::PackedBcd => {
            let mut count = digits;
            if count % 2 == 0 {
                count += 1;
            }
            count.div_ceil(2)
        }
    }
}

fn encode_binary_number_u64(
    value: u64,
    rep: BinaryNumberRep,
    width: usize,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    match rep {
        BinaryNumberRep::Binary => Ok(int_bytes(value as i64, width, le)),
        BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed => {
            u64_to_bcd_bytes(value, width, le)
        }
        BinaryNumberRep::PackedBcd => u64_to_packed_bcd_bytes(value, width, le),
    }
}

fn u64_to_bcd_bytes(value: u64, width: usize, le: bool) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let mut digits = alloc::format!("{value:0width$}", width = width * 2);
    if digits.len() > width * 2 {
        digits = digits[digits.len() - width * 2..].to_string();
    }
    while digits.len() < width * 2 {
        digits.insert(0, '0');
    }
    let mut bytes = alloc::vec::Vec::with_capacity(width);
    for chunk in digits.as_bytes().chunks(2) {
        let hi = chunk[0].wrapping_sub(b'0');
        let lo = chunk.get(1).copied().unwrap_or(b'0').wrapping_sub(b'0');
        if hi > 9 || lo > 9 {
            return Err(VmError::InvalidValue {
                message: "invalid BCD digit".into(),
            });
        }
        bytes.push((hi << 4) | lo);
    }
    if le {
        bytes.reverse();
    }
    Ok(bytes)
}

fn u64_to_packed_bcd_bytes(
    value: u64,
    width: usize,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let mut digits = value.to_string();
    if digits.len() % 2 != 0 {
        digits.insert(0, '0');
    }
    while digits.len() < width * 2 - 1 {
        digits.insert(0, '0');
    }
    if digits.len() > width * 2 - 1 {
        digits = digits[digits.len() - (width * 2 - 1)..].to_string();
    }
    let mut bytes = alloc::vec![0u8; width];
    for (i, chunk) in digits.as_bytes().chunks(2).enumerate() {
        let hi = chunk[0].wrapping_sub(b'0');
        let lo = chunk
            .get(1)
            .copied()
            .unwrap_or(b'0')
            .wrapping_sub(b'0');
        if hi > 9 || lo > 9 {
            return Err(VmError::InvalidValue {
                message: "invalid packed BCD digit".into(),
            });
        }
        bytes[i] = (hi << 4) | lo;
    }
    bytes[width - 1] = (bytes[width - 1] & 0xf0) | 0x0c;
    if le {
        bytes.reverse();
    }
    Ok(bytes)
}

fn minimal_byte_width(value: u64) -> usize {
    if value == 0 {
        1
    } else {
        ((u64::BITS - value.leading_zeros()) as usize).div_ceil(8)
    }
}

fn int_bytes(value: i64, size: usize, le: bool) -> alloc::vec::Vec<u8> {
    let mut bytes = value.to_be_bytes().to_vec();
    if bytes.len() > size {
        bytes = bytes[bytes.len() - size..].to_vec();
    } else if bytes.len() < size {
        let pad = size - bytes.len();
        if le {
            bytes.splice(0..0, iter::repeat(0u8).take(pad));
        } else {
            bytes.extend(iter::repeat(0u8).take(pad));
        }
    }
    if le {
        bytes.reverse();
    }
    bytes
}

fn write_prefixed_bytes(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    payload: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let prefix = props
        .prefix_length
        .as_deref()
        .ok_or(VmError::InvalidValue {
            message: "prefixed field missing prefixLengthType".into(),
        })?;
    let encoding = encoding_name(props, strings)?;
    let payload_units = payload_length_units(payload, props.length_units, encoding)?;
    let mut prefix_value = payload_units as u64;
    if props.prefix_includes_prefix_length {
        prefix_value = adjust_prefix_value_for_includes(
            prefix_value,
            payload_units,
            prefix,
            props,
            strings,
            encoding,
        )?;
    }
    write_prefix_field(out, bit_count, prefix_value, prefix, props.length_units, strings)?;
    write_byte_aligned(out, bit_count, payload)?;
    Ok(())
}

fn adjust_prefix_value_for_includes(
    mut prefix_value: u64,
    payload_units: usize,
    prefix: &IrPrefixLength,
    props: &IrProps,
    strings: &StringPool,
    encoding: &str,
) -> Result<u64, crate::error::VmError> {
    use crate::error::VmError;
    if prefix.props.length_kind == LengthKind::Prefixed {
        for _ in 0..4 {
            let mut tmp = alloc::vec::Vec::new();
            let mut tmp_bit_count = 0u8;
            write_prefix_field(
                &mut tmp,
                &mut tmp_bit_count,
                prefix_value,
                prefix,
                props.length_units,
                strings,
            )?;
            let field_units = payload_length_units(&tmp, props.length_units, encoding)?;
            let adjusted = (payload_units as u64)
                .checked_add(field_units as u64)
                .ok_or(VmError::InvalidValue {
                    message: "prefixed length overflow".into(),
                })?;
            if adjusted == prefix_value {
                return Ok(prefix_value);
            }
            prefix_value = adjusted;
        }
        return Ok(prefix_value);
    }
    let prefix_field_units = prefix_field_length_units(prefix, props.length_units)?;
    prefix_value
        .checked_add(prefix_field_units as u64)
        .ok_or(VmError::InvalidValue {
            message: "prefixed length overflow".into(),
        })
}

fn payload_length_units(
    payload: &[u8],
    units: LengthUnits,
    encoding: &str,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    match units {
        LengthUnits::Bytes => Ok(payload.len()),
        LengthUnits::Bits => payload
            .len()
            .checked_mul(8)
            .ok_or(VmError::InvalidValue {
                message: "bit length overflow".into(),
            }),
        LengthUnits::Characters => count_characters(payload, encoding),
    }
}

fn prefix_field_length_units(
    prefix: &IrPrefixLength,
    element_units: LengthUnits,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    match prefix.props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed => {
            let len = prefix.props.length.ok_or(VmError::InvalidValue {
                message: "prefix type missing length".into(),
            })? as usize;
            match element_units {
                LengthUnits::Bytes => match prefix.props.length_units {
                    LengthUnits::Bytes => Ok(len),
                    LengthUnits::Bits => len
                        .checked_div(8)
                        .ok_or(VmError::InvalidValue {
                            message: "prefix bit length not byte-aligned".into(),
                        }),
                    LengthUnits::Characters => Ok(len),
                },
                LengthUnits::Bits => match prefix.props.length_units {
                    LengthUnits::Bits => Ok(len),
                    LengthUnits::Bytes => Ok(len.saturating_mul(8)),
                    LengthUnits::Characters => Ok(len.saturating_mul(8)),
                },
                LengthUnits::Characters => match prefix.props.length_units {
                    LengthUnits::Characters => Ok(len),
                    LengthUnits::Bytes | LengthUnits::Bits => Err(VmError::UnsupportedOperation {
                        op: "character prefix from byte/bit prefix type".into(),
                    }),
                },
            }
        }
        LengthKind::Implicit => Ok(type_size(prefix.kind)),
        other => Err(VmError::UnsupportedOperation {
            op: alloc::format!(
                "prefix lengthKind `{}` encode",
                length_kind_name(other)
            ),
        }),
    }
}

fn prefix_field_byte_length(prefix: &IrPrefixLength) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let len = match prefix.props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed => prefix
            .props
            .length
            .ok_or(VmError::InvalidValue {
                message: "prefix type missing length".into(),
            })? as usize,
        LengthKind::Implicit => type_size(prefix.kind),
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!(
                    "prefix lengthKind `{}` encode",
                    length_kind_name(other)
                ),
            });
        }
    };
    Ok(match prefix.props.length_units {
        LengthUnits::Bits => len.div_ceil(8),
        LengthUnits::Bytes | LengthUnits::Characters => len,
    })
}

fn prefix_is_numeric(kind: crate::ir::ValueKind) -> bool {
    use crate::ir::ValueKind::*;
    matches!(
        kind,
        Boolean
            | Byte
            | Short
            | Int
            | Long
            | UnsignedByte
            | UnsignedShort
            | UnsignedInt
            | Float
            | Double
            | Decimal
    )
}

fn write_prefix_field(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: u64,
    prefix: &IrPrefixLength,
    element_length_units: LengthUnits,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::schema::Representation;
    validate_prefix_facets(value, prefix)?;
    if prefix.props.length_kind == LengthKind::Prefixed {
        let payload = prefix_scalar_payload(value, prefix, strings)?;
        return write_prefixed_bytes(out, bit_count, &payload, &prefix.props, strings);
    }
    match prefix.props.representation {
        Representation::Text => {
            write_text_prefix_field(out, bit_count, value, prefix, element_length_units, strings)
        }
        Representation::Binary => write_binary_prefix_field(out, bit_count, value, prefix, strings),
    }
}

fn prefix_scalar_payload(
    value: u64,
    prefix: &IrPrefixLength,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::schema::Representation;
    let _ = strings;
    match prefix.props.representation {
        Representation::Text => {
            let text = if prefix.props.length_kind == LengthKind::Prefixed && value < 100 {
                alloc::format!("{value:02}")
            } else {
                alloc::format!("{value}")
            };
            Ok(text.into_bytes())
        }
        Representation::Binary => {
            let le = prefix.props.byte_order == ByteOrder::LittleEndian;
            let width = auto_width_for_rep(value, prefix.props.binary_number_rep);
            encode_binary_number_u64(value, prefix.props.binary_number_rep, width, le)
        }
    }
}

fn number_pad_char(
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
) -> char {
    if let Some(pad) = pad_char_from_props(props, strings) {
        let ch = pad.chars().next().unwrap_or(' ');
        if ch != ' ' || !prefix_is_numeric(kind) {
            return ch;
        }
    }
    if prefix_is_numeric(kind) {
        '0'
    } else {
        ' '
    }
}

fn number_pad_char_for_compact_prefix(
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
) -> char {
    let _ = props;
    let _ = strings;
    if prefix_is_numeric(kind) {
        '0'
    } else {
        ' '
    }
}

fn write_text_prefix_field(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: u64,
    prefix: &IrPrefixLength,
    element_length_units: LengthUnits,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let text = alloc::format!("{value}");
    match prefix.props.length_kind {
        LengthKind::Implicit | LengthKind::Delimited => {
            write_byte_aligned(out, bit_count, text.as_bytes())?;
            Ok(())
        }
        LengthKind::Explicit | LengthKind::Fixed => {
            let len = prefix.props.length.ok_or(VmError::InvalidValue {
                message: "text prefix type missing length".into(),
            })? as usize;
            let use_schema_pad = prefix.props.length_units == LengthUnits::Characters
                && element_length_units == LengthUnits::Characters;
            let pad = if use_schema_pad {
                number_pad_char(&prefix.props, strings, prefix.kind)
            } else {
                number_pad_char_for_compact_prefix(&prefix.props, strings, prefix.kind)
            };
            let justification = if use_schema_pad {
                prefix.props.text_number_justification
            } else {
                TextNumberJustification::Right
            };
            let mut padded = text;
            match prefix.props.length_units {
                LengthUnits::Bytes => {
                    if padded.len() > len {
                        return Err(VmError::InvalidValue {
                            message: "prefix value too long".into(),
                        });
                    }
                    let pad_count = len - padded.len();
                    match justification {
                        TextNumberJustification::Right => {
                            for _ in 0..pad_count {
                                padded.insert(0, pad);
                            }
                        }
                        TextNumberJustification::Left => {
                            padded.extend(iter::repeat(pad).take(pad_count));
                        }
                    }
                    write_byte_aligned(out, bit_count, padded.as_bytes())?;
                }
                LengthUnits::Characters => {
                    let encoding = encoding_name(&prefix.props, strings)?;
                    while count_characters(padded.as_bytes(), encoding)? < len {
                        match justification {
                            TextNumberJustification::Right => {
                                padded.insert(0, pad);
                            }
                            TextNumberJustification::Left => {
                                padded.push(pad);
                            }
                        }
                    }
                    if count_characters(padded.as_bytes(), encoding)? > len {
                        return Err(VmError::InvalidValue {
                            message: "prefix value too long".into(),
                        });
                    }
                    write_byte_aligned(
                        out,
                        bit_count,
                        encode_document_text(&padded, encoding)?.as_slice(),
                    )?;
                }
                LengthUnits::Bits => {
                    let byte_len = len.div_ceil(8);
                    while padded.as_bytes().len() < byte_len {
                        padded.insert(0, pad);
                    }
                    let bytes = padded.as_bytes();
                    if bytes.len() > byte_len {
                        return Err(VmError::InvalidValue {
                            message: "prefix value too long".into(),
                        });
                    }
                    write_byte_aligned(out, bit_count, bytes)?;
                }
            }
            Ok(())
        }
        other => Err(VmError::UnsupportedOperation {
            op: alloc::format!(
                "prefix lengthKind `{}` encode",
                length_kind_name(other)
            ),
        }),
    }
}

fn write_binary_prefix_field(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: u64,
    prefix: &IrPrefixLength,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    let _ = strings;
    let byte_len = prefix_field_byte_length(prefix)?;
    let le = prefix.props.byte_order == ByteOrder::LittleEndian;
    let mut bytes = if prefix.props.binary_number_rep == BinaryNumberRep::Binary {
        value.to_be_bytes().to_vec()
    } else {
        let width = auto_width_for_rep(value, prefix.props.binary_number_rep);
        encode_binary_number_u64(value, prefix.props.binary_number_rep, width, le)?
    };
    if bytes.len() > byte_len {
        bytes = bytes[bytes.len() - byte_len..].to_vec();
    } else if bytes.len() < byte_len {
        let pad = byte_len - bytes.len();
        if le {
            bytes.splice(0..0, iter::repeat(0u8).take(pad));
        } else {
            bytes.extend(iter::repeat(0u8).take(pad));
        }
    }
    if le && prefix.props.binary_number_rep == BinaryNumberRep::Binary {
        bytes.reverse();
    }
    write_byte_aligned(out, bit_count, &bytes)?;
    Ok(())
}

pub(crate) fn write_framed_payload(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    payload: &[u8],
    payload_bit_count: u8,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    match props.length_kind {
        LengthKind::Prefixed => write_prefixed_bytes(out, bit_count, payload, props, strings),
        LengthKind::Explicit | LengthKind::Fixed => {
            write_explicit_payload(out, bit_count, payload, payload_bit_count, props, strings)
        }
        LengthKind::Delimited => {
            write_bits_from_stream(
                out,
                bit_count,
                payload,
                payload_bit_length(payload, payload_bit_count),
                props.bit_order,
            )?;
            if let Some(id) = props.terminator {
                write_byte_aligned(out, bit_count, &encode_delimiter(strings.get(id)?))?;
            }
            Ok(())
        }
        LengthKind::Implicit | LengthKind::Pattern | LengthKind::EndOfParent => {
            write_bits_from_stream(
                out,
                bit_count,
                payload,
                payload_bit_length(payload, payload_bit_count),
                props.bit_order,
            )
        }
    }
}

fn write_explicit_payload(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    payload: &[u8],
    payload_bit_count: u8,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let len = props.length.ok_or(VmError::InvalidValue {
        message: "explicit payload missing length".into(),
    })? as usize;
    match props.length_units {
        LengthUnits::Bytes => {
            let mut bytes = payload.to_vec();
            if bytes.len() > len {
                bytes.truncate(len);
            } else if bytes.len() < len {
                let pad = if props.representation == Representation::Text {
                    b' '
                } else {
                    0u8
                };
                bytes.extend(iter::repeat(pad).take(len - bytes.len()));
            }
            write_byte_aligned(out, bit_count, &bytes)?;
            Ok(())
        }
        LengthUnits::Bits => {
            let available = payload_bit_length(payload, payload_bit_count);
            write_bits_from_stream(out, bit_count, payload, available.min(len), props.bit_order)?;
            for _ in available..len {
                write_stream_bit(out, bit_count, 0, props.bit_order);
            }
            Ok(())
        }
        LengthUnits::Characters => {
            let encoding = encoding_name(props, strings)?;
            let mut bytes = payload.to_vec();
            while count_characters(&bytes, encoding)? < len {
                let pad = encode_document_text(" ", encoding)?;
                bytes.extend_from_slice(&pad);
            }
            if count_characters(&bytes, encoding)? > len {
                return Err(VmError::InvalidValue {
                    message: "explicit character payload too long".into(),
                });
            }
            write_byte_aligned(out, bit_count, &bytes)?;
            Ok(())
        }
    }
}

pub(crate) fn coerce_value_for_kind(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    Ok(match (kind, value) {
        (Boolean, v @ DfdlValue::Boolean(_)) => v.clone(),
        (Byte, DfdlValue::Int(v)) => DfdlValue::Byte(i8::try_from(*v).map_err(|_| VmError::InvalidValue {
            message: alloc::format!("value `{v}` out of range for byte"),
        })?),
        (Byte, v @ DfdlValue::Byte(_)) => v.clone(),
        (UnsignedByte, DfdlValue::Int(v)) => DfdlValue::UnsignedByte(u8::try_from(*v).map_err(
            |_| VmError::InvalidValue {
                message: alloc::format!("value `{v}` out of range for unsignedByte"),
            },
        )?),
        (UnsignedByte, v @ DfdlValue::UnsignedByte(_)) => v.clone(),
        (Short, DfdlValue::Int(v)) => DfdlValue::Short(*v as i16),
        (Short, v @ DfdlValue::Short(_)) => v.clone(),
        (UnsignedShort, DfdlValue::Int(v)) => {
            DfdlValue::UnsignedShort(u16::try_from(*v).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("value `{v}` out of range for unsignedShort"),
            })?)
        }
        (UnsignedShort, v @ DfdlValue::UnsignedShort(_)) => v.clone(),
        (Int, v @ DfdlValue::Int(_)) => v.clone(),
        (UnsignedInt, DfdlValue::Int(v)) => {
            DfdlValue::UnsignedInt(u32::try_from(*v).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("value `{v}` out of range for unsignedInt"),
            })?)
        }
        (UnsignedInt, v @ DfdlValue::UnsignedInt(_)) => v.clone(),
        (Long, DfdlValue::Int(v)) => DfdlValue::Long(*v as i64),
        (Long, v @ DfdlValue::Long(_)) => v.clone(),
        (_, v) => v.clone(),
    })
}

pub(crate) fn write_simple(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
) -> Result<(), crate::error::VmError> {
    let value = coerce_value_for_kind(value, kind)?;
    if let Some(id) = props.initiator {
        write_byte_aligned(out, bit_count, &encode_delimiter(strings.get(id)?))?;
    }
    match props.representation {
        Representation::Binary => {
            write_binary_scalar(out, bit_count, &value, kind, props, strings, tunables)?
        }
        Representation::Text => write_text_scalar(out, bit_count, &value, kind, props, strings)?,
    }
    if let Some(id) = props.terminator {
        write_byte_aligned(out, bit_count, &encode_delimiter(strings.get(id)?))?;
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
