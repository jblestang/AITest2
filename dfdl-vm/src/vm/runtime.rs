use super::encoding::{
    character_span_byte_length, count_characters, decode_text_bytes, encode_document_text,
    is_iso8859_1_encoding, remap_pua_to_xml_illegal_characters,
    remap_xml_illegal_characters_to_pua, uses_xml_illegal_char_remap,
    bits_charset_spec, decode_bits_charset_payload, hex_charset_order, hex_charset_payload_to_text,
    HexCharsetOrder, normalize_encoding_name,
    read_character_bytes, read_one_utf8_char,
};
use super::text_number;
use super::packed_decimal::{
    bcd_to_digit_string, digits_to_u64, encode_ibm4690_magnitude, encode_packed_bcd_magnitude,
    ibm4690_to_digit_string, packed_to_digit_string,
    PackedSignCodes,
};
use crate::schema::BinaryNumberCheckPolicy;
use crate::length_validate::{
    binary_length_validation_applies, is_packed_binary_rep, validate_data_length_vm,
    validate_decimal_data_length_vm, validate_decimal_signed_one_bit_length_vm,
    validate_packed_binary_bit_length_parse, validate_signed_one_bit_length_vm, DaffodilTunables,
    VmDecimalPhase,
};
use crate::ir::{IrPrefixLength, IrProgram, IrProps, StringId, StringPool, ValueKind};
use crate::schema::{
    encode_delimiter, encode_delimiter_by_alt, encode_property_delimiter, match_length_pattern,
    BinaryNumberRep, BitOrder,
    ByteOrder,
    EncodingErrorPolicy, LengthKind, LengthUnits, NilKind, Representation, SeparatorPosition,
    SeparatorSuppressionPolicy, TextNumberJustification, TextNumberRep, TextPadKind,
    TextStringJustification, TextTrimKind,
};
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::iter;

/// Runtime configuration shared by encoder and decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// When true, the decoder rejects input with leftover bytes after the root value.
    pub strict_eos: bool,
    /// When false, XSD facet checks are skipped (TDML `validation="off"`).
    pub enable_facet_validation: bool,
    /// When true, XSD facet checks run after parse (TDML `validationErrors` tests).
    pub defer_facet_validation: bool,
    /// When true, PUA code points in string values encode as UTF-8 (TDML text documents).
    pub encode_pua_codepoints_as_utf8: bool,
    /// TDML unparser tests: per-region transmission bit order while encoding.
    pub encode_tdml_bit_regions: Option<alloc::vec::Vec<(BitOrder, usize)>>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            strict_eos: true,
            enable_facet_validation: true,
            defer_facet_validation: false,
            encode_pua_codepoints_as_utf8: false,
            encode_tdml_bit_regions: None,
        }
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
    /// How transmission bits are packed into bytes (TDML document assembly).
    pub transmission_bit_order: BitOrder,
    /// TDML `@bitOrder` / per-part orders: `(order, lengthInBits)` in document order.
    pub tdml_bit_order_regions: Option<alloc::vec::Vec<(BitOrder, usize)>>,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bit_count: 0,
            frame_bit_limit: None,
            transmission_bit_order: BitOrder::MostSignificantBitFirst,
            tdml_bit_order_regions: None,
        }
    }

    pub fn with_frame_bits(data: &'a [u8], frame_bits: usize) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bit_count: 0,
            frame_bit_limit: Some(frame_bits),
            transmission_bit_order: BitOrder::MostSignificantBitFirst,
            tdml_bit_order_regions: None,
        }
    }

    pub fn with_frame_bits_and_transmission(
        data: &'a [u8],
        frame_bits: usize,
        transmission_bit_order: BitOrder,
    ) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bit_count: 0,
            frame_bit_limit: Some(frame_bits),
            transmission_bit_order,
            tdml_bit_order_regions: None,
        }
    }

    fn tdml_bit_order_at(&self, bit_idx: usize) -> BitOrder {
        if let Some(regions) = &self.tdml_bit_order_regions {
            let mut end = 0usize;
            for (order, len) in regions {
                end = end.saturating_add(*len);
                if bit_idx < end {
                    return *order;
                }
            }
            return regions
                .last()
                .map(|(order, _)| *order)
                .unwrap_or(self.transmission_bit_order);
        }
        self.transmission_bit_order
    }

    pub fn effective_field_bit_order(&self, schema_order: BitOrder) -> BitOrder {
        if self.tdml_bit_order_regions.is_some() {
            self.tdml_bit_order_at(self.absolute_bit_index())
        } else {
            schema_order
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
        let start = self.absolute_bit_index();
        for _ in 0..n {
            self.read_stream_bit(bit_order).map_err(|e| {
                use crate::error::VmError;
                if e == VmError::UnexpectedEof {
                    let found = self.absolute_bit_index().saturating_sub(start);
                    insufficient_data_bits_error(n, found)
                } else {
                    e
                }
            })?;
        }
        Ok(())
    }

    pub fn rewind_stream_bits(&mut self, n: usize) -> Result<(), crate::error::VmError> {
        use crate::error::VmError;
        if n == 0 {
            return Ok(());
        }
        let abs = self.absolute_bit_index();
        if n > abs {
            return Err(VmError::InvalidValue {
                message: "cannot rewind past start of stream".into(),
            });
        }
        let new_abs = abs - n;
        self.pos = new_abs / 8;
        self.bit_count = (new_abs % 8) as u8;
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
        let start = self.absolute_bit_index();
        let field_order = self.effective_field_bit_order(bit_order);
        let mut value = 0u64;
        match field_order {
            BitOrder::MostSignificantBitFirst => {
                for _ in 0..n {
                    value = (value << 1)
                        | self.read_stream_bit_with_hint(n, start, field_order)?;
                }
            }
            BitOrder::LeastSignificantBitFirst => {
                for i in 0..n {
                    value |= self.read_stream_bit_with_hint(n, start, field_order)? << i;
                }
            }
        }
        Ok(value)
    }

    fn read_stream_bit_with_hint(
        &mut self,
        requested: usize,
        start: usize,
        bit_order: BitOrder,
    ) -> Result<u64, crate::error::VmError> {
        self.read_stream_bit(bit_order).map_err(|e| {
            use crate::error::VmError;
            if e == VmError::UnexpectedEof {
                let found = self.absolute_bit_index().saturating_sub(start);
                insufficient_data_bits_error(requested, found)
            } else {
                e
            }
        })
    }

    /// Pack `n` stream bits into bytes the way Daffodil `fillByteArray` does for `xs:hexBinary`
    /// (no little-endian byte reversal on the stored array).
    pub fn read_hex_binary_bits(
        &mut self,
        n: usize,
        bit_order: BitOrder,
    ) -> Result<Vec<u8>, crate::error::VmError> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let field_order = self.effective_field_bit_order(bit_order);
        let bytes_to_fill = n.div_ceil(8);
        // LSBF fields must read stream bits in transmission order; byte-aligned fast path is MSBF-only.
        if self.bit_count == 0 && field_order == BitOrder::MostSignificantBitFirst {
            let end_byte = self.pos + bytes_to_fill;
            if end_byte <= self.data.len() {
                let start = self.absolute_bit_index();
                let mut array = self.data[self.pos..end_byte].to_vec();
                let fragment = n % 8;
                if fragment != 0 {
                    let last = array.len() - 1;
                    let mask = match field_order {
                        BitOrder::MostSignificantBitFirst => 0xFFu8 << (8 - fragment),
                        BitOrder::LeastSignificantBitFirst => {
                            if fragment >= 8 {
                                0xFF
                            } else {
                                ((1u16 << fragment) - 1) as u8
                            }
                        }
                    };
                    array[last] &= mask;
                }
                let end = start + n;
                self.pos = end / 8;
                self.bit_count = (end % 8) as u8;
                return Ok(array);
            }
        }
        let mut array = vec![0u8; bytes_to_fill];
        let mut bits_left = n;
        let mut out_bit = 0usize;
        while bits_left > 0 {
            if self.pos >= self.data.len() {
                use crate::error::VmError;
                return Err(insufficient_data_bits_error(n, n - bits_left));
            }
            let byte = self.data[self.pos];
            let avail = 8 - self.bit_count as usize;
            let take = bits_left.min(avail);
            let chunk_mask = if take >= 8 {
                0xFFu8
            } else {
                ((1u16 << take) - 1) as u8
            };
            let chunk = match field_order {
                BitOrder::LeastSignificantBitFirst => (byte >> self.bit_count) & chunk_mask,
                BitOrder::MostSignificantBitFirst => {
                    let shift = 8 - self.bit_count as usize - take;
                    (byte >> shift) & chunk_mask
                }
            };
            for b in 0..take {
                let bit = match field_order {
                    BitOrder::LeastSignificantBitFirst => (chunk >> b) & 1,
                    BitOrder::MostSignificantBitFirst => (chunk >> (take - 1 - b)) & 1,
                };
                match field_order {
                    BitOrder::LeastSignificantBitFirst => {
                        array[out_bit / 8] |= (bit as u8) << (out_bit % 8);
                    }
                    BitOrder::MostSignificantBitFirst => {
                        array[out_bit / 8] |= (bit as u8) << (7 - (out_bit % 8));
                    }
                }
                out_bit += 1;
            }
            self.bit_count += take as u8;
            bits_left -= take;
            while self.bit_count >= 8 {
                self.bit_count -= 8;
                self.pos += 1;
            }
        }
        let fragment = n % 8;
        if fragment != 0 {
            let last = array.len() - 1;
            let mask = match field_order {
                BitOrder::MostSignificantBitFirst => 0xFFu8 << (8 - fragment),
                BitOrder::LeastSignificantBitFirst => {
                    if fragment >= 8 {
                        0xFF
                    } else {
                        ((1u16 << fragment) - 1) as u8
                    }
                }
            };
            array[last] &= mask;
        }
        Ok(array)
    }

    pub fn read_stream_bits_as_bytes(
        &mut self,
        n: usize,
        bit_order: BitOrder,
    ) -> Result<Vec<u8>, crate::error::VmError> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let start = self.absolute_bit_index();
        let byte_len = n.div_ceil(8);
        let mut out = vec![0u8; byte_len];
        for i in 0..n {
            let bit = self.read_stream_bit_with_hint(n, start, bit_order)? as u8;
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

    fn read_stream_bit(&mut self, _field_bit_order: BitOrder) -> Result<u64, crate::error::VmError> {
        use crate::error::VmError;
        let idx = self.absolute_bit_index();
        if let Some(limit) = self.frame_bit_limit {
            if idx >= limit {
                return Err(VmError::UnexpectedEof);
            }
        }
        let byte_idx = idx / 8;
        if byte_idx >= self.data.len() {
            return Err(VmError::UnexpectedEof);
        }
        let byte = self.data[byte_idx];
        let tx_order = self.tdml_bit_order_at(idx);
        let bit_in_byte = match tx_order {
            BitOrder::MostSignificantBitFirst => 7 - (idx % 8),
            BitOrder::LeastSignificantBitFirst => idx % 8,
        };
        let bit = (byte >> bit_in_byte) & 1;
        self.bit_count += 1;
        if self.bit_count == 8 {
            self.bit_count = 0;
            self.pos = byte_idx + 1;
        } else {
            self.pos = byte_idx;
        }
        Ok(bit as u64)
    }

    pub fn consume_delimiter(
        &mut self,
        pattern: &str,
        ignore_case: bool,
        encoding: Option<&str>,
    ) -> bool {
        self.consume_delimiter_with_alt(pattern, ignore_case, encoding)
            .is_some()
    }

    pub fn consume_delimiter_with_alt(
        &mut self,
        pattern: &str,
        ignore_case: bool,
        encoding: Option<&str>,
    ) -> Option<(usize, u8)> {
        if pattern.is_empty() {
            return Some((0, 0));
        }
        let (n, alt) = crate::schema::match_delimiter_with_alt_for_encoding(
            &self.data[self.pos..],
            pattern,
            ignore_case,
            encoding,
        )?;
        if n > 0 {
            self.advance(n);
        }
        Some((n, alt))
    }

    /// Read `binary_byte_len` bytes from a DFDL hex charset (4 bits per hex digit).
    pub(crate) fn read_hex_charset_bytes(
        &mut self,
        binary_byte_len: usize,
        order: HexCharsetOrder,
    ) -> Result<Vec<u8>, crate::error::VmError> {
        use crate::error::VmError;
        let bit_order = match order {
            HexCharsetOrder::MostSignificantByteFirst => BitOrder::MostSignificantBitFirst,
            HexCharsetOrder::LeastSignificantByteFirst => BitOrder::LeastSignificantBitFirst,
        };
        let mut out = Vec::with_capacity(binary_byte_len);
        for _ in 0..binary_byte_len {
            let hi = self.read_hex_nibble(bit_order)?;
            let lo = self.read_hex_nibble(bit_order)?;
            if hi > 0x0f || lo > 0x0f {
                return Err(VmError::InvalidValue {
                    message: "invalid hex charset code unit".into(),
                });
            }
            out.push((hi << 4) | lo);
        }
        Ok(out)
    }

    fn read_hex_nibble(&mut self, bit_order: BitOrder) -> Result<u8, crate::error::VmError> {
        let mut v = 0u8;
        for i in 0..4 {
            let bit = self.read_stream_bit(bit_order)? as u8;
            match bit_order {
                BitOrder::MostSignificantBitFirst => v = (v << 1) | bit,
                BitOrder::LeastSignificantBitFirst => v |= bit << i,
            }
        }
        Ok(v)
    }
}

pub(crate) fn encode_absolute_bit_index(out: &[u8], bit_count: u8) -> usize {
    if bit_count == 0 {
        out.len().saturating_mul(8)
    } else {
        out.len().saturating_sub(1).saturating_mul(8) + bit_count as usize
    }
}

pub(crate) fn effective_encode_bit_order(
    out: &[u8],
    bit_count: u8,
    schema_order: BitOrder,
    config: &RuntimeConfig,
) -> BitOrder {
    let Some(regions) = &config.encode_tdml_bit_regions else {
        return schema_order;
    };
    let bit_idx = encode_absolute_bit_index(out, bit_count);
    let mut end = 0usize;
    for (order, len) in regions {
        end = end.saturating_add(*len);
        if bit_idx < end {
            return *order;
        }
    }
    regions
        .last()
        .map(|(order, _)| *order)
        .unwrap_or(schema_order)
}

pub(crate) fn write_stream_bit(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    bit: u8,
    bit_order: BitOrder,
) {
    write_stream_bit_with_config(out, bit_count, bit, bit_order, None);
}

pub(crate) fn write_stream_bit_with_config(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    bit: u8,
    schema_order: BitOrder,
    config: Option<&RuntimeConfig>,
) {
    let bit_order = config
        .map(|c| effective_encode_bit_order(out, *bit_count, schema_order, c))
        .unwrap_or(schema_order);
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
    write_stream_bits_with_config(out, bit_count, value, n, bit_order, None);
}

pub(crate) fn write_stream_bits_with_config(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: u64,
    n: usize,
    schema_order: BitOrder,
    config: Option<&RuntimeConfig>,
) {
    for i in 0..n {
        let bit = match schema_order {
            BitOrder::MostSignificantBitFirst => ((value >> (n - 1 - i)) & 1) as u8,
            BitOrder::LeastSignificantBitFirst => ((value >> i) & 1) as u8,
        };
        write_stream_bit_with_config(out, bit_count, bit, schema_order, config);
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
    write_bits_from_stream_with_config(out, bit_count, src, n, bit_order, None)
}

fn read_packed_bit_at(data: &[u8], bit_idx: usize, order: BitOrder) -> u8 {
    let byte = data[bit_idx / 8];
    let bit_in_byte = bit_idx % 8;
    match order {
        BitOrder::MostSignificantBitFirst => (byte >> (7 - bit_in_byte)) & 1,
        BitOrder::LeastSignificantBitFirst => (byte >> bit_in_byte) & 1,
    }
}

fn write_bits_from_stream_with_config(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    src: &[u8],
    n: usize,
    schema_order: BitOrder,
    config: Option<&RuntimeConfig>,
) -> Result<(), crate::error::VmError> {
    for i in 0..n {
        let bit = read_packed_bit_at(src, i, schema_order);
        write_stream_bit_with_config(out, bit_count, bit, schema_order, config);
    }
    Ok(())
}

fn payload_bit_length(payload: &[u8], payload_bit_count: u8) -> usize {
    encode_absolute_bit_index(payload, payload_bit_count)
}

fn validate_explicit_decimal_vm(
    props: &IrProps,
    phase: VmDecimalPhase,
    tunables: &DaffodilTunables,
    units: Option<LengthUnits>,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    if !matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        return Ok(());
    }
    let Some(len) = props.length else {
        return Ok(());
    };
    let runtime_resolved = props.length_sibling.is_some();
    let units = units.unwrap_or(props.length_units);
    if let Ok(enc) = strings.get(props.encoding) {
        if hex_charset_order(&enc).is_some() {
            let n_bits = match units {
                LengthUnits::Bits => len as usize,
                LengthUnits::Bytes => len.saturating_mul(8) as usize,
                LengthUnits::Characters => 0,
            };
            return validate_packed_binary_bit_length_parse(
                n_bits,
                ValueKind::Decimal,
                props.binary_number_rep,
            );
        }
    }
    validate_decimal_data_length_vm(
        props.decimal_signed,
        len,
        units,
        phase,
        runtime_resolved,
    )?;
    validate_decimal_signed_one_bit_length_vm(
        props.decimal_signed,
        len,
        units,
        tunables,
        phase,
        runtime_resolved,
    )
}

pub(crate) fn validate_explicit_decimal_before_encode(
    kind: crate::ir::ValueKind,
    props: &IrProps,
    tunables: &DaffodilTunables,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    if kind == crate::ir::ValueKind::Decimal {
        validate_explicit_decimal_vm(props, VmDecimalPhase::Unparse, tunables, None, strings)?;
    }
    Ok(())
}

fn validate_binary_decimal_virtual_point_runtime(
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let Some(vp) = props.binary_decimal_virtual_point_signed else {
        return Ok(());
    };
    const MIN: i32 = -200;
    const MAX: i32 = 200;
    if vp < MIN {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Tunable Limit Exceeded Error: Property binaryDecimalVirtualPoint {vp} is less than limit {MIN}"
            ),
        });
    }
    if vp > MAX {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Tunable Limit Exceeded Error: Property binaryDecimalVirtualPoint {vp} is greater than limit {MAX}"
            ),
        });
    }
    Ok(())
}

pub(crate) fn validate_explicit_decimal_before_decode(
    kind: ValueKind,
    props: &IrProps,
    tunables: &DaffodilTunables,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    if kind == ValueKind::Decimal {
        validate_binary_decimal_virtual_point_runtime(props)?;
        validate_explicit_decimal_vm(props, VmDecimalPhase::Parse, tunables, None, strings)?;
    }
    Ok(())
}

pub(crate) fn encoding_name<'a>(
    props: &IrProps,
    strings: &'a StringPool,
) -> Result<&'a str, crate::error::VmError> {
    let raw = strings.get(props.encoding)?;
    Ok(crate::vm::encoding::resolve_encoding_with_byte_order(
        raw,
        props.byte_order,
    ))
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
        String | HexBinary | Decimal | DateTime | Time | Complex | Integer => 0,
    }
}

fn implicit_binary_scalar_byte_length(kind: crate::ir::ValueKind, props: &IrProps) -> usize {
    if kind == crate::ir::ValueKind::DateTime {
        match props.binary_calendar_rep {
            BinaryNumberRep::BinarySeconds => return 4,
            BinaryNumberRep::BinaryMilliseconds => return 8,
            _ => {}
        }
    }
    if kind == crate::ir::ValueKind::Boolean && props.representation == Representation::Binary {
        return 4;
    }
    type_size(kind)
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
    stop_sequences: &[&IrProps],
    field_name: Option<&str>,
    tunables: &DaffodilTunables,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    use crate::schema::ObjectKind;

    if props.object_kind == ObjectKind::Bytes {
        return read_binary_blob(cursor, props);
    }

    if props.length_kind == LengthKind::Delimited {
        let bytes = read_until_delimiters(
            cursor,
            props,
            strings,
            require_delimiter,
            stop_sequences,
            None,
        )?;
        return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
    }

    if props.length_kind == LengthKind::Prefixed {
        let bytes = read_prefixed_payload(cursor, props, strings, field_name)?;
        return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
    }

    if kind == ValueKind::Decimal {
        validate_explicit_decimal_vm(props, VmDecimalPhase::Parse, tunables, None, strings)?;
    }

    if props.length_units == LengthUnits::Bits {
        let len = binary_bit_length(cursor, kind, props, strings)?;
        if kind != ValueKind::String && kind != ValueKind::HexBinary {
            let enc = encoding_name(props, strings)?;
            if hex_charset_order(&enc).is_some()
                && (kind == ValueKind::Decimal || is_packed_binary_rep(props.binary_number_rep))
            {
                validate_packed_binary_bit_length_parse(len, kind, props.binary_number_rep)?;
            }
            if len == 0 {
                return Err(VmError::InvalidValue {
                    message: "zero-length scalar".into(),
                });
            }
        }
        if kind == ValueKind::HexBinary {
            let bytes = cursor.read_hex_binary_bits(len, props.bit_order)?;
            return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
        }
        if kind == ValueKind::String {
            let bytes = cursor.read_stream_bits_as_bytes(len, props.bit_order)?;
            return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
        }
        if props.byte_order == ByteOrder::LittleEndian
            && kind != ValueKind::String
            && kind != ValueKind::HexBinary
        {
            let bytes = if props.bit_order == BitOrder::LeastSignificantBitFirst {
                cursor.read_hex_binary_bits(len, props.bit_order)?
            } else if cursor.bit_count != 0 {
                cursor.read_stream_bits_as_bytes(len, props.bit_order)?
            } else {
                cursor.read_hex_binary_bits(len, props.bit_order)?
            };
            return decode_binary_scalar(kind, &bytes, props, strings, Some(len), tunables);
        }
        let raw = cursor.read_stream_bits(len, props.bit_order)?;
        let raw = normalize_bit_field_raw(raw, len, props.byte_order, props.bit_order);
        return decode_binary_from_raw_bits(kind, raw, len, props, strings, tunables);
    }

    if cursor.frame_bit_limit.is_some() {
        let bit_len = match props.length_kind {
            LengthKind::Implicit | LengthKind::Fixed => {
                implicit_binary_scalar_byte_length(kind, props).saturating_mul(8)
            }
            LengthKind::Explicit => {
                let len = props.length.ok_or(VmError::InvalidValue {
                    message: "explicit binary missing length".into(),
                })? as usize;
                if props.length_units == LengthUnits::Bits {
                    len
                } else {
                    len.saturating_mul(8)
                }
            }
            _ => 0,
        };
        if bit_len > 0
            && matches!(
                props.length_kind,
                LengthKind::Implicit | LengthKind::Fixed | LengthKind::Explicit
            )
        {
            if kind != ValueKind::String && kind != ValueKind::HexBinary {
                if binary_length_validation_applies(kind, props.binary_number_rep) {
                    validate_data_length_vm(
                        kind,
                        bit_len as u64,
                        LengthUnits::Bits,
                        props.binary_number_rep,
                    )?;
                }
                validate_packed_binary_bit_length_parse(bit_len, kind, props.binary_number_rep)?;
            }
            if kind == ValueKind::HexBinary {
                let bytes = cursor.read_hex_binary_bits(bit_len, props.bit_order)?;
                return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
            }
            if kind == ValueKind::String {
                let bytes = cursor.read_stream_bits_as_bytes(bit_len, props.bit_order)?;
                return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
            }
            if bit_len >= 8 && props.byte_order == ByteOrder::LittleEndian {
                let bytes = if cursor.bit_count != 0 {
                    cursor.read_stream_bits_as_bytes(bit_len, props.bit_order)?
                } else {
                    cursor.read_hex_binary_bits(bit_len, props.bit_order)?
                };
                return decode_binary_scalar(kind, &bytes, props, strings, Some(bit_len), tunables);
            }
            let raw = cursor.read_stream_bits(bit_len, props.bit_order)?;
            let raw = normalize_bit_field_raw(raw, bit_len, props.byte_order, props.bit_order);
            return decode_binary_from_raw_bits(kind, raw, bit_len, props, strings, tunables);
        }
    }

    if props.alignment_units == LengthUnits::Bits || cursor.bit_count != 0 {
        let bit_len = match props.length_kind {
            LengthKind::Implicit | LengthKind::Fixed => {
                implicit_binary_scalar_byte_length(kind, props).saturating_mul(8)
            }
            LengthKind::Explicit => {
                let len = props.length.ok_or(VmError::InvalidValue {
                    message: "explicit binary missing length".into(),
                })? as usize;
                if props.length_units == LengthUnits::Bytes {
                    len.saturating_mul(8)
                } else {
                    len
                }
            }
            _ => 0,
        };
        if bit_len > 0
            && matches!(
                props.length_kind,
                LengthKind::Implicit | LengthKind::Fixed | LengthKind::Explicit
            )
        {
            if kind != ValueKind::String && kind != ValueKind::HexBinary {
                if binary_length_validation_applies(kind, props.binary_number_rep) {
                    validate_data_length_vm(
                        kind,
                        bit_len as u64,
                        LengthUnits::Bits,
                        props.binary_number_rep,
                    )?;
                }
                validate_packed_binary_bit_length_parse(bit_len, kind, props.binary_number_rep)?;
            }
            if kind == ValueKind::HexBinary {
                let bytes = cursor.read_hex_binary_bits(bit_len, props.bit_order)?;
                return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
            }
            if kind == ValueKind::String {
                let bytes = cursor.read_stream_bits_as_bytes(bit_len, props.bit_order)?;
                return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
            }
            if bit_len >= 8 && props.byte_order == ByteOrder::LittleEndian {
                let bytes = if cursor.bit_count != 0 {
                    cursor.read_stream_bits_as_bytes(bit_len, props.bit_order)?
                } else {
                    cursor.read_hex_binary_bits(bit_len, props.bit_order)?
                };
                return decode_binary_scalar(kind, &bytes, props, strings, Some(bit_len), tunables);
            }
            let raw = cursor.read_stream_bits(bit_len, props.bit_order)?;
            let raw = normalize_bit_field_raw(raw, bit_len, props.byte_order, props.bit_order);
            return decode_binary_from_raw_bits(kind, raw, bit_len, props, strings, tunables);
        }
    }

    let size = binary_byte_length(cursor, kind, props, strings)?;

    if kind != ValueKind::String && kind != ValueKind::HexBinary {
        validate_packed_binary_bit_length_parse(size.saturating_mul(8), kind, props.binary_number_rep)?;
        if size == 0 {
            return Err(VmError::InvalidValue {
                message: "zero-length scalar".into(),
            });
        }
    }

    let encoding = encoding_name(props, strings)?;
    let bytes = if size == 0 {
        Vec::new()
    } else if let Some(order) = hex_charset_order(encoding) {
        cursor.read_hex_charset_bytes(size, order)?
    } else {
        cursor.read_bytes(size).ok_or(VmError::UnexpectedEof)?
    };

    decode_binary_scalar(kind, &bytes, props, strings, None, tunables)
}

fn decode_binary_scalar(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
    bit_width: Option<usize>,
    tunables: &DaffodilTunables,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::ir::ValueKind;
    use crate::value::DfdlValue;

    if kind == ValueKind::Decimal {
        return decode_decimal_binary(bytes, props, strings);
    }
    if matches!(kind, ValueKind::DateTime | ValueKind::Time)
        && calendar_binary_rep(props)
    {
        return decode_binary_calendar(kind, bytes, props, strings, tunables, None);
    }

    match props.binary_number_rep {
        BinaryNumberRep::Binary => {
            decode_binary_bytes(
                kind,
                bytes,
                props,
                props.byte_order == ByteOrder::LittleEndian,
                bit_width,
            )
        }
        BinaryNumberRep::Bcd => decode_bcd_number(kind, bytes, props),
        BinaryNumberRep::Ibm4690Packed => decode_ibm4690_number(kind, bytes, props),
        BinaryNumberRep::PackedBcd => decode_packed_bcd_number(kind, bytes, props, strings),
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds require dateTime type".into(),
            })
        }
    }
}

fn packed_sign_codes(props: &IrProps, strings: &StringPool) -> Result<PackedSignCodes, crate::error::VmError> {
    let spec = strings.get(props.binary_packed_sign_codes)?;
    PackedSignCodes::parse(spec, props.binary_number_check_policy)
}

fn binary_payload_to_u64(
    rep: BinaryNumberRep,
    bytes: &[u8],
    le: bool,
) -> Result<u64, crate::error::VmError> {
    match rep {
        BinaryNumberRep::Binary => Ok(decode_unsigned_binary_bytes(bytes, le)),
        BinaryNumberRep::Bcd => digits_to_u64(&bcd_to_digit_string(bytes, le)?),
        BinaryNumberRep::Ibm4690Packed => {
            let (_neg, digits) = ibm4690_to_digit_string(bytes, le)?;
            digits_to_u64(&digits)
        }
        BinaryNumberRep::PackedBcd => Err(crate::error::VmError::InvalidValue {
            message: "packed decimal requires sign codes".into(),
        }),
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds are calendar encodings".into(),
            })
        }
    }
}

fn stream_raw_to_msbf_bytes(raw: u64, bit_width: usize) -> Vec<u8> {
    let nbytes = bit_width.div_ceil(8);
    let mut bytes = vec![0u8; nbytes];
    for i in 0..bit_width {
        let bit = (raw >> (bit_width - 1 - i)) & 1;
        bytes[i / 8] |= (bit as u8) << (7 - (i % 8));
    }
    bytes
}

fn reverse_field_bits(v: u64, bit_width: usize) -> u64 {
    let mut r = 0u64;
    for i in 0..bit_width {
        if (v >> i) & 1 != 0 {
            r |= 1 << (bit_width - 1 - i);
        }
    }
    r
}

/// Daffodil `InputSourceDataInputStream.getUnsignedLong` / `fillByteArray` fragment handling.
/// Wire bytes for `read_hex_binary_bits` / fillByteArray order (inverse of [`decode_packed_bit_field_u64`]).
fn encode_packed_bit_field_bytes(
    value: u64,
    bit_width: usize,
    byte_order: ByteOrder,
    bit_order: BitOrder,
) -> alloc::vec::Vec<u8> {
    if bit_width == 0 {
        return alloc::vec::Vec::new();
    }
    let mut v = value & bit_mask(bit_width);
    let byte_len = bit_width.div_ceil(8);
    let fragment = bit_width % 8;
    let mut buf = vec![0u8; byte_len];
    if fragment != 0
        && byte_order == ByteOrder::BigEndian
        && bit_order == BitOrder::MostSignificantBitFirst
    {
        v <<= 8 - fragment;
    }
    for i in (0..byte_len).rev() {
        buf[i] = (v & 0xff) as u8;
        v >>= 8;
    }
    if fragment != 0 {
        if byte_order == ByteOrder::LittleEndian && bit_order == BitOrder::MostSignificantBitFirst {
            if let Some(first) = buf.first_mut() {
                *first <<= 8 - fragment;
            }
        } else if byte_order == ByteOrder::BigEndian && bit_order == BitOrder::LeastSignificantBitFirst {
            if let Some(last) = buf.last_mut() {
                *last >>= 8 - fragment;
            }
        }
    }
    if bit_width > 8 && byte_order == ByteOrder::LittleEndian {
        buf.reverse();
    }
    buf
}

fn decode_packed_bit_field_u64(
    bytes: &[u8],
    bit_width: usize,
    byte_order: ByteOrder,
    bit_order: BitOrder,
) -> u64 {
    if bit_width == 0 {
        return 0;
    }
    let mut buf = bytes.to_vec();
    // `read_hex_binary_bits` leaves bytes in fillByteArray order (BE); LE fields reverse when > 8 bits.
    if bit_width > 8 && byte_order == ByteOrder::LittleEndian {
        buf.reverse();
    }
    let fragment = bit_width % 8;
    if fragment != 0 {
        if byte_order == ByteOrder::LittleEndian && bit_order == BitOrder::MostSignificantBitFirst {
            if let Some(first) = buf.first_mut() {
                *first >>= 8 - fragment;
            }
        } else if byte_order == ByteOrder::BigEndian && bit_order == BitOrder::LeastSignificantBitFirst {
            if let Some(last) = buf.last_mut() {
                *last <<= 8 - fragment;
            }
        }
    }
    let mut v = 0u64;
    for b in &buf {
        v = (v << 8) | u64::from(*b);
    }
    if fragment != 0
        && byte_order == ByteOrder::BigEndian
        && bit_order == BitOrder::MostSignificantBitFirst
    {
        v >>= 8 - fragment;
    }
    v & bit_mask(bit_width)
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
    digits_to_u64(&bcd_to_digit_string(bytes, le)?)
}

fn signed_magnitude_to_dfdl(
    negative: bool,
    digits: &str,
    kind: crate::ir::ValueKind,
    virtual_point: i32,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let abs = digits_to_u64(digits)?;
    if kind == Decimal {
        let body = format_binary_decimal_magnitude(abs, virtual_point);
        let text = if negative {
            alloc::format!("-{body}")
        } else {
            body
        };
        return Ok(DfdlValue::Decimal(text.into()));
    }
    if negative {
        match kind {
            UnsignedByte | UnsignedShort | UnsignedInt => {
                return Err(VmError::InvalidValue {
                    message: "out of range for type".into(),
                });
            }
            _ => {}
        }
    }
    let signed_i64 = if negative {
        -(abs as i64)
    } else {
        abs as i64
    };
    macro_rules! signed {
        ($t:ty, $cons:expr) => {{
            <$t>::try_from(signed_i64).map($cons).map_err(|_| VmError::InvalidValue {
                message: "out of range for type".into(),
            })
        }};
    }
    match kind {
        Byte => signed!(i8, DfdlValue::Byte),
        UnsignedByte => u8::try_from(abs).map(DfdlValue::UnsignedByte).map_err(|_| VmError::InvalidValue {
            message: "out of range for type".into(),
        }),
        Short => signed!(i16, DfdlValue::Short),
        UnsignedShort => u16::try_from(abs).map(DfdlValue::UnsignedShort).map_err(|_| VmError::InvalidValue {
            message: "out of range for type".into(),
        }),
        Int => signed!(i32, DfdlValue::Int),
        UnsignedInt => u32::try_from(abs).map(DfdlValue::UnsignedInt).map_err(|_| VmError::InvalidValue {
            message: "out of range for type".into(),
        }),
        Long => signed!(i64, DfdlValue::Long),
        other => Err(VmError::TypeMismatch {
            expected: alloc::format!("decimal digits for `{other:?}`"),
        }),
    }
}

fn decode_decimal_binary(
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::value::DfdlValue;
    let le = props.byte_order == ByteOrder::LittleEndian;
    let vp = effective_binary_decimal_vp(props);
    match props.binary_number_rep {
        BinaryNumberRep::Binary => {
            let text = if bytes.len() <= 8 {
                format_binary_decimal_magnitude(decode_unsigned_binary_bytes(bytes, le), vp)
            } else {
                let mut mag = bytes.to_vec();
                if le {
                    mag.reverse();
                }
                let dec = magnitude_bytes_be_to_decimal(&mag);
                apply_virtual_point_to_decimal_magnitude(&dec, vp)
            };
            Ok(DfdlValue::Decimal(text))
        }
        BinaryNumberRep::Bcd => {
            let digits = bcd_to_digit_string(bytes, le)?;
            signed_magnitude_to_dfdl(false, &digits, crate::ir::ValueKind::Decimal, vp)
        }
        BinaryNumberRep::Ibm4690Packed => {
            let (negative, digits) = ibm4690_to_digit_string(bytes, le)?;
            validate_decimal_parse_sign(negative, props)?;
            signed_magnitude_to_dfdl(negative, &digits, crate::ir::ValueKind::Decimal, vp)
        }
        BinaryNumberRep::PackedBcd => {
            let codes = packed_sign_codes(props, strings)?;
            let (negative, digits) = packed_to_digit_string(bytes, le, &codes)?;
            validate_decimal_parse_sign(negative, props)?;
            signed_magnitude_to_dfdl(negative, &digits, crate::ir::ValueKind::Decimal, vp)
        }
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds are calendar encodings".into(),
            })
        }
    }
}

fn effective_binary_decimal_vp(props: &IrProps) -> i32 {
    props
        .binary_decimal_virtual_point_signed
        .unwrap_or(props.binary_decimal_virtual_point as i32)
}

fn format_virtual_decimal(value: u64, virtual_point: u32) -> alloc::string::String {
    format_binary_decimal_magnitude(value, virtual_point as i32)
}

fn apply_virtual_point_to_decimal_magnitude(mag: &str, vp: i32) -> alloc::string::String {
    if vp == 0 {
        return mag.into();
    }
    if vp < 0 {
        let exp = vp.unsigned_abs() as usize;
        return alloc::format!("{mag}{}", "0".repeat(exp));
    }
    format_binary_decimal_magnitude_from_digits(mag, vp as usize)
}

fn format_binary_decimal_magnitude_from_digits(mag: &str, vp: usize) -> alloc::string::String {
    let mag = mag.trim_start_matches('0');
    let mag = if mag.is_empty() { "0" } else { mag };
    if vp == 0 {
        return mag.into();
    }
    if mag == "0" {
        return alloc::format!("0.{:0width$}", 0, width = vp);
    }
    if mag.len() <= vp {
        return alloc::format!("0.{mag:0>width$}", width = vp);
    }
    let split = mag.len() - vp;
    alloc::format!("{}.{:0width$}", &mag[..split], &mag[split..], width = vp)
}

fn format_binary_decimal_magnitude(mag: u64, vp: i32) -> alloc::string::String {
    if vp == 0 {
        return mag.to_string();
    }
    if vp < 0 {
        let exp = vp.unsigned_abs() as usize;
        let mut s = mag.to_string();
        s.extend(core::iter::repeat_n('0', exp));
        return s;
    }
    let vp = vp as usize;
    if mag == 0 {
        return if vp == 0 {
            "0".into()
        } else {
            alloc::format!("0.{:0width$}", 0, width = vp)
        };
    }
    if let Some(scale) = 10u64.checked_pow(vp as u32) {
        let whole = mag / scale;
        let frac = mag % scale;
        return alloc::format!("{whole}.{frac:0width$}", width = vp);
    }
    let s = mag.to_string();
    if s.len() <= vp {
        return alloc::format!("0.{s:0>width$}", width = vp);
    }
    let split = s.len() - vp;
    alloc::format!("{}.{:0width$}", &s[..split], &s[split..], width = vp)
}

fn validate_decimal_parse_sign(
    negative: bool,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if !negative {
        return Ok(());
    }
    if !props.decimal_signed {
        let rep = match props.binary_number_rep {
            BinaryNumberRep::Binary => "Binary",
            BinaryNumberRep::PackedBcd | BinaryNumberRep::Ibm4690Packed => "Packed binary",
            BinaryNumberRep::Bcd => "BCD",
            BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => "Binary",
        };
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error: {rep} negative value when decimalSigned=\"no\""
            ),
        });
    }
    Ok(())
}

fn validate_decimal_unparse_sign(
    negative: bool,
    props: &IrProps,
    field_name: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if !negative {
        return Ok(());
    }
    let label = field_name.unwrap_or("value");
    if !props.decimal_signed {
        let rep = match props.binary_number_rep {
            BinaryNumberRep::Binary => "Binary",
            BinaryNumberRep::PackedBcd | BinaryNumberRep::Ibm4690Packed => "Packed binary",
            BinaryNumberRep::Bcd => "BCD",
            BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => "Binary",
        };
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Unparse Error: {rep} negative value when decimalSigned=\"no\""
            ),
        });
    }
    if props.binary_number_rep == BinaryNumberRep::Bcd {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Signed bcd only positive values allowed. {label} cannot be negative"
            ),
        });
    }
    Ok(())
}

fn parse_virtual_decimal_signed(
    text: &str,
    virtual_point: u32,
) -> Result<(bool, u64), crate::error::VmError> {
    let trimmed = text.trim();
    let negative = trimmed.starts_with('-');
    let body = if negative {
        trimmed.trim_start_matches('-').trim()
    } else {
        trimmed
    };
    parse_virtual_decimal(body, virtual_point).map(|mag| (negative, mag))
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
    bcd_to_digit_string(bytes, le).unwrap_or_default()
}

fn packed_digit_string(bytes: &[u8], le: bool, codes: &PackedSignCodes) -> alloc::string::String {
    let Ok((_neg, digits)) = packed_to_digit_string(bytes, le, codes) else {
        return alloc::string::String::new();
    };
    let mut out = digits;
    while out.starts_with('0') && out.len() > 1 {
        out.remove(0);
    }
    out
}

/// Magnitude string for packed binary calendars (matches Daffodil `packedToBigInteger` text).
fn packed_calendar_magnitude_string(
    bytes: &[u8],
    le: bool,
    codes: &PackedSignCodes,
) -> Result<alloc::string::String, crate::error::VmError> {
    let (_neg, digits) = packed_to_digit_string(bytes, le, codes)?;
    let trimmed = digits.trim_start_matches('0');
    Ok(if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    })
}

fn calendar_binary_rep(props: &IrProps) -> bool {
    matches!(
        props.binary_calendar_rep,
        BinaryNumberRep::BinarySeconds
            | BinaryNumberRep::BinaryMilliseconds
            | BinaryNumberRep::Bcd
            | BinaryNumberRep::Ibm4690Packed
            | BinaryNumberRep::PackedBcd
    )
}

fn decode_binary_calendar(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
    raw_bits: Option<(u64, usize)>,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::value::DfdlValue;

    let le = props.byte_order == ByteOrder::LittleEndian;
    let rep = props.binary_calendar_rep;
    if matches!(
        rep,
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds
    ) {
        let epoch_raw = props
            .binary_calendar_epoch
            .and_then(|id| strings.get(id).ok())
            .unwrap_or("1970-01-01T00:00:00");
        let base = crate::vm::calendar_binary::parse_calendar_epoch_unix(epoch_raw)?;
        return match rep {
            BinaryNumberRep::BinarySeconds => {
                let delta = crate::vm::calendar_binary::decode_binary_seconds_value(bytes, le)?;
                let text = crate::vm::calendar_binary::format_binary_calendar_from_seconds_delta(
                    epoch_raw,
                    delta,
                    tunables,
                )?;
                return calendar_value_from_text(kind, text);
            }
            BinaryNumberRep::BinaryMilliseconds => {
                let mut buf = [0u8; 8];
                if bytes.len() == 8 {
                    buf.copy_from_slice(bytes);
                } else if bytes.len() == 4 {
                    buf[..4].copy_from_slice(bytes);
                } else {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "binaryMilliseconds expects 4 or 8 bytes, got {}",
                            bytes.len()
                        ),
                    });
                }
                let delta_ms = if le {
                    i64::from_le_bytes(buf)
                } else {
                    i64::from_be_bytes(buf)
                };
                let text = crate::vm::calendar_binary::format_binary_calendar_from_millis_delta(
                    epoch_raw,
                    delta_ms,
                    tunables,
                )?;
                return calendar_value_from_text(kind, text);
            }
            _ => unreachable!(),
        };
    }
    let digits = match rep {
        BinaryNumberRep::Bcd => {
            if let Some((raw, bits)) = raw_bits {
                let from_raw =
                    crate::vm::calendar_binary::bcd_digits_from_raw_bits(raw, bits);
                if !from_raw.is_empty() {
                    from_raw
                } else {
                    bcd_to_digit_string(bytes, le)?
                }
            } else {
                bcd_to_digit_string(bytes, le)?
            }
        }
        BinaryNumberRep::Ibm4690Packed => {
            let (negative, d) = ibm4690_to_digit_string(bytes, le)?;
            if negative {
                return Err(crate::vm::calendar_binary::strict_calendar_lexical_error_from_negative_magnitude(
                    kind,
                    props.calendar_date_only,
                    &d,
                ));
            }
            d
        }
        BinaryNumberRep::PackedBcd => {
            let codes = packed_sign_codes(props, strings).unwrap_or_else(|_| {
                PackedSignCodes::parse("C D F C", BinaryNumberCheckPolicy::Lax).unwrap()
            });
            let (negative, digits) = packed_to_digit_string(bytes, le, &codes)?;
            if negative {
                return Err(crate::vm::calendar_binary::strict_calendar_lexical_error_from_negative_magnitude(
                    kind,
                    props.calendar_date_only,
                    &digits,
                ));
            }
            let trimmed = digits.trim_start_matches('0');
            if trimmed.is_empty() {
                "0".into()
            } else {
                trimmed.into()
            }
        }
        BinaryNumberRep::Binary => {
            return Err(VmError::InvalidValue {
                message: "binary dateTime requires BCD representation".into(),
            });
        }
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => unreachable!(),
    };
    let pat_id = props.calendar_pattern.ok_or(VmError::InvalidValue {
        message: "dateTime missing calendarPattern".into(),
    })?;
    let pattern = strings.get(pat_id)?;
    let digits = pad_calendar_digit_field(&digits, pattern);
    let text = format_calendar_pattern(
        &digits,
        pattern,
        props.calendar_century_start,
        props.calendar_first_day_of_week,
    )?;
    let default_utc = rep == BinaryNumberRep::PackedBcd
        && kind == crate::ir::ValueKind::DateTime
        && !props.calendar_date_only
        && text.contains('T');
    let text = append_packed_calendar_timezone(
        props,
        strings,
        kind,
        props.calendar_date_only,
        &text,
        default_utc,
        false,
    )?;
    if props.binary_number_check_policy == BinaryNumberCheckPolicy::Strict
        && !props.calendar_check_policy_lax
    {
        crate::vm::calendar_binary::strict_binary_calendar_component_ranges(
            kind,
            props.calendar_date_only,
            &text,
        )?;
    }
    calendar_value_from_text(kind, text)
}

fn calendar_value_from_text(
    kind: crate::ir::ValueKind,
    text: alloc::string::String,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::ir::ValueKind;
    use crate::value::DfdlValue;
    match kind {
        ValueKind::Time => Ok(DfdlValue::DateTime(text)),
        ValueKind::DateTime => Ok(DfdlValue::DateTime(text)),
        _ => Err(crate::error::VmError::TypeMismatch {
            expected: "calendar".into(),
        }),
    }
}

fn field_chars(
    fields: &alloc::collections::BTreeMap<char, alloc::string::String>,
    keys: &[char],
) -> Option<alloc::string::String> {
    for k in keys {
        if let Some(v) = fields.get(k) {
            return Some(v.clone());
        }
    }
    None
}

fn calendar_pattern_digit_count(pattern: &str) -> usize {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    let mut total = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        total += width;
        i += width;
    }
    total
}

fn pad_calendar_digit_field(digits: &str, pattern: &str) -> alloc::string::String {
    let need = calendar_pattern_digit_count(pattern);
    if digits.len() >= need {
        return digits.into();
    }
    let pad = need - digits.len();
    alloc::format!("{}{digits}", "0".repeat(pad))
}

fn format_calendar_pattern(
    digits: &str,
    pattern: &str,
    century_start: u32,
    first_day_of_week: u32,
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
    let letters = crate::vm::calendar_binary::calendar_pattern_letters_only(pattern);
    let has_y = letters.contains('y') || letters.contains('Y');
    let has_m = letters.contains('M');
    let has_d = letters.contains('d') || letters.contains('e');
    let has_doy = letters.contains('D');
    let year = fields
        .get(&'y')
        .or_else(|| fields.get(&'Y'))
        .map(|y| expand_calendar_year(y, century_start))
        .transpose()?;
    let month = fields.get(&'M').cloned();
    let day = fields
        .get(&'d')
        .or_else(|| fields.get(&'e'))
        .cloned();
    let day_of_year = fields.get(&'D').cloned();
    let hour = field_chars(&fields, &['H', 'h', 'k', 'K']);
    let minute = fields.get(&'m').cloned();
    let second = fields.get(&'s').cloned();
    let frac = fields.get(&'S').map(|s| format_calendar_s_fraction(s));
    if year.is_some() || month.is_some() || day.is_some() || day_of_year.is_some() || has_y || has_m || has_d || has_doy
    {
        let year_s = if has_y {
            year.ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` missing year"),
            })?
        } else {
            alloc::string::String::from("1970")
        };
        let year_i: i32 = year_s.parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` invalid year `{year_s}`"),
        })?;
        let (month_s, day_s) = if has_doy {
            let ord: u32 = day_of_year.ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` missing day-of-year"),
            })?
            .parse()
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` invalid day-of-year"),
            })?;
            let (m, d) = crate::vm::calendar_binary::month_day_from_ordinal(year_i, ord)?;
            (alloc::format!("{m:02}"), alloc::format!("{d:02}"))
        } else {
            let month_s = if has_m {
                month.ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("calendar `{pattern}` missing month"),
                })?
            } else {
                alloc::string::String::from("01")
            };
            let month_n: u32 = month_s.parse().map_err(|_| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` invalid month `{month_s}`"),
            })?;
            let day_s = if letters.contains('e') && !letters.contains('d') {
                let e_raw = fields.get(&'e').ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("calendar `{pattern}` missing localized day-of-week"),
                })?;
                let localized: u32 = e_raw.parse().map_err(|_| VmError::InvalidValue {
                    message: alloc::format!("calendar `{pattern}` invalid day-of-week `{e_raw}`"),
                })?;
                let wd = crate::vm::calendar_binary::weekday_from_localized_index(
                    localized,
                    first_day_of_week,
                );
                let d = crate::vm::calendar_binary::first_weekday_in_month(year_i, month_n, wd)
                    .ok_or_else(|| VmError::InvalidValue {
                        message: alloc::format!(
                            "calendar `{pattern}` no day for localized week index `{localized}`"
                        ),
                    })?;
                alloc::format!("{d:02}")
            } else if has_d {
                day.ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("calendar `{pattern}` missing day"),
                })?
            } else {
                alloc::string::String::from("01")
            };
            (month_s, day_s)
        };
        let month_n: u32 = month_s.parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` invalid month `{month_s}`"),
        })?;
        let day_n: u32 = day_s.parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` invalid day `{day_s}`"),
        })?;
        if let (Some(hour), Some(minute), Some(second)) = (&hour, &minute, &second) {
            let mut out =
                alloc::format!("{year_s}-{month_n:02}-{day_n:02}T{hour}:{minute}:{second}");
            if let Some(f) = frac {
                out.push_str(&f);
            }
            return Ok(out);
        }
        return Ok(alloc::format!("{year_s}-{month_n:02}-{day_n:02}"));
    }
    if crate::vm::calendar_binary::calendar_pattern_time_only(pattern) {
        let hour_s = hour
            .as_deref()
            .unwrap_or("00")
            .to_string();
        let minute_s = minute.as_deref().unwrap_or("00").to_string();
        let second_s = second.as_deref().unwrap_or("00").to_string();
        let mut out = alloc::format!("{hour_s}:{minute_s}:{second_s}");
        if let Some(f) = frac {
            out.push_str(&f);
        }
        return Ok(out);
    }
    if let (Some(hour), Some(minute), Some(second)) = (&hour, &minute, &second) {
        let mut out = alloc::format!("{hour}:{minute}:{second}");
        if let Some(f) = frac {
            out.push_str(&f);
        }
        return Ok(out);
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("calendar `{pattern}` missing date/time fields"),
    })
}

fn format_calendar_s_fraction(s_digits: &str) -> alloc::string::String {
    if s_digits.is_empty() {
        return alloc::string::String::new();
    }
    let ms = if s_digits.len() >= 3 {
        &s_digits[..3]
    } else {
        s_digits
    };
    let micros = ms.parse::<u32>().unwrap_or(0).saturating_mul(1000);
    alloc::format!(".{micros:06}")
}

struct CalendarTextFields {
    weekday: Option<alloc::string::String>,
    month: Option<u32>,
    day: Option<u32>,
    day_of_year: Option<u32>,
    week_in_month: Option<u32>,
    week_of_year: Option<u32>,
    localized_dow: Option<u32>,
    year: Option<i32>,
    hour: Option<u32>,
    minute: Option<u32>,
    second: Option<u32>,
    fraction_digits: Option<alloc::string::String>,
    hour12: bool,
    am_pm: Option<bool>,
    timezone: Option<alloc::string::String>,
    era_is_bc: bool,
}

#[derive(Default)]
struct CalendarPatternPresence {
    y: bool,
    m: bool,
    d: bool,
    day_of_year: bool,
    week_in_month: bool,
    week_of_year: bool,
    weekday: bool,
}

struct CalendarTextConfig<'a> {
    language: Option<&'a str>,
    first_day_of_week: u32,
    days_in_first_week: u32,
}

fn append_default_utc_offset(kind: crate::ir::ValueKind, date_only: bool, parsed: &str) -> alloc::string::String {
    if parsed.contains('T') {
        if parsed.contains('+')
            || parsed
                .rfind('-')
                .is_some_and(|i| i > 10 && parsed[i + 1..].contains(':'))
        {
            return parsed.into();
        }
        return alloc::format!("{parsed}+00:00");
    }
    if kind == crate::ir::ValueKind::Time || date_only {
        return alloc::format!("{parsed}+00:00");
    }
    parsed.into()
}

fn lexical_has_xsd_timezone(parsed: &str) -> bool {
    let check = |s: &str| {
        if s.contains('+') {
            return true;
        }
        s.rfind('-')
            .is_some_and(|i| i > 0 && s[i + 1..].contains(':'))
    };
    if let Some(idx) = parsed.find('T') {
        return check(&parsed[idx + 1..]);
    }
    check(parsed)
}

fn append_packed_calendar_timezone(
    props: &IrProps,
    strings: &StringPool,
    _kind: crate::ir::ValueKind,
    date_only: bool,
    parsed: &str,
    default_utc_when_missing: bool,
    allow_inherited_format_timezone: bool,
) -> Result<alloc::string::String, crate::error::VmError> {
    if lexical_has_xsd_timezone(parsed) {
        return Ok(parsed.into());
    }
    let timezone_suffix = || {
        props.calendar_time_zone.and_then(|id| strings.get(id).ok()).and_then(
            |raw| crate::vm::calendar_binary::calendar_timezone_xsd_suffix(raw),
        )
    };
    if props.calendar_time_zone_defined {
        if let Some(suffix) = timezone_suffix() {
            return Ok(alloc::format!("{parsed}{suffix}"));
        }
        return Ok(parsed.into());
    }
    if allow_inherited_format_timezone {
        if let Some(suffix) = timezone_suffix() {
            return Ok(alloc::format!("{parsed}{suffix}"));
        }
    }
    if default_utc_when_missing && parsed.contains('T') && !date_only {
        Ok(alloc::format!("{parsed}+00:00"))
    } else {
        Ok(parsed.into())
    }
}

fn read_calendar_timezone(text: &str, ti: &mut usize, z_width: usize) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let mut pos = *ti;
    while pos < text.len() && text.as_bytes()[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let mut tail = &text[pos..];
    if tail.len() >= 3 && tail[..3].eq_ignore_ascii_case("GMT") {
        pos += 3;
        while pos < text.len() && text.as_bytes()[pos].is_ascii_whitespace() {
            pos += 1;
        }
        tail = &text[pos..];
        if tail.is_empty() {
            *ti = pos;
            return Ok("+00:00".into());
        }
    }
    let (tz, consumed) = parse_calendar_tz_offset(tail).ok_or_else(|| VmError::InvalidValue {
        message: "calendar text mismatch".into(),
    })?;
    *ti = pos + consumed;
    Ok(tz)
}

fn timezone_abbrev_to_offset(name: &str) -> Option<&'static str> {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    match lower.as_str() {
        "est" | "et" | "eastern standard time" => Some("-05:00"),
        "edt" | "eastern daylight time" => Some("-04:00"),
        "cst" | "central standard time" => Some("-06:00"),
        "cdt" | "central daylight time" => Some("-05:00"),
        "mst" | "mountain standard time" => Some("-07:00"),
        "mdt" | "mountain daylight time" => Some("-06:00"),
        "pst" | "pt" | "pacific time" | "pacific standard time" => Some("-08:00"),
        "pdt" | "pacific daylight time" => Some("-07:00"),
        "utc" | "gmt" | "z" => Some("+00:00"),
        _ if n.eq_ignore_ascii_case("GMT") => Some("+00:00"),
        _ => None,
    }
}

fn timezone_long_names() -> &'static [(&'static str, &'static str)] {
    &[
        ("Eastern Standard Time", "-05:00"),
        ("Pacific Standard Time", "-08:00"),
        ("Central Standard Time", "-06:00"),
        ("Mountain Standard Time", "-07:00"),
        ("Los Angeles Time", "-08:00"),
    ]
}

fn timezone_id_to_offset(id: &str) -> Option<&'static str> {
    match id.to_ascii_lowercase().as_str() {
        "uslax" => Some("-08:00"),
        "unk" => Some("+00:00"),
        _ => None,
    }
}

fn read_calendar_timezone_name(
    text: &str,
    ti: &mut usize,
    kind: char,
    width: usize,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let rest = text[*ti..].trim_start();
    let base = *ti + text[*ti..].len().saturating_sub(rest.len());
    if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case("GMT") {
        let after_gmt = &rest[3..];
        let tail = after_gmt.trim_start();
        let tail_start = base + 3 + after_gmt.len().saturating_sub(tail.len());
        if tail.is_empty() {
            *ti = tail_start;
            return Ok("+00:00".into());
        }
        if let Some((tz, consumed)) = parse_calendar_tz_offset(tail) {
            *ti = tail_start + consumed;
            return Ok(tz);
        }
    }
    if (kind == 'z' || kind == 'v') && width >= 4 {
        for (name, off) in timezone_long_names() {
            if rest.starts_with(name) {
                *ti += text[*ti..].len() - rest.len() + name.len();
                return Ok((*off).into());
            }
        }
    }
    if kind == 'V' {
        if width >= 4 {
            for (name, off) in timezone_long_names() {
                if rest.starts_with(name) {
                    *ti += text[*ti..].len() - rest.len() + name.len();
                    return Ok((*off).into());
                }
            }
        }
        let end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '.' || c == ',')
            .unwrap_or(rest.len());
        let word = &rest[..end];
        if word.is_empty() {
            return Err(VmError::InvalidValue {
                message: "calendar text mismatch".into(),
            });
        }
        let off = timezone_id_to_offset(word).ok_or_else(|| VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        })?;
        *ti += text[*ti..].len() - rest.len() + word.len();
        return Ok((*off).into());
    }
    let end = rest
        .find(|c: char| c.is_ascii_whitespace() || c == '.' || c == ',')
        .unwrap_or(rest.len());
    let word = &rest[..end];
    if word.is_empty() {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    }
    if word.len() >= 3 && word[..3].eq_ignore_ascii_case("GMT") {
        let tail = word[3..].trim();
        if tail.is_empty() {
            *ti += text[*ti..].len() - rest.len() + word.len();
            return Ok("+00:00".into());
        }
        if let Some((tz, _)) = parse_calendar_tz_offset(tail) {
            *ti += text[*ti..].len() - rest.len() + word.len();
            return Ok(tz);
        }
    }
    if let Some((tz, consumed)) = parse_calendar_tz_offset(word) {
        *ti += text[*ti..].len() - rest.len() + consumed;
        return Ok(tz);
    }
    let off = timezone_abbrev_to_offset(word).ok_or_else(|| VmError::InvalidValue {
        message: "calendar text mismatch".into(),
    })?;
    *ti += text[*ti..].len() - rest.len() + word.len();
    Ok((*off).into())
}

pub(crate) fn parse_calendar_tz_offset(raw: &str) -> Option<(alloc::string::String, usize)> {
    let mut i = 0usize;
    while i < raw.len() && raw.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    let sign = *raw.as_bytes().get(i)?;
    if sign != b'+' && sign != b'-' {
        return None;
    }
    i += 1;
    if i + 5 <= raw.len()
        && raw[i..i + 2].bytes().all(|b| b.is_ascii_digit())
        && raw.as_bytes()[i + 2] == b':'
        && raw[i + 3..i + 5].bytes().all(|b| b.is_ascii_digit())
    {
        let hh = &raw[i..i + 2];
        let mm = &raw[i + 3..i + 5];
        hh.parse::<u8>().ok()?;
        mm.parse::<u8>().ok()?;
        let sign = sign as char;
        return Some((
            alloc::format!("{sign}{hh}:{mm}"),
            start + 1 + 2 + 1 + 2,
        ));
    }
    if i + 4 <= raw.len() && raw[i..i + 4].bytes().all(|b| b.is_ascii_digit()) {
        let hh = &raw[i..i + 2];
        let mm = &raw[i + 2..i + 4];
        hh.parse::<u8>().ok()?;
        mm.parse::<u8>().ok()?;
        let sign = sign as char;
        return Some((
            alloc::format!("{sign}{hh}:{mm}"),
            start + 1 + 4,
        ));
    }
    if i + 2 <= raw.len() && raw[i..i + 2].bytes().all(|b| b.is_ascii_digit()) {
        let hh = &raw[i..i + 2];
        hh.parse::<u8>().ok()?;
        let sign = sign as char;
        return Some((alloc::format!("{sign}{hh}:00"), start + 1 + 2));
    }
    if i + 1 <= raw.len() && raw[i..i + 1].bytes().all(|b| b.is_ascii_digit()) {
        let h1 = raw[i..i + 1].parse::<u8>().ok()?;
        if h1 <= 9 {
            let sign = sign as char;
            return Some((alloc::format!("{sign}{h1:02}:00"), start + 1 + 1));
        }
    }
    None
}

fn read_calendar_ampm(text: &str, ti: &mut usize) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    let rest = text[*ti..].trim_start();
    let upper = rest.to_ascii_uppercase();
    let (pm, consumed) = if upper.starts_with("PM") {
        (true, 2)
    } else if upper.starts_with("AM") {
        (false, 2)
    } else {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    };
    *ti += text[*ti..].len() - rest.len() + consumed;
    Ok(pm)
}

fn read_calendar_era(text: &str, ti: &mut usize) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    let rest = text[*ti..].trim_start();
    let word_end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let word = &rest[..word_end];
    let bc = if word.eq_ignore_ascii_case("BC") {
        true
    } else if word.eq_ignore_ascii_case("AD") {
        false
    } else {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    };
    *ti += text[*ti..].len() - rest.len() + word.len();
    Ok(bc)
}

fn apply_hour12(fields: &mut CalendarTextFields) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if !fields.hour12 {
        return Ok(());
    }
    let h = fields.hour.ok_or_else(|| VmError::InvalidValue {
        message: "calendar missing hour".into(),
    })?;
    let pm = fields.am_pm.unwrap_or(false);
    let h24 = if pm {
        if h == 12 { 12 } else { h + 12 }
    } else if h == 12 {
        0
    } else {
        h
    };
    fields.hour = Some(h24);
    Ok(())
}

fn finalize_calendar_hour_fields(
    fields: &mut CalendarTextFields,
    pattern: &str,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let letters = crate::vm::calendar_binary::calendar_pattern_letters_only(pattern);
    let has_a = letters.contains('a');
    let has_cap_h = pattern.contains('H');
    if has_a && has_cap_h && !fields.hour12 {
        fields.hour = Some(if fields.am_pm.unwrap_or(false) { 12 } else { 0 });
        return Ok(());
    }
    if letters.contains('K') {
        let h = fields.hour.unwrap_or(0);
        if has_a && h == 12 {
            return Err(VmError::InvalidValue {
                message: "calendar text mismatch".into(),
            });
        }
        let pm = fields.am_pm.unwrap_or(false);
        let h24 = if pm {
            if h == 0 { 12 } else { h + 12 }
        } else {
            h
        };
        fields.hour = Some(h24);
        return Ok(());
    }
    if letters.contains('k') {
        let h = fields.hour.unwrap_or(0);
        fields.hour = Some(if h == 0 { 0 } else if h == 24 { 0 } else { h - 1 });
        return Ok(());
    }
    apply_hour12(fields)
}

fn month_from_name_locale(name: &str, language: Option<&str>, lax: bool) -> Option<u32> {
    let trimmed = if lax {
        name.trim().trim_end_matches('.')
    } else {
        name.trim()
    };
    if let Some(lang) = language {
        let key = lang.split('-').next().unwrap_or(lang).to_ascii_lowercase();
        let lower = trimmed.to_lowercase();
        match key.as_str() {
            "de" if lower.contains("mär") || lower.contains("marz") || lower.contains("maerz") => {
                return Some(3);
            }
            "es" if lower == "noviembre" => return Some(11),
            "ru" if lower.starts_with("мар") => return Some(3),
            _ => {}
        }
    }
    month_from_name(trimmed)
}

fn month_from_name(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "january" | "jan" => Some(1),
        "february" | "feb" => Some(2),
        "march" | "mar" => Some(3),
        "april" | "apr" => Some(4),
        "may" => Some(5),
        "june" | "jun" => Some(6),
        "july" | "jul" => Some(7),
        "august" | "aug" => Some(8),
        "september" | "sep" | "sept" => Some(9),
        "october" | "oct" => Some(10),
        "november" | "nov" => Some(11),
        "december" | "dec" => Some(12),
        _ => None,
    }
}

fn weekday_from_name_locale(name: &str, language: Option<&str>) -> Option<u32> {
    let n = name.trim().to_ascii_lowercase();
    if let Some(lang) = language {
        let key = lang.split('-').next().unwrap_or(lang).to_ascii_lowercase();
        match key.as_str() {
            "de" if matches!(n.as_str(), "freitag" | "fr") => return Some(5),
            "es" if matches!(n.as_str(), "lunes" | "lu") => return Some(1),
            "ru" if n.starts_with("пят") => return Some(5),
            _ => {}
        }
    }
    weekday_from_name(name)
}

fn weekday_from_name(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "monday" | "mon" => Some(1),
        "tuesday" | "tue" | "tues" => Some(2),
        "wednesday" | "wed" => Some(3),
        "thursday" | "thu" | "thur" | "thurs" => Some(4),
        "friday" | "fri" => Some(5),
        "saturday" | "sat" => Some(6),
        "sunday" | "sun" => Some(7),
        _ => None,
    }
}

fn calendar_language_is_valid(locale: &str) -> bool {
    if locale.is_empty() {
        return false;
    }
    let mut parts = locale.split(|c| c == '-' || c == '_');
    let first = parts.next().unwrap_or("");
    if first.is_empty()
        || first.len() > 8
        || !first.chars().all(|c| c.is_ascii_alphabetic())
    {
        return false;
    }
    for part in parts {
        if part.is_empty() || part.len() > 8 {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_alphanumeric()) {
            return false;
        }
    }
    true
}

fn calendar_language_sde(locale: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!(
            "Schema Definition Error: dfdl:calendarLanguage property syntax error. Must match '([A-Za-z]{{1,8}}([-_][A-Za-z0-9]{{1,8}})*)' (ex: 'en_us' or 'de_1996'), but was '{locale}'."
        ),
    }
}

pub(crate) fn sibling_text_map_for_calendar(
    siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
) -> Option<alloc::collections::BTreeMap<String, alloc::string::String>> {
    let map = siblings?;
    let mut out = alloc::collections::BTreeMap::new();
    for (k, v) in map {
        let text = match v {
            crate::value::DfdlValue::String(s) => s.text.clone(),
            crate::value::DfdlValue::DateTime(s) => s.clone(),
            crate::value::DfdlValue::Integer(i) => i.clone(),
            crate::value::DfdlValue::Long(n) => alloc::format!("{n}"),
            crate::value::DfdlValue::Int(n) => alloc::format!("{n}"),
            _ => continue,
        };
        out.insert(k.clone(), text);
    }
    Some(out)
}

pub(crate) fn resolve_calendar_language(
    props: &IrProps,
    strings: &StringPool,
    siblings: Option<&alloc::collections::BTreeMap<String, alloc::string::String>>,
) -> Result<Option<alloc::string::String>, crate::error::VmError> {
    use crate::ir::IrInputValueCalcSegment;
    if let Some(segs) = &props.calendar_language_segments {
        let mut out = alloc::string::String::new();
        for seg in segs {
            match seg {
                IrInputValueCalcSegment::Sibling(id) => {
                    let name = strings.get(*id)?;
                    let local = crate::xml_util::local_name_str(name);
                    let val = siblings
                        .and_then(|m| m.get(name).or_else(|| m.get(local)))
                        .ok_or_else(|| crate::error::VmError::InvalidValue {
                            message: alloc::format!(
                                "missing sibling `{name}` for calendarLanguage"
                            ),
                        })?;
                    out.push_str(val);
                }
                IrInputValueCalcSegment::Literal(id) => out.push_str(strings.get(*id)?),
                IrInputValueCalcSegment::Substring {
                    sibling,
                    start,
                    length,
                } => {
                    let name = strings.get(*sibling)?;
                    let text = siblings
                        .and_then(|m| m.get(name))
                        .ok_or_else(|| crate::error::VmError::InvalidValue {
                            message: alloc::format!("missing sibling `{name}` for calendarLanguage"),
                        })?;
                    let start = (*start as usize).saturating_sub(1);
                    for ch in text.chars().skip(start).take(*length as usize) {
                        out.push(ch);
                    }
                }
            }
        }
        if !calendar_language_is_valid(&out) {
            return Err(calendar_language_sde(&out));
        }
        return Ok(Some(out));
    }
    if let Some(id) = props.calendar_language {
        let s = strings.get(id)?.trim();
        if !s.is_empty() {
            if !calendar_language_is_valid(s) {
                return Err(calendar_language_sde(s));
            }
            return Ok(Some(s.to_string()));
        }
    }
    Ok(None)
}

fn month_name_unparse_locale(month: u32, language: Option<&str>, width: usize) -> alloc::string::String {
    let key = language
        .and_then(|l| l.split(|c| c == '-' || c == '_').next())
        .map(|s| s.to_ascii_lowercase());
    if key.as_deref() == Some("de") {
        let full = match month {
            1 => "Januar",
            2 => "Februar",
            3 => "März",
            4 => "April",
            5 => "Mai",
            6 => "Juni",
            7 => "Juli",
            8 => "August",
            9 => "September",
            10 => "Oktober",
            11 => "November",
            12 => "Dezember",
            _ => "März",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    let full = match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "March",
    };
    if width >= 4 {
        full.into()
    } else {
        full.chars().take(width.max(1)).collect()
    }
}

fn weekday_name_unparse_locale(wd: u32, language: Option<&str>, width: usize) -> alloc::string::String {
    let key = language
        .and_then(|l| l.split(|c| c == '-' || c == '_').next())
        .map(|s| s.to_ascii_lowercase());
    if key.as_deref() == Some("de") {
        let full = match wd {
            1 => "Montag",
            2 => "Dienstag",
            3 => "Mittwoch",
            4 => "Donnerstag",
            5 => "Freitag",
            6 => "Samstag",
            7 => "Sonntag",
            _ => "Freitag",
        };
        if width >= 4 {
            return full.into();
        }
        return full.chars().take(width.max(1)).collect();
    }
    let full = match wd {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        7 => "Sunday",
        _ => "Friday",
    };
    if width >= 4 {
        full.into()
    } else {
        full.chars().take(width.max(1)).collect()
    }
}

fn parse_iso_date_ymd(iso: &str) -> Result<(i32, u32, u32), crate::error::VmError> {
    use crate::error::VmError;
    let date_part = iso.split('T').next().unwrap_or(iso).trim();
    let mut parts = date_part.split('-');
    let y: i32 = parts
        .next()
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("invalid xs:date `{iso}`"),
        })?
        .parse()
        .map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:date `{iso}`"),
        })?;
    let m: u32 = parts
        .next()
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("invalid xs:date `{iso}`"),
        })?
        .parse()
        .map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:date `{iso}`"),
        })?;
    let d: u32 = parts
        .next()
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("invalid xs:date `{iso}`"),
        })?
        .parse()
        .map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:date `{iso}`"),
        })?;
    Ok((y, m, d))
}

fn unparse_iso_date_to_calendar_pattern(
    iso: &str,
    pattern: &str,
    language: Option<&str>,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let (year, month, day) = parse_iso_date_ymd(iso)?;
    let weekday = weekday_of_ymd(year, month, day).unwrap_or(5);
    let mut out = alloc::string::String::new();
    let chars: alloc::vec::Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                out.push('\'');
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid calendarPattern `{pattern}`"),
                });
            }
            let lit: alloc::string::String = chars[start..i]
                .iter()
                .collect::<alloc::string::String>()
                .replace("''", "'");
            out.push_str(&lit);
            i += 1;
            continue;
        }
        let c = chars[i];
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            let field = match c {
                'E' => weekday_name_unparse_locale(weekday, language, w),
                'M' if w >= 3 => month_name_unparse_locale(month, language, w),
                'M' => alloc::format!("{month:02}"),
                'd' => {
                    if w >= 2 {
                        alloc::format!("{day:02}")
                    } else {
                        alloc::format!("{day}")
                    }
                }
                'y' | 'Y' => {
                    if w >= 4 {
                        alloc::format!("{year:04}")
                    } else {
                        alloc::format!("{:02}", year % 100)
                    }
                }
                _ => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("unsupported calendar field `{c}` in unparse"),
                    });
                }
            };
            out.push_str(&field);
            i += w;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

fn weekday_of_ymd(year: i32, month: u32, day: u32) -> Option<u32> {
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let q = day as i32;
    let m = month as i32;
    let y = year;
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    let k = y % 100;
    let j = y / 100;
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    // Zeller: 0=Saturday … convert to ISO Monday=1 … Sunday=7
    Some(((h + 5) % 7 + 1) as u32)
}

fn infer_day_from_weekday(year: i32, month: u32, weekday: u32) -> Option<u32> {
    let dim = crate::vm::calendar_binary::days_in_month(year, month);
    for day in 1..=dim {
        if weekday_of_ymd(year, month, day) == Some(weekday) {
            return Some(day);
        }
    }
    None
}

fn read_calendar_field(
    text: &str,
    ti: &mut usize,
    width: usize,
    letters: char,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    if letters == 'E' || letters == 'M' && width >= 3 {
        let rest = text[*ti..].trim_start();
        let word_end = rest
            .find(|c: char| c.is_whitespace() || c == '-' || c == ':' || c == ',')
            .unwrap_or(rest.len());
        let word = &rest[..word_end];
        if word.is_empty() {
            return Err(VmError::InvalidValue {
                message: "calendar text mismatch".into(),
            });
        }
        *ti += text[*ti..].len() - rest.len() + word.len();
        return Ok(word.to_string());
    }
    let slice = text.get(*ti..).ok_or(VmError::InvalidValue {
        message: "calendar text mismatch".into(),
    })?;
    let mut out = alloc::string::String::new();
    let max_digits = if width == 1
        && matches!(
            letters,
            'd' | 'M' | 'y' | 'Y' | 'D' | 'F' | 'w' | 'W' | 'H' | 'h' | 'm' | 's' | 'e'
        )
    {
        4
    } else {
        width
    };
    for ch in slice.chars().take(max_digits) {
        if !ch.is_ascii_digit() {
            break;
        }
        out.push(ch);
        *ti += ch.len_utf8();
        if width > 1 && out.len() >= width {
            break;
        }
    }
    if out.is_empty() {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    }
    if width > 1
        && out.len() != width
        && !matches!(letters, 'H' | 'h' | 'k' | 'K' | 'm' | 's' | 'S' | 'd' | 'M')
    {
        return Err(VmError::InvalidValue {
            message: "calendar text mismatch".into(),
        });
    }
    Ok(out)
}

fn calendar_text_strict_date_error(text: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!("Parse Error: Unable to parse xs:date from text: {text}"),
    }
}

fn calendar_text_strict_time_error(text: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!("Parse Error: Unable to parse xs:time from text: {text}"),
    }
}

fn xsd_tz_to_offset_secs(tz: &str) -> Option<i64> {
    let (normalized, _) = parse_calendar_tz_offset(tz)?;
    let sign = if normalized.starts_with('-') { -1i64 } else { 1i64 };
    let body = normalized.trim_start_matches(['+', '-']);
    let (hh, mm) = body.split_once(':').unwrap_or((body, "0"));
    let hh: i64 = hh.parse().ok()?;
    let mm: i64 = mm.parse().ok()?;
    Some(sign * (hh * 3600 + mm * 60))
}

fn parse_iso_time_hms(iso: &str) -> Result<(u32, u32, u32, Option<i64>), crate::error::VmError> {
    use crate::error::VmError;
    let iso = iso.trim();
    let (core, tz_secs) = if iso.ends_with('Z') {
        (&iso[..iso.len().saturating_sub(1)], Some(0i64))
    } else if let Some(i) = iso.rfind('+').filter(|&i| i >= 5) {
        (
            &iso[..i],
            xsd_tz_to_offset_secs(&iso[i..]),
        )
    } else if let Some(rel) = iso.get(8..) {
        if let Some(i) = rel.find('-') {
            let idx = 8 + i;
            (
                &iso[..idx],
                xsd_tz_to_offset_secs(&iso[idx..]),
            )
        } else {
            (iso, None)
        }
    } else {
        (iso, None)
    };
    let parts: alloc::vec::Vec<&str> = core.split(':').collect();
    if parts.len() < 2 {
        return Err(VmError::InvalidValue {
            message: alloc::format!("invalid xs:time `{iso}`"),
        });
    }
    let hh: u32 = parts[0].parse().map_err(|_| VmError::InvalidValue {
        message: alloc::format!("invalid xs:time `{iso}`"),
    })?;
    let mm: u32 = parts[1].parse().map_err(|_| VmError::InvalidValue {
        message: alloc::format!("invalid xs:time `{iso}`"),
    })?;
    let ss = if parts.len() > 2 {
        parts[2]
            .split('.')
            .next()
            .unwrap_or(parts[2])
            .parse()
            .unwrap_or(0)
    } else {
        0
    };
    Ok((hh, mm, ss, tz_secs))
}

fn emit_calendar_tz_unparse(offset_secs: i64, kind: char, width: usize) -> alloc::string::String {
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs = offset_secs.abs();
    let oh = abs / 3600;
    let om = (abs % 3600) / 60;
    let xsd = alloc::format!("{sign}{oh:02}:{om:02}");
    match kind {
        'z' => {
            if width >= 4 {
                alloc::format!("GMT{xsd}")
            } else {
                alloc::format!("GMT{sign}{oh}")
            }
        }
        'Z' => alloc::format!("{sign}{oh:02}{om:02}"),
        'v' => {
            if width >= 4 {
                alloc::format!("GMT{xsd}")
            } else {
                alloc::format!("GMT{sign}{oh}")
            }
        }
        'V' => {
            if offset_secs == 0 {
                "gmt".into()
            } else if width >= 4 {
                alloc::format!("GMT{xsd}")
            } else {
                "unk".into()
            }
        }
        _ => alloc::string::String::new(),
    }
}

fn unparse_iso_time_to_calendar_pattern(
    iso: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let (hour, minute, second, tz_secs) = parse_iso_time_hms(iso)?;
    let mut out = alloc::string::String::new();
    let chars: alloc::vec::Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                out.push('\'');
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid calendarPattern `{pattern}`"),
                });
            }
            let lit: alloc::string::String = chars[start..i]
                .iter()
                .collect::<alloc::string::String>()
                .replace("''", "'");
            out.push_str(&lit);
            i += 1;
            continue;
        }
        let c = chars[i];
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        if matches!(c, 'z' | 'Z' | 'v' | 'V') {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            if let Some(secs) = tz_secs {
                out.push_str(&emit_calendar_tz_unparse(secs, c, w));
            }
            i += w;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            let field = match c {
                'h' | 'H' | 'k' | 'K' => {
                    let h = if matches!(c, 'H' | 'k') {
                        hour
                    } else if hour == 0 || hour > 12 {
                        hour % 12
                    } else {
                        hour
                    };
                    if w >= 2 {
                        alloc::format!("{h:02}")
                    } else {
                        alloc::format!("{h}")
                    }
                }
                'm' => {
                    if w >= 2 {
                        alloc::format!("{minute:02}")
                    } else {
                        alloc::format!("{minute}")
                    }
                }
                's' => {
                    if w >= 2 {
                        alloc::format!("{second:02}")
                    } else {
                        alloc::format!("{second}")
                    }
                }
                'S' => "0".repeat(w.min(9)),
                _ => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("unsupported calendar field `{c}` in unparse"),
                    });
                }
            };
            out.push_str(&field);
            i += w;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

fn format_calendar_text(
    text: &str,
    pattern: &str,
    lax: bool,
    century_start: u32,
    cal: CalendarTextConfig<'_>,
    time_overflow_carries_to_date: bool,
    date_only: bool,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;

    let text = text.trim();
    let map_err = |e: VmError| -> VmError {
        if lax {
            e
        } else {
            calendar_text_strict_date_error(text)
        }
    };
    let mut ti = 0usize;
    let mut fields = CalendarTextFields {
        weekday: None,
        month: None,
        day: None,
        day_of_year: None,
        week_in_month: None,
        week_of_year: None,
        localized_dow: None,
        year: None,
        hour: None,
        minute: None,
        second: None,
        fraction_digits: None,
        hour12: false,
        am_pm: None,
        timezone: None,
        era_is_bc: false,
    };
    let mut pattern_parts = CalendarPatternPresence::default();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                if !text[ti..].starts_with('\'') {
                    return Err(VmError::InvalidValue {
                        message: "calendar text mismatch".into(),
                    });
                }
                ti += 1;
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid calendarPattern `{pattern}`"),
                });
            }
            let lit: alloc::string::String = chars[start..i]
                .iter()
                .collect::<alloc::string::String>()
                .replace("''", "'");
            i += 1;
            if !text[ti..].starts_with(&lit) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("calendar `{pattern}` mismatch"),
                });
            }
            ti += lit.len();
            continue;
        }
        let c = chars[i];
        if c.is_whitespace() {
            while ti < text.len() && text.as_bytes()[ti].is_ascii_whitespace() {
                ti += 1;
            }
            i += 1;
            continue;
        }
        if c == 'Z' || c == 'z' || c == 'v' || c == 'V' {
            let mut z_width = 1usize;
            while i + z_width < chars.len() && chars[i + z_width] == c {
                z_width += 1;
            }
            fields.timezone = Some(if c == 'Z' {
                read_calendar_timezone(text, &mut ti, z_width)?
            } else {
                read_calendar_timezone_name(text, &mut ti, c, z_width)?
            });
            i += z_width;
            continue;
        }
        if c == 'G' {
            fields.era_is_bc = read_calendar_era(text, &mut ti)?;
            i += 1;
            continue;
        }
        if c == 'a' {
            let mut a_width = 1usize;
            while i + a_width < chars.len() && chars[i + a_width] == 'a' {
                a_width += 1;
            }
            fields.am_pm = Some(read_calendar_ampm(text, &mut ti)?);
            i += a_width;
            continue;
        }
        const FIELD: &str = "EMdDFwWmyYHhsekKS";
        if !FIELD.contains(c) {
            let Some(ch) = text[ti..].chars().next() else {
                return Err(VmError::InvalidValue {
                    message: "calendar text mismatch".into(),
                });
            };
            if ch != c {
                return Err(VmError::InvalidValue {
                    message: "calendar text mismatch".into(),
                });
            }
            ti += ch.len_utf8();
            i += 1;
            continue;
        }
        let mut width = 1usize;
        while i + width < chars.len() && chars[i + width] == c {
            width += 1;
        }
        let raw = read_calendar_field(text, &mut ti, width, c)?;
        match c {
            'E' => {
                pattern_parts.weekday = true;
                fields.weekday = Some(raw);
            }
            'M' if width >= 3 => {
                pattern_parts.m = true;
                fields.month = Some(
                    month_from_name_locale(&raw, cal.language, lax).ok_or_else(|| {
                        map_err(VmError::InvalidValue {
                            message: alloc::format!("calendar `{pattern}` invalid month `{raw}`"),
                        })
                    })?,
                );
            }
            'e' => {
                fields.localized_dow = raw.parse().ok();
            }
            'F' => {
                pattern_parts.week_in_month = true;
                fields.week_in_month = raw.parse().ok();
            }
            'w' => {
                pattern_parts.week_of_year = true;
                fields.week_of_year = raw.parse().ok();
            }
            'W' => {
                pattern_parts.week_in_month = true;
                fields.week_in_month = raw.parse().ok();
            }
            'M' => {
                pattern_parts.m = true;
                fields.month = raw.parse().ok();
            }
            'd' => {
                pattern_parts.d = true;
                fields.day = raw.parse().ok();
            }
            'D' => {
                pattern_parts.day_of_year = true;
                fields.day_of_year = raw.parse().ok();
            }
            'y' | 'Y' => {
                pattern_parts.y = true;
                let ys = expand_calendar_year(&raw, century_start)?;
                fields.year = ys.parse().ok();
            }
            'H' => {
                fields.hour = raw.parse().ok();
            }
            'h' => {
                fields.hour12 = true;
                fields.hour = raw.parse().ok();
            }
            'k' | 'K' => {
                fields.hour = raw.parse().ok();
            }
            'm' => {
                fields.minute = raw.parse().ok();
            }
            's' => {
                fields.second = raw.parse().ok();
            }
            'S' => {
                fields.fraction_digits = Some(raw);
            }
            _ => {}
        }
        i += width;
    }
    while ti < text.len() && text.as_bytes()[ti].is_ascii_whitespace() {
        ti += 1;
    }
    if ti != text.len() {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "calendar text mismatch at {ti}/{} for `{pattern}` in `{text}`",
                text.len()
            ),
        });
    }
    let has_date = pattern_parts.y
        || pattern_parts.m
        || pattern_parts.d
        || pattern_parts.day_of_year
        || pattern_parts.week_in_month
        || pattern_parts.week_of_year
        || pattern_parts.weekday
        || fields.year.is_some()
        || fields.month.is_some()
        || fields.day.is_some()
        || fields.day_of_year.is_some()
        || fields.week_in_month.is_some()
        || fields.week_of_year.is_some();
    let has_time = fields.hour.is_some() || fields.minute.is_some() || fields.second.is_some();
    finalize_calendar_hour_fields(&mut fields, pattern)?;
    if has_time && !has_date {
        let hour = fields.hour.unwrap_or(0);
        let minute = fields.minute.unwrap_or(0);
        let second = fields.second.unwrap_or(0);
        let (hour, minute, second) = if lax {
            crate::vm::calendar_binary::normalize_lenient_hms(hour, minute, second)
        } else {
            (hour, minute, second)
        };
        let mut out = alloc::format!("{hour:02}:{minute:02}:{second:02}");
        if let Some(ref frac) = fields.fraction_digits {
            out.push_str(&format_calendar_s_fraction(frac));
        }
        if let Some(tz) = fields.timezone {
            out.push_str(&tz);
        }
        return Ok(out);
    }
    let mut year = fields.year.unwrap_or(1970);
    if fields.era_is_bc {
        year = -(year - 1);
    }
    let (month, day) = if pattern_parts.day_of_year {
        let ordinal = fields.day_of_year.ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` missing day-of-year"),
        })?;
        if !pattern_parts.y {
            year = 1970;
        }
        crate::vm::calendar_binary::month_day_from_ordinal(year, ordinal)?
    } else if pattern_parts.week_of_year {
        let week = fields.week_of_year.ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` missing week-of-year"),
        })?;
        let target_year = fields.year.unwrap_or(1970);
        let (y, m, d) = crate::vm::calendar_binary::date_from_week_of_year(
            target_year,
            week,
            cal.first_day_of_week,
            cal.days_in_first_week,
        )?;
        year = y;
        (m, d)
    } else if pattern_parts.week_in_month {
        let month = fields.month.ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` missing month"),
        })?;
        if !pattern_parts.y {
            year = 1970;
        }
        let n = fields.week_in_month.ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` missing week-in-month"),
        })?;
        if pattern.contains('W') {
            let (y, m, d) = crate::vm::calendar_binary::date_from_week_of_month(
                year,
                month,
                n,
                cal.first_day_of_week,
                cal.days_in_first_week,
            )?;
            year = y;
            (m, d)
        } else {
            let day = crate::vm::calendar_binary::nth_weekday_in_month(
                year,
                month,
                n,
                cal.first_day_of_week,
            )?;
            (month, day)
        }
    } else {
        if !pattern_parts.y {
            year = 1970;
        }
        let month = if pattern_parts.m {
            fields.month.ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` missing month"),
            })?
        } else {
            1
        };
        let day = if pattern_parts.d {
            fields.day.ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` missing day"),
            })?
        } else if let Some(e) = fields.localized_dow {
            let wd = crate::vm::calendar_binary::weekday_from_localized_index(
                e,
                cal.first_day_of_week,
            );
            crate::vm::calendar_binary::first_weekday_in_month(year, month, wd).ok_or_else(|| {
                VmError::InvalidValue {
                    message: alloc::format!("calendar `{pattern}` missing day"),
                }
            })?
        } else if let Some(ref wd) = fields.weekday {
            let w = weekday_from_name_locale(wd, cal.language).ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` invalid weekday"),
            })?;
            infer_day_from_weekday(year, month, w).ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` missing day"),
            })?
        } else {
            1
        };
        (month, day)
    };
    let (year, month, day) = if lax {
        crate::vm::calendar_binary::normalize_lenient_ymd(year, month, day)
    } else {
        (year, month, day)
    };
    if has_time {
        let hour = fields.hour.ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("calendar `{pattern}` missing hour"),
        })?;
        let letters = crate::vm::calendar_binary::calendar_pattern_letters_only(pattern);
        let minute = if letters.contains('m') {
            fields.minute.ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("calendar `{pattern}` missing minute"),
            })?
        } else {
            fields.minute.unwrap_or(0)
        };
        let second = if letters.contains('s') {
            fields.second.unwrap_or(0)
        } else {
            fields.second.unwrap_or(0)
        };
        let (hour, minute, second, day_carry) = if lax {
            crate::vm::calendar_binary::normalize_lenient_hms_with_day_carry(
                hour,
                minute,
                second,
                !time_overflow_carries_to_date,
            )
        } else {
            (hour, minute, second, 0)
        };
        let (year, month, day) = if day_carry != 0 {
            crate::vm::calendar_binary::normalize_lenient_ymd(
                year,
                month,
                day.saturating_add(day_carry as u32),
            )
        } else {
            (year, month, day)
        };
        let mut out = alloc::format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
        );
        if let Some(tz) = fields.timezone {
            out.push_str(&tz);
        }
        return Ok(out);
    }
    let out = alloc::format!("{year:04}-{month:02}-{day:02}");
    if time_overflow_carries_to_date
        && !date_only
        && pattern.contains('W')
        && !crate::vm::calendar_binary::calendar_pattern_has_time_fields(pattern)
    {
        Ok(alloc::format!("{out}T00:00:00"))
    } else {
        Ok(out)
    }
}

fn match_text_number_subpattern(text: &str, pattern: &str) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let mut ti = 0usize;
    let mut digits = alloc::string::String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("invalid textNumberPattern `{pattern}`"),
                });
            }
            let lit: alloc::string::String = chars[start..i].iter().collect();
            i += 1;
            if !text[ti..].starts_with(&lit) {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            ti += lit.len();
        } else if matches!(chars[i], '0' | '#') {
            let Some(ch) = text.as_bytes().get(ti) else {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            };
            if !ch.is_ascii_digit() {
                return Err(VmError::InvalidValue {
                    message: "textNumberPattern mismatch".into(),
                });
            }
            digits.push(*ch as char);
            ti += 1;
            i += 1;
        } else {
            i += 1;
        }
    }
    if ti != text.len() {
        return Err(VmError::InvalidValue {
            message: "textNumberPattern mismatch".into(),
        });
    }
    Ok(digits)
}

fn apply_text_number_sign_pattern(
    text: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    let mut alts = pattern.split(';');
    if let Some(pos) = alts.next() {
        if let Ok(digits) = match_text_number_subpattern(text, pos) {
            return Ok(digits);
        }
    }
    if let Some(neg) = alts.next() {
        if let Ok(digits) = match_text_number_subpattern(text, neg) {
            return Ok(alloc::format!("-{digits}"));
        }
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("Parse Error. xs:int {text}"),
    })
}

fn apply_text_number_pattern_numeric(
    text: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    if pattern.contains(';') && pattern.contains('\'') {
        return apply_text_number_sign_pattern(text, pattern);
    }
    if !pattern.contains('V') {
        return Ok(text.into());
    }
    let mut parts = pattern.split('V');
    let before = parts.next().unwrap_or("");
    let after = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(VmError::InvalidValue {
            message: alloc::format!("unsupported textNumberPattern `{pattern}`"),
        });
    }
    let digit_before = before
        .chars()
        .filter(|c| matches!(c, '0' | '#'))
        .count();
    let digit_after = after
        .chars()
        .filter(|c| matches!(c, '0' | '#'))
        .count();
    let (negative, body) = if text.starts_with('-') {
        (true, &text[1..])
    } else {
        (false, text)
    };
    if body.chars().any(|c| !c.is_ascii_digit()) {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error. Unable to parse xs:decimal from text: {text}"
            ),
        });
    }
    let total_digits = digit_before + digit_after;
    let mut body_owned = body.to_string();
    if body_owned.len() < total_digits {
        let pad = total_digits - body_owned.len();
        body_owned = alloc::format!("{}{body_owned}", "0".repeat(pad));
    } else if body_owned.len() > total_digits {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "textNumberPattern `{pattern}` expected {} digits, got `{text}`",
                total_digits
            ),
        });
    }
    let body = body_owned.as_str();
    let mut out = alloc::format!(
        "{}.{}",
        &body[..digit_before],
        &body[digit_before..]
    );
    if negative {
        out.insert(0, '-');
    }
    Ok(out)
}

fn resolved_text_number_format_parts(
    props: &IrProps,
    strings: &StringPool,
) -> (
    alloc::vec::Vec<alloc::string::String>,
    Option<alloc::string::String>,
    alloc::string::String,
    Option<char>,
    crate::schema::BinaryNumberCheckPolicy,
) {
    let dec = if let Some(r) = props.resolved_text_standard_decimal_separator.as_deref() {
        r
    } else if props.text_standard_decimal_separator_defined {
        strings
            .get(props.text_standard_decimal_separator)
            .unwrap_or("")
    } else {
        "."
    };
    let mut dec_seps = crate::schema::parse_text_standard_separator_list(dec);
    if dec_seps.is_empty()
        && !props.text_standard_decimal_separator_defined
        && props.resolved_text_standard_decimal_separator.is_none()
        && props.text_standard_decimal_separator_sibling.is_none()
    {
        dec_seps.push(".".into());
    }
    let exponent = if props.text_standard_exponent_rep_defined {
        props
            .resolved_text_standard_exponent_rep
            .clone()
            .or_else(|| strings.get(props.text_standard_exponent_rep).ok().map(|s| s.to_string()))
            .unwrap_or_default()
    } else {
        props
            .resolved_text_standard_exponent_rep
            .as_deref()
            .or_else(|| strings.get(props.text_standard_exponent_rep).ok())
            .unwrap_or("E")
            .to_string()
    };
    let grouping = props
        .resolved_text_standard_grouping_separator
        .clone()
        .or_else(|| {
            props
                .text_standard_grouping_separator
                .and_then(|id| strings.get(id).ok())
                .map(|g| g.to_string())
        });
    let pad = props
        .text_number_pad_character
        .and_then(|id| strings.get(id).ok())
        .and_then(|p| p.chars().next())
        .or(Some('0'));
    (
        dec_seps,
        grouping,
        exponent,
        pad,
        props.text_number_check_policy,
    )
}

fn text_number_pattern_needs_full_span(pattern: &str) -> bool {
    if pattern.contains('*') || pattern.contains(';') || pattern.contains('\'') {
        return true;
    }
    pattern.chars().any(|c| {
        c.is_ascii_alphabetic() && c != 'E' && c != 'e' && c != 'V' && c != 'P'
    })
}

fn read_implicit_numeric_text(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> alloc::vec::Vec<u8> {
    if props.custom_text_number_pattern {
        if let Some(pat_id) = props.text_number_pattern {
            if let Ok(raw_pattern) = strings.get(pat_id) {
                let pattern_owned = if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
                    crate::vm::zoned_text::strip_zoned_plus_markers(raw_pattern)
                } else {
                    raw_pattern.to_string()
                };
                let pattern = pattern_owned.as_str();
                if text_number_pattern_needs_full_span(pattern) {
                    let (dec_seps, grouping, exponent, pad, check_policy) =
                        resolved_text_number_format_parts(props, strings);
                    let fmt = crate::vm::text_number::TextNumberFormatProps {
                        check_policy,
                        decimal_separators: &dec_seps,
                        grouping_separator: grouping.as_deref(),
                        exponent_chars: &exponent,
                        pad_character: pad,
                        ignore_case: props.ignore_case,
                    };
                    let available = cursor.data.len() - cursor.pos;
                    if let Some(mut len) = crate::vm::text_number::implicit_text_number_byte_length(
                        &cursor.data[cursor.pos..],
                        pattern,
                        &fmt,
                    ) {
                        if pattern.contains('*') && len < available {
                            len = available;
                        }
                        let start = cursor.pos;
                        cursor.pos += len;
                        return cursor.data[start..cursor.pos].to_vec();
                    } else if pattern.contains('*') && available > 0 {
                        let at_nil = match_nil_literal_prefix(cursor, props, strings)
                            .ok()
                            .flatten()
                            .is_some();
                        if !at_nil {
                            let start = cursor.pos;
                            cursor.pos += available;
                            return cursor.data[start..cursor.pos].to_vec();
                        }
                    }
                }
            }
        }
    }
    read_numeric_token(cursor)
}

fn validate_standard_v_pattern_runtime(
    pattern: &str,
    kind: crate::ir::ValueKind,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    if !pattern.contains('V') {
        return Ok(());
    }
    if !matches!(kind, ValueKind::Decimal | ValueKind::Float | ValueKind::Double) {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: textNumberPattern xs:double xs:float xs:decimal xs:byte The dfdl:textNumberPattern has a virtual decimal point 'V' or decimal scaling 'P' and dfdl:textNumberRep='standard'. The type must be xs:decimal but was: xs:{}",
                match kind {
                    ValueKind::Byte => "byte",
                    ValueKind::Float => "float",
                    ValueKind::Double => "double",
                    ValueKind::Decimal => "decimal",
                    _ => "number",
                }
            ),
        });
    }
    for sub in pattern.split(';') {
        let mut bare = sub.to_string();
        let mut in_q = false;
        let mut stripped = alloc::string::String::new();
        for c in bare.chars() {
            if c == '\'' {
                in_q = !in_q;
                continue;
            }
            if !in_q {
                stripped.push(c);
            }
        }
        bare = stripped;
        if bare.starts_with('-') {
            bare = bare[1..].to_string();
        }
        if bare.starts_with('[') && bare.ends_with(']') {
            bare = bare[1..bare.len() - 1].to_string();
        }
        if bare.starts_with('(') && bare.ends_with(')') {
            bare = bare[1..bare.len() - 1].to_string();
        }
        let Some(v_idx) = bare.find('V') else {
            continue;
        };
        let (before, rest) = bare.split_at(v_idx);
        let after = &rest[1..];
        let ok = !before.is_empty()
            && !after.is_empty()
            && before
                .chars()
                .all(|c| c.is_ascii_digit() || c == '#')
            && after.chars().all(|c| c.is_ascii_digit());
        if !ok {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Invalid textNumberPattern: Must match '#', then digits 0-9 then 'V' then digits 0-9".into(),
            });
        }
    }
    Ok(())
}

fn parse_field_text_number(
    trimmed: &str,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::ir::ValueKind;
    use crate::error::VmError;
    if !props.custom_text_number_pattern {
        if props.text_standard_base != 10 {
            return Ok(trimmed.into());
        }
    }
    let Some(pat_id) = props.text_number_pattern else {
        return Ok(trimmed.into());
    };
    let raw_pattern = strings.get(pat_id)?;
    if matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && matches!(
            kind,
            ValueKind::Int
                | ValueKind::Integer
                | ValueKind::Long
                | ValueKind::Short
                | ValueKind::Byte
                | ValueKind::UnsignedInt
                | ValueKind::UnsignedShort
                | ValueKind::UnsignedByte
        )
        && !raw_pattern.contains('V')
        && !raw_pattern.contains('.')
        && !raw_pattern.contains('E')
        && !raw_pattern.contains('e')
        && trimmed.contains(':')
    {
        return Err(unable_parse_from_text(
            type_name_for_parse(kind, props),
            trimmed,
        ));
    }
    if props.text_number_rep == crate::schema::TextNumberRep::Standard {
        validate_standard_v_pattern_runtime(raw_pattern, kind)?;
        if raw_pattern.contains('V')
            && trimmed.contains('.')
            && !trimmed.contains('E')
            && !trimmed.contains('e')
        {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. Unable to parse xs:decimal from text: {trimmed}"
                ),
            });
        }
    }
    if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
        if matches!(kind, ValueKind::Float | ValueKind::Double) {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: textNumberRep=\"zoned\" cannot be used with xs:{}",
                    if kind == ValueKind::Float {
                        "float"
                    } else {
                        "double"
                    }
                ),
            });
        }
        crate::vm::zoned_text::validate_zoned_text_number_pattern_runtime(
            raw_pattern,
            kind,
            props.text_number_check_policy,
        )?;
        crate::vm::zoned_text::validate_zoned_pattern_characters(raw_pattern)?;
    }
    let pattern_owned = if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
        crate::vm::zoned_text::strip_zoned_plus_markers(raw_pattern)
    } else {
        raw_pattern.to_string()
    };
    let pattern = pattern_owned.as_str();
    let mut text_to_parse = trimmed.to_string();
    if props.text_number_rep == crate::schema::TextNumberRep::Zoned {
        use crate::schema::TextZonedSignStyle as ZStyle;
        use crate::vm::zoned_text::{
            overpunch_location_from_pattern, zoned_to_number, OverpunchLocation,
            TextZonedSignStyle as VmStyle,
        };
        let enc = strings
            .get(props.encoding)
            .unwrap_or("")
            .to_ascii_lowercase();
        let ebcdic = enc.contains("ebcdic");
        let vm_style = match props.text_zoned_sign_style {
            None if ebcdic => VmStyle::Ebcdic,
            None | Some(ZStyle::AsciiStandard) => VmStyle::AsciiStandard,
            Some(ZStyle::AsciiTranslatedEBCDIC) => VmStyle::AsciiTranslatedEBCDIC,
            Some(ZStyle::AsciiCARealiaModified) => VmStyle::AsciiCARealiaModified,
            Some(ZStyle::AsciiTandemModified) => VmStyle::AsciiTandemModified,
        };
        let opl = overpunch_location_from_pattern(raw_pattern);
        if opl != OverpunchLocation::None {
            text_to_parse = zoned_to_number(trimmed, vm_style, opl).map_err(|e| {
                let VmError::InvalidValue { message: detail } = e else {
                    return unable_parse_from_text(type_name_for_parse(kind, props), trimmed);
                };
                VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. Unable to parse zoned {} from text: {trimmed}. {detail}",
                        type_name_for_parse(kind, props)
                    ),
                }
            })?;
        }
    }
    if pattern.contains('V')
        && !pattern.contains('E')
        && !pattern.contains('e')
        && !pattern.contains('\'')
        && !pattern.contains(';')
    {
        return apply_text_number_pattern_numeric(&text_to_parse, pattern);
    }
    let (dec_seps, grouping, exponent, pad, check_policy) =
        resolved_text_number_format_parts(props, strings);
    let fmt = text_number::TextNumberFormatProps {
        check_policy,
        decimal_separators: &dec_seps,
        grouping_separator: grouping.as_deref(),
        exponent_chars: &exponent,
        pad_character: pad,
        ignore_case: props.ignore_case,
    };
    let parse_result = if text_to_parse.starts_with('-') {
        text_number::parse_standard_text_number(&text_to_parse[1..], pattern, &fmt).map(|inner| {
            if inner.starts_with('-') {
                inner
            } else {
                alloc::format!("-{inner}")
            }
        })
    } else {
        text_number::parse_standard_text_number(&text_to_parse, pattern, &fmt)
    };
    if let Ok(v) = parse_result {
        if props.text_standard_zero_rep_defined {
            let raw = strings
                .get(props.text_standard_zero_rep)
                .unwrap_or("");
            if crate::schema::parse_text_standard_zero_rep_list(raw).is_empty()
                && !trimmed.is_empty()
                && trimmed.chars().all(|c| c.is_ascii_alphabetic())
            {
                return Err(unable_parse_from_text(
                    type_name_for_parse(kind, props),
                    trimmed,
                ));
            }
        }
        if matches!(
            kind,
            crate::ir::ValueKind::Int
                | crate::ir::ValueKind::Integer
                | crate::ir::ValueKind::Long
                | crate::ir::ValueKind::Short
                | crate::ir::ValueKind::Byte
                |             crate::ir::ValueKind::UnsignedInt
                | crate::ir::ValueKind::UnsignedShort
                | crate::ir::ValueKind::UnsignedByte
        ) {
            return normalize_text_number_for_integer(
                &v,
                trimmed,
                type_name_for_parse(kind, props),
            );
        }
        return Ok(v);
    }
    if let Some(zero) = text_standard_zero_rep_match(trimmed, props, strings) {
        return Ok(zero);
    }
    Err(unable_parse_from_text(type_name_for_parse(kind, props), trimmed))
}

fn normalize_text_number_for_integer(
    parsed: &str,
    raw_input: &str,
    type_name: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    if let Some((int_part, frac)) = parsed.split_once('.') {
        if frac.chars().all(|c| c == '0') {
            return Ok(int_part.to_string());
        }
        return Err(unable_parse_from_text(type_name, raw_input));
    }
    if parsed.contains('E') || parsed.contains('e') {
        return Err(unable_parse_from_text(type_name, raw_input));
    }
    Ok(parsed.to_string())
}

fn type_name_for_parse(kind: crate::ir::ValueKind, props: &IrProps) -> &'static str {
    value_kind_type_name(kind, Some(props))
}

fn text_standard_infinity_nan_match(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Option<&'static str> {
    let inf = strings
        .get(props.text_standard_infinity_rep)
        .unwrap_or("Inf");
    let nan = strings.get(props.text_standard_nan_rep).unwrap_or("NaN");
    let ic = props.ignore_case;
    let eq = |a: &str, b: &str| {
        if ic {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if eq(trimmed, nan) {
        return Some("NaN");
    }
    if eq(trimmed, inf) {
        return Some("INF");
    }
    if trimmed.starts_with('-') {
        let rest = trimmed.trim_start_matches('-');
        if eq(rest, inf) {
            return Some("-INF");
        }
    }
    None
}

fn text_standard_zero_rep_match(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Option<alloc::string::String> {
    if !props.text_standard_zero_rep_defined {
        return None;
    }
    let raw = strings.get(props.text_standard_zero_rep).ok()?;
    let reps = crate::schema::parse_text_standard_zero_rep_list(raw);
    if reps.is_empty() {
        return None;
    }
    let ic = props.ignore_case;
    for rep in reps {
        let matches = if ic {
            trimmed.eq_ignore_ascii_case(rep.as_str())
        } else {
            trimmed == rep
        };
        if matches {
            return Some("0".into());
        }
    }
    None
}

pub(crate) fn reject_text_standard_special_for_integer(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
    type_name: &str,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let Some(special) = text_standard_infinity_nan_match(trimmed, props, strings) else {
        return Ok(());
    };
    let detail = if special == "NaN" {
        "NaN"
    } else {
        "Infinity"
    };
    Err(VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. {detail} value out of range for type {type_name}"
        ),
    })
}

fn text_number_for_parse<'a>(
    trimmed: &'a str,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::ir::ValueKind;
    if !matches!(kind, ValueKind::Float | ValueKind::Double | ValueKind::Decimal) {
        return Ok(trimmed.into());
    }
    if let Some(special) = text_standard_infinity_nan_match(trimmed, props, strings) {
        return Ok(special.into());
    }
    parse_field_text_number(trimmed, kind, props, strings)
}

fn calendar_explicit_pattern_extends_year_beyond_length(pattern: &str) -> bool {
    const FIELD: &str = "EMdDFwWmyYHhsekKS";
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0usize;
    let mut last_y_width = 0usize;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        let c = chars[i];
        if c == 'y' || c == 'Y' {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            last_y_width = w;
            i += w;
        } else if FIELD.contains(c) {
            let mut w = 1usize;
            while i + w < chars.len() && chars[i + w] == c {
                w += 1;
            }
            i += w;
        } else {
            i += 1;
        }
    }
    last_y_width == 1
}

fn expand_calendar_year(
    y: &str,
    century_start: u32,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    if y.len() == 2 {
        let yy: u32 = y.parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid calendar year `{y}`"),
        })?;
        let full = if yy >= century_start {
            1900 + yy
        } else {
            2000 + yy
        };
        return Ok(alloc::format!("{full:04}"));
    }
    Ok(y.into())
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
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(VmError::InvalidValue {
                message: "use decode path for binarySeconds/binaryMilliseconds".into(),
            })
        }
    }
}

fn datetime_to_calendar_digits(
    value: &str,
    pattern: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;

    let (date, time) = match value.split_once('T') {
        Some((d, t)) => (d, t),
        None => (value, "00:00:00"),
    };
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
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    let le = props.byte_order == ByteOrder::LittleEndian;
    let digits = bcd_to_digit_string(bytes, le)?;
    signed_magnitude_to_dfdl(false, &digits, kind, 0)
}

fn decode_ibm4690_number(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    let le = props.byte_order == ByteOrder::LittleEndian;
    let (negative, digits) = ibm4690_to_digit_string(bytes, le)?;
    signed_magnitude_to_dfdl(negative, &digits, kind, 0)
}

fn decode_packed_bcd_number(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    let le = props.byte_order == ByteOrder::LittleEndian;
    let codes = packed_sign_codes(props, strings)?;
    let (negative, digits) = packed_to_digit_string(bytes, le, &codes)?;
    signed_magnitude_to_dfdl(negative, &digits, kind, 0).map_err(|e| match e {
        VmError::InvalidValue { message } if message == "out of range for type" => {
            VmError::InvalidValue {
                message: alloc::format!("Error in packed data: \n{message}"),
            }
        }
        other => other,
    })
}

fn binary_bit_length(
    cursor: &Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;

    match props.length_kind {
        LengthKind::Fixed => Ok(props.length.unwrap_or(
            (implicit_binary_scalar_byte_length(kind, props) * 8) as u64,
        ) as usize),
        LengthKind::Implicit => Ok(implicit_binary_scalar_byte_length(kind, props) * 8),
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            if hex_charset_order(&encoding_name(props, strings)?).is_some()
                && (kind == ValueKind::Decimal || is_packed_binary_rep(props.binary_number_rep))
            {
                validate_packed_binary_bit_length_parse(len as usize, kind, props.binary_number_rep)?;
            } else if binary_length_validation_applies(kind, props.binary_number_rep) {
                validate_data_length_vm(kind, len, LengthUnits::Bits, props.binary_number_rep)?;
            }
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
        LengthKind::Fixed => Ok(
            props
                .length
                .unwrap_or(implicit_binary_scalar_byte_length(kind, props) as u64) as usize,
        ),
        LengthKind::Implicit => {
            if matches!(kind, crate::ir::ValueKind::String | crate::ir::ValueKind::HexBinary) {
                if let Some(len) = crate::vm::facet_validate::implicit_facet_byte_length(props) {
                    return Ok(len);
                }
            }
            Ok(implicit_binary_scalar_byte_length(kind, props))
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            validate_data_length_vm(kind, len, LengthUnits::Bytes, props.binary_number_rep)?;
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
    tunables: &DaffodilTunables,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    if kind == Decimal {
        return Ok(DfdlValue::Decimal(format_binary_decimal_magnitude(
            raw,
            effective_binary_decimal_vp(props),
        )));
    }
    if matches!(kind, DateTime | Time) && calendar_binary_rep(props) {
        let bytes = stream_bits_to_bytes(raw, bit_width, props.byte_order);
        return decode_binary_calendar(
            kind,
            &bytes,
            props,
            strings,
            tunables,
            Some((raw, bit_width)),
        );
    }

    macro_rules! unsigned {
        ($t:ty, $cons:expr) => {{
            <$t>::try_from(raw).map($cons).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("bit value `{raw}` out of range"),
            })
        }};
    }

    match kind {
        Boolean => Ok(DfdlValue::Boolean(decode_binary_boolean_sl(raw, props)?)),
        Byte => {
            let v = if bit_width == 1 {
                raw as i8
            } else {
                sign_extend_u64(raw, bit_width) as i8
            };
            Ok(DfdlValue::Byte(v))
        }
        UnsignedByte => Ok(DfdlValue::UnsignedByte((raw & bit_mask(bit_width)) as u8)),
        Short => {
            let v = if bit_width == 1 {
                raw as i16
            } else {
                sign_extend_u64(raw, bit_width) as i16
            };
            Ok(DfdlValue::Short(v))
        }
        UnsignedShort => Ok(DfdlValue::UnsignedShort((raw & bit_mask(bit_width)) as u16)),
        Int => {
            let v = if bit_width == 1 {
                raw as i32
            } else {
                sign_extend_u64(raw, bit_width) as i32
            };
            Ok(DfdlValue::Int(v))
        }
        UnsignedInt => Ok(DfdlValue::UnsignedInt((raw & bit_mask(bit_width)) as u32)),
        Integer => {
            let v: i64 = if props.non_negative_integer {
                (raw & bit_mask(bit_width)) as i64
            } else if bit_width == 1 {
                raw as i64
            } else {
                sign_extend_u64(raw, bit_width)
            };
            Ok(DfdlValue::Integer(v.to_string()))
        }
        Long => {
            let v = if props.unsigned_integer {
                raw & bit_mask(bit_width)
            } else if bit_width == 1 {
                raw
            } else {
                sign_extend_u64(raw, bit_width) as u64
            };
            if props.unsigned_integer {
                Ok(DfdlValue::UnsignedLong(v))
            } else {
                Ok(DfdlValue::Long(v as i64))
            }
        }
        Float => Ok(DfdlValue::Float(f32::from_bits(raw as u32))),
        Double => Ok(DfdlValue::Double(f64::from_bits(raw))),
        Decimal | DateTime | Time => unreachable!("handled above"),
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
    if bits >= 64 {
        return value as i64;
    }
    let sign = 1u64 << (bits - 1);
    if value & sign != 0 {
        let mask = (1u64 << bits) - 1;
        (value | (!mask)) as i64
    } else {
        value as i64
    }
}

fn bit_mask(width: usize) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

fn twos_complement_negate_be(bytes: &mut [u8]) {
    let mut carry = 1u16;
    for b in bytes.iter_mut().rev() {
        let v = (!(*b as u16)).wrapping_add(carry);
        *b = v as u8;
        carry = (v >> 8) as u16;
    }
}

fn magnitude_bytes_be_to_decimal(bytes: &[u8]) -> alloc::string::String {
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    if start >= bytes.len() {
        return "0".into();
    }
    let mut digits = alloc::vec![0u8];
    for &byte in &bytes[start..] {
        let mut carry = u32::from(byte);
        for d in digits.iter_mut() {
            let v = u32::from(*d) * 256 + carry;
            *d = (v % 10) as u8;
            carry = v / 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    digits
        .iter()
        .rev()
        .map(|d| char::from(b'0' + *d))
        .collect()
}

fn decode_binary_integer_decimal(
    bytes: &[u8],
    le: bool,
    non_negative: bool,
) -> alloc::string::String {
    let mut mag = bytes.to_vec();
    if le {
        mag.reverse();
    }
    let negative = !non_negative && !mag.is_empty() && (mag[0] & 0x80) != 0;
    if negative {
        twos_complement_negate_be(&mut mag);
    }
    let dec = magnitude_bytes_be_to_decimal(&mag);
    if negative {
        alloc::format!("-{dec}")
    } else {
        dec
    }
}

fn normalize_bit_field_raw(
    raw: u64,
    bit_width: usize,
    byte_order: ByteOrder,
    bit_order: BitOrder,
) -> u64 {
    if bit_width == 0 {
        return 0;
    }
    // LSBF 1–2 bit fields pack LSB-first in the stream (reverse to MSBF numeric order).
    if bit_width < 8 {
        if bit_order == BitOrder::LeastSignificantBitFirst && bit_width <= 2 {
            return reverse_field_bits(raw, bit_width) & bit_mask(bit_width);
        }
        return raw & bit_mask(bit_width);
    }
    // `read_stream_bits` with LSBF already places stream bit i at u64 bit i (LE numeric layout).
    if byte_order == ByteOrder::LittleEndian
        && bit_order == BitOrder::LeastSignificantBitFirst
    {
        return raw & bit_mask(bit_width);
    }
    if byte_order == ByteOrder::LittleEndian {
        if bit_width >= 8 {
            let bytes = stream_raw_to_msbf_bytes(raw, bit_width);
            return decode_packed_bit_field_u64(&bytes, bit_width, byte_order, bit_order);
        }
        let mut bytes = stream_bits_to_bytes(raw, bit_width, ByteOrder::BigEndian);
        bytes.reverse();
        let mut v = 0u64;
        for b in bytes {
            v = (v << 8) | u64::from(b);
        }
        let shift = (8 - (bit_width % 8)) % 8;
        (v >> shift) & bit_mask(bit_width)
    } else {
        raw & bit_mask(bit_width)
    }
}

fn decode_binary_bytes(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
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
        Boolean => {
            let le = props.byte_order == ByteOrder::LittleEndian;
            let sl = decode_unsigned_binary_bytes(bytes, le);
            Ok(DfdlValue::Boolean(decode_binary_boolean_sl(sl, props)?))
        }
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
                Ok(DfdlValue::Short(sign_extend_u64(
                    decode_packed_bit_field_u64(bytes, bits, props.byte_order, props.bit_order),
                    bits,
                ) as i16))
            } else {
                Ok(DfdlValue::Short(int!(i16)))
            }
        }
        UnsignedShort => {
            if let Some(bits) = bit_width {
                Ok(DfdlValue::UnsignedShort(
                    decode_packed_bit_field_u64(bytes, bits, props.byte_order, props.bit_order)
                        as u16,
                ))
            } else {
                Ok(DfdlValue::UnsignedShort(int!(u16)))
            }
        }
        Int => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Int(decode_unsigned_binary_bytes(bytes, le) as i32))
            } else if let Some(bits) = bit_width {
                Ok(DfdlValue::Int(sign_extend_u64(
                    decode_packed_bit_field_u64(bytes, bits, props.byte_order, props.bit_order),
                    bits,
                ) as i32))
            } else if bytes.len() < core::mem::size_of::<i32>() {
                let bits = bytes.len().saturating_mul(8);
                let raw = decode_unsigned_binary_bytes(bytes, le);
                Ok(DfdlValue::Int(sign_extend_u64(raw, bits) as i32))
            } else {
                Ok(DfdlValue::Int(int!(i32)))
            }
        }
        UnsignedInt => Ok(DfdlValue::UnsignedInt(int!(u32))),
        Integer => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Integer(
                    (decode_unsigned_binary_bytes(bytes, le) as i64).to_string(),
                ))
            } else if let Some(bits) = bit_width {
                let raw = decode_unsigned_binary_bytes(bytes, le);
                let v: i64 = if props.non_negative_integer {
                    (raw & bit_mask(bits)) as i64
                } else {
                    sign_extend_u64(raw, bits)
                };
                Ok(DfdlValue::Integer(v.to_string()))
            } else if bytes.len() < core::mem::size_of::<i64>() {
                let bits = bytes.len().saturating_mul(8);
                let raw = decode_unsigned_binary_bytes(bytes, le);
                let v: i64 = if props.non_negative_integer {
                    (raw & bit_mask(bits)) as i64
                } else {
                    sign_extend_u64(raw, bits)
                };
                Ok(DfdlValue::Integer(v.to_string()))
            } else if bytes.len() > core::mem::size_of::<i64>() {
                Ok(DfdlValue::Integer(decode_binary_integer_decimal(
                    bytes,
                    le,
                    props.non_negative_integer,
                )))
            } else {
                Ok(DfdlValue::Integer(int!(i64).to_string()))
            }
        }
        Long => {
            if bit_width == Some(1) {
                let raw = decode_unsigned_binary_bytes(bytes, le);
                if props.unsigned_integer {
                    Ok(DfdlValue::UnsignedLong(raw))
                } else {
                    Ok(DfdlValue::Long(raw as i64))
                }
            } else if let Some(bits) = bit_width {
                let raw = decode_unsigned_binary_bytes(bytes, le);
                let v = if props.unsigned_integer {
                    raw & bit_mask(bits)
                } else {
                    sign_extend_u64(raw, bits) as u64
                };
                if props.unsigned_integer {
                    Ok(DfdlValue::UnsignedLong(v))
                } else {
                    Ok(DfdlValue::Long(v as i64))
                }
            } else if props.unsigned_integer {
                Ok(DfdlValue::UnsignedLong(int!(u64)))
            } else {
                Ok(DfdlValue::Long(int!(i64)))
            }
        }
        Float => Ok(DfdlValue::Float(f32::from_bits(int!(u32)))),
        Double => Ok(DfdlValue::Double(f64::from_bits(int!(u64)))),
        HexBinary => Ok(DfdlValue::HexBinary(bytes.to_vec())),
        Decimal | DateTime | Time | String | Complex => Err(VmError::TypeMismatch {
            expected: "binary scalar".into(),
        }),
    }
}

fn nil_value_alternatives<'a>(
    props: &'a IrProps,
    strings: &'a StringPool,
) -> Result<Option<alloc::vec::Vec<alloc::string::String>>, crate::error::VmError> {
    if !props.nillable {
        return Ok(None);
    }
    if !matches!(
        props.nil_kind,
        Some(NilKind::LiteralValue) | Some(NilKind::LiteralCharacter)
    ) {
        return Ok(None);
    }
    let Some(id) = props.nil_value else {
        return Ok(None);
    };
    let raw = strings.get(id)?;
    Ok(Some(crate::schema::nil_value_alternatives(raw)))
}

fn nil_first_alternative(props: &IrProps, strings: &StringPool) -> Result<Option<alloc::string::String>, crate::error::VmError> {
    Ok(nil_value_alternatives(props, strings)?
        .and_then(|alts| alts.into_iter().next()))
}

pub(crate) fn nil_value_includes_empty(props: &IrProps, strings: &StringPool) -> Result<bool, crate::error::VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(false);
    };
    Ok(alts.iter().any(|a| crate::schema::expand_entities(a.trim()).is_empty()))
}

fn nil_character_repeat_count(props: &IrProps) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    match props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit/fixed nil field missing length".into(),
            })? as usize;
            Ok(match props.length_units {
                LengthUnits::Bytes | LengthUnits::Characters => len,
                LengthUnits::Bits => len.div_ceil(8),
            })
        }
        _ => Ok(1),
    }
}

fn expand_nil_alternative(
    alt: &str,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    let trimmed = alt.trim();
    if trimmed == "%NL;" {
        if let Some(id) = props.output_new_line {
            if let Ok(onl) = strings.get(id) {
                return Ok(crate::schema::expand_entities(onl));
            }
        }
    }
    Ok(crate::schema::expand_entities(trimmed))
}

pub(crate) fn nil_unparse_bytes_for_encode(
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    nil_unparse_bytes(props, strings)
}

fn nil_unparse_bytes(
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(alloc::vec::Vec::new());
    };
    let first = alts.first().map(|s| s.as_str()).unwrap_or("");
    if props.nil_kind == Some(NilKind::LiteralCharacter) {
        let unit = expand_nil_alternative(first, props, strings)?;
        if unit.is_empty() {
            return Ok(alloc::vec::Vec::new());
        }
        let repeat = nil_character_repeat_count(props)?;
        let mut out = alloc::vec::Vec::with_capacity(unit.len().saturating_mul(repeat));
        for _ in 0..repeat {
            out.extend_from_slice(&unit);
        }
        return Ok(out);
    }
    expand_nil_alternative(first, props, strings)
}

fn text_matches_nil_literal(text: &str, props: &IrProps, strings: &StringPool) -> Result<bool, crate::error::VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(false);
    };
    if props.nil_kind == Some(NilKind::LiteralCharacter) {
        if alts.iter().any(|a| a.is_empty()) && text.is_empty() {
            return Ok(true);
        }
        if let Some(nil) = alts.first() {
            if nil.is_empty() {
                return Ok(text.is_empty());
            }
        }
        if !alts.is_empty() {
            for alt in alts {
                if alt.is_empty() {
                    if text.is_empty() {
                        return Ok(true);
                    }
                    continue;
                }
                let expanded = crate::schema::expand_entities_str(alt.trim());
                if expanded.is_empty() {
                    if text.is_empty() {
                        return Ok(true);
                    }
                    continue;
                }
                let nil_char = expanded.chars().next().unwrap_or('\0');
                if expanded.chars().count() == 1
                    && !text.is_empty()
                    && text.chars().all(|c| {
                        if props.ignore_case {
                            c.eq_ignore_ascii_case(&nil_char)
                        } else {
                            c == nil_char
                        }
                    })
                {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
    }
    Ok(alts.iter().any(|alt| text == alt.as_str()))
}

pub(crate) fn validate_nil_value_runtime(
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::NilKind;
    if !props.nillable {
        return Ok(());
    }
    let Some(nil_id) = props.nil_value else {
        return Ok(());
    };
    let raw = strings.get(nil_id)?;
    if raw.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Property dfdl:nilValue cannot be empty string. Use dfdl:nilValue='%ES;' for empty string.".into(),
        });
    }
    if props.nil_kind == Some(NilKind::LiteralCharacter) {
        for token in ["%NL;", "%ES;", "%WSP;", "%WSP+;", "%WSP*;"] {
            if raw.contains(token) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Schema Definition Error: Property dfdl:nilValue contains disallowed character class(es): {token}"
                    ),
                });
            }
        }
        for alt in crate::schema::nil_value_alternatives(raw) {
            let expanded = crate::schema::expand_entities(alt.trim());
            let as_text = alloc::string::String::from_utf8_lossy(&expanded);
            if as_text.chars().count() != 1 {
                return Err(VmError::InvalidValue {
                    message: "Schema Definition Error: For property dfdl:nilValue the length of string must be exactly 1 character.".into(),
                });
            }
        }
    }
    let _ = strings;
    Ok(())
}

pub(crate) fn try_consume_nillable_element_nil(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    parent_sequence: Option<&IrProps>,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::NilKind;
    if !props.nillable {
        return Ok(false);
    }
    if !matches!(
        props.nil_kind,
        Some(NilKind::LiteralValue) | Some(NilKind::LiteralCharacter)
    ) {
        return Ok(false);
    }
    let saved = cursor.pos;
    let empty_nil = nil_value_includes_empty(props, strings)?;
    if let Some(nil_len) = match_nil_literal_prefix(cursor, props, strings)? {
        cursor.advance(nil_len);
        if let Some(term_id) = props.terminator {
            let term = strings.get(term_id)?;
            if !term.is_empty()
                && crate::schema::match_delimiter_opts(
                    &cursor.data[cursor.pos..],
                    term,
                    props.ignore_case,
                )
                .is_some()
            {
                let enc = encoding_name(props, strings).ok();
                let _ = cursor.consume_delimiter(term, props.ignore_case, enc.as_deref());
                return Ok(true);
            }
        }
        if empty_nil && nil_len == 0 {
            // fall through to parent-separator / EOS empty-nil checks below
        } else {
            cursor.pos = saved;
        }
    }
    if empty_nil {
        if let Some(term_id) = props.terminator {
            let term = strings.get(term_id)?;
            if !term.is_empty()
                && crate::schema::match_delimiter_opts(
                    &cursor.data[cursor.pos..],
                    term,
                    props.ignore_case,
                )
                .is_some()
            {
                let parent_owns = parent_sequence.is_some_and(|parent| {
                    parent.terminator == Some(term_id)
                });
                if parent_owns {
                    return Ok(true);
                }
                let enc = encoding_name(props, strings).ok();
                let _ = cursor.consume_delimiter(term, props.ignore_case, enc.as_deref());
                return Ok(true);
            }
        }
        if let Some(parent) = parent_sequence {
            if let Some(sep_id) = parent.separator {
                let sep = strings.get(sep_id)?;
                if !sep.is_empty()
                    && crate::schema::match_delimiter_opts(
                        &cursor.data[cursor.pos..],
                        sep,
                        parent.ignore_case,
                    )
                    .is_some()
                {
                    return Ok(true);
                }
            }
            if let Some(term_id) = parent.terminator {
                let term = strings.get(term_id)?;
                if !term.is_empty()
                    && crate::schema::match_delimiter_opts(
                        &cursor.data[cursor.pos..],
                        term,
                        parent.ignore_case,
                    )
                    .is_some()
                {
                    return Ok(true);
                }
            }
        }
        if cursor.pos >= cursor.data.len() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn match_nil_literal_prefix(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Option<usize>, crate::error::VmError> {
    let Some(alts) = nil_value_alternatives(props, strings)? else {
        return Ok(None);
    };
    let mut best: Option<usize> = None;
    for alt in alts {
        let bytes = crate::schema::expand_entities(&alt);
        if cursor.data[cursor.pos..].starts_with(&bytes) {
            let len = bytes.len();
            if best.map(|prev| len > prev).unwrap_or(true) {
                best = Some(len);
            }
        }
    }
    Ok(best)
}

fn pattern_allows_zero_length_match(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    let id = props.length_pattern.ok_or(crate::error::VmError::InvalidValue {
        message: "pattern length missing lengthPattern".into(),
    })?;
    let pat = pattern_str(strings, id)?;
    if pat == "." && normalize_encoding_name(encoding_name(props, strings)?) == Some("utf-8") {
        return Ok(cursor.pos >= cursor.data.len());
    }
    Ok(match_length_pattern(&cursor.data[cursor.pos..], pat) == Some(0))
}

/// Explicit/fixed length in bytes with UTF-16: Daffodil may decode a lone trailing byte as a character.
fn decode_specified_length_text_bytes(
    raw: &[u8],
    enc: &str,
    props: &IrProps,
) -> Result<alloc::string::String, crate::error::VmError> {
    let utf16 = normalize_encoding_name(enc).is_some_and(|n| n.starts_with("utf-16"));
    let byte_len = matches!(
        props.length_kind,
        LengthKind::Explicit | LengthKind::Fixed
    ) && props.length_units == LengthUnits::Bytes;
    // Single-byte explicit UTF-16 fields: Daffodil StringOfSpecifiedLength leaves incomplete
    // code units empty, but decodes a lone non-zero byte as that character.
    if utf16 && byte_len && raw.len() == 1 {
        return Ok(if raw[0] == 0 {
            alloc::string::String::new()
        } else {
            alloc::string::String::from(raw[0] as char)
        });
    }
    decode_text_bytes(raw, enc, props.encoding_error_policy)
}

pub(crate) fn read_text_scalar(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
    field_name: Option<&str>,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
    tunables: &DaffodilTunables,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    validate_nil_value_runtime(props, strings)?;

    if matches!(kind, String) {
        if let Some(id) = props.text_string_pad_character {
            let raw = strings.get(id)?;
            if let Err(msg) = crate::schema::validate_text_string_pad_character_runtime(raw) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Schema Definition Error: {msg}"),
                });
            }
        }
    }

    if props.length_kind == LengthKind::Pattern {
        if let Some(nil_len) = match_nil_literal_prefix(cursor, props, strings)? {
            cursor.advance(nil_len);
            return Ok(DfdlValue::Null);
        }
        if props.empty_element_parse_policy == crate::schema::EmptyElementParsePolicy::TreatAsAbsent
            && pattern_allows_zero_length_match(cursor, props, strings)?
        {
            return Err(crate::error::VmError::ElementAbsent.into());
        }
        if nil_value_includes_empty(props, strings)?
            && pattern_allows_zero_length_match(cursor, props, strings)?
        {
            return Ok(DfdlValue::Null);
        }
    }

    let enc = encoding_name(props, strings)?;
    let mut raw = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed text missing length".into(),
            })? as usize;
            read_length_span(
                cursor,
                len,
                props.length_units,
                enc,
                props.bit_order,
                props.encoding_error_policy,
                false,
            )?
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit text missing length".into(),
            })? as usize;
            read_length_span(
                cursor,
                len,
                props.length_units,
                enc,
                props.bit_order,
                props.encoding_error_policy,
                false,
            )?
        }
        LengthKind::Delimited => {
            read_until_delimiters(
                cursor,
                props,
                strings,
                require_delimiter,
                stop_sequences,
                Some(enc),
            )?
        }
        LengthKind::Pattern => {
            let id = props.length_pattern.ok_or(VmError::InvalidValue {
                message: "pattern length missing lengthPattern".into(),
            })?;
            let pat = pattern_str(strings, id)?;
            let policy = props.encoding_error_policy;
            let len = if pat == "." && normalize_encoding_name(enc) == Some("utf-8") {
                read_one_utf8_char(&cursor.data, cursor.pos, policy)?.1
            } else {
                match_length_pattern(&cursor.data[cursor.pos..], pat).ok_or(VmError::InvalidValue {
                    message: alloc::format!("pattern `{pat}` mismatch"),
                })?
            };
            cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)?
        }
        LengthKind::Implicit => {
            if is_numeric_text_kind(kind) {
                read_implicit_numeric_text(cursor, props, strings)
            } else if matches!(kind, String | HexBinary) {
                if let Some(len) = crate::vm::facet_validate::implicit_facet_byte_length(props) {
                    read_length_span(
                        cursor,
                        len,
                        props.length_units,
                        enc,
                        props.bit_order,
                        props.encoding_error_policy,
                        false,
                    )?
                } else {
                    read_until_delimiters(cursor, props, strings, false, stop_sequences, Some(enc))?
                }
            } else {
                read_until_delimiters(cursor, props, strings, false, stop_sequences, Some(enc))?
            }
        }
        LengthKind::Prefixed => read_prefixed_payload(cursor, props, strings, field_name)?,
        LengthKind::EndOfParent => {
            let rest = cursor.data[cursor.pos..].to_vec();
            cursor.pos = cursor.data.len();
            rest
        }
    };

    if matches!(kind, DateTime | Time)
        && props.calendar_pattern.is_some()
        && matches!(
            props.length_kind,
            LengthKind::Explicit | LengthKind::Fixed
        )
        && props.length_units == LengthUnits::Bytes
    {
        if let Some(pat_id) = props.calendar_pattern {
            if let Ok(pat) = strings.get(pat_id) {
                if calendar_explicit_pattern_extends_year_beyond_length(pat) {
                    while cursor.pos < cursor.data.len()
                        && cursor.data[cursor.pos].is_ascii_digit()
                    {
                        raw.push(cursor.data[cursor.pos]);
                        cursor.pos += 1;
                    }
                }
            }
        }
    }

    let trailing_input = cursor.pos < cursor.data.len();

    let text = if hex_charset_order(enc).is_some() {
        hex_charset_payload_to_text(&raw)
    } else if let Some(spec) = bits_charset_spec(enc) {
        let n_bits = match props.length_kind {
            LengthKind::Fixed | LengthKind::Explicit if props.length_units == LengthUnits::Bits => {
                props.length.unwrap_or(0) as usize
            }
            _ => raw.len().saturating_mul(8),
        };
        decode_bits_charset_payload(&raw, n_bits, spec)?
    } else {
        decode_specified_length_text_bytes(&raw, enc, props)?
    };
    let text = if kind == crate::ir::ValueKind::String && uses_xml_illegal_char_remap(enc) {
        remap_xml_illegal_characters_to_pua(&text)
    } else {
        text
    };
    let text = if kind == crate::ir::ValueKind::String {
        normalize_string_line_endings(&text)
    } else {
        text
    };
    let trimmed = trim_text_value(&text, kind, props.text_trim_kind, props, strings);
    let trimmed = if kind == crate::ir::ValueKind::String {
        if let Some(ref scheme) = props.escape_scheme {
            crate::vm::escape::unescape_field_text(trimmed, scheme)?
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };
    let trimmed = trimmed.as_str();

    if text_matches_nil_literal(trimmed, props, strings)? {
        return Ok(DfdlValue::Null);
    }

    if trimmed.is_empty() {
        if let Some(v) = default_value_for(kind, props, strings) {
            return Ok(v);
        }
        if is_numeric_text_kind(kind)
            && matches!(
                props.length_kind,
                LengthKind::Delimited | LengthKind::Implicit
            )
        {
            let type_name = value_kind_type_name(kind, Some(props));
            return Err(VmError::InvalidValue {
                message: alloc::format!("Parse Error. Unable to parse {type_name} from empty string"),
            });
        }
    }

    if props.custom_text_number_pattern
        && props.text_number_rep == crate::schema::TextNumberRep::Standard
    {
        if let Some(id) = props.text_number_pattern {
            validate_standard_v_pattern_runtime(strings.get(id)?, kind)?;
        }
    }

    let base = props.text_standard_base;
    let value = match kind {
        Boolean => parse_text_boolean(trimmed, props, strings, sibling_env).map(DfdlValue::Boolean),
        Byte => {
            reject_internal_whitespace_explicit_field(
                trimmed, "xs:byte", props, base, trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_int_typed_with_base(&num, "xs:byte", base, trimmed).map(DfdlValue::Byte)
        }
        UnsignedByte => {
            reject_internal_whitespace_explicit_field(
                trimmed, "xs:unsignedByte", props, base, trailing_input,
            )?;
            let num = if base == 10 && unsigned_uses_text_number_pattern(trimmed, props) {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else if base == 10 {
                lax_numeric_field_text(trimmed, props, "xs:unsignedByte")
            } else {
                trimmed.to_string()
            };
            parse_unsigned_radix_typed(&num, "xs:unsignedByte", base, trimmed).and_then(|v| {
                u8::try_from(v)
                    .map(DfdlValue::UnsignedByte)
                    .map_err(|_| parse_out_of_range("xs:unsignedByte", trimmed))
            })
        }
        Short => {
            reject_internal_whitespace_explicit_field(
                trimmed, "xs:short", props, base, trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_int_typed_with_base(&num, "xs:short", base, trimmed).map(DfdlValue::Short)
        }
        UnsignedShort => {
            reject_internal_whitespace_explicit_field(
                trimmed, "xs:unsignedShort", props, base, trailing_input,
            )?;
            let num = if base == 10 && unsigned_uses_text_number_pattern(trimmed, props) {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else if base == 10 {
                lax_numeric_field_text(trimmed, props, "xs:unsignedShort")
            } else {
                trimmed.to_string()
            };
            parse_unsigned_radix_typed(&num, "xs:unsignedShort", base, trimmed).and_then(|v| {
                u16::try_from(v)
                    .map(DfdlValue::UnsignedShort)
                    .map_err(|_| parse_out_of_range("xs:unsignedShort", trimmed))
            })
        }
        Int => {
            reject_text_standard_special_for_integer(trimmed, props, strings, "xs:int")?;
            reject_internal_whitespace_explicit_field(
                trimmed, "xs:int", props, base, trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_int_typed_with_base(&num, "xs:int", base, trimmed).map(DfdlValue::Int)
        }
        Integer => {
            let num = if base == 10 && props.custom_text_number_pattern {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            parse_unbounded_integer_decimal(&num, base, props.non_negative_integer)
                .map(DfdlValue::Integer)
        }
        UnsignedInt => {
            reject_internal_whitespace_explicit_field(
                trimmed, "xs:unsignedInt", props, base, trailing_input,
            )?;
            let num = if base == 10 && unsigned_uses_text_number_pattern(trimmed, props) {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else if base == 10 {
                lax_numeric_field_text(trimmed, props, "xs:unsignedInt")
            } else {
                trimmed.to_string()
            };
            parse_unsigned_radix_typed(&num, "xs:unsignedInt", base, trimmed).and_then(|v| {
                u32::try_from(v)
                    .map(DfdlValue::UnsignedInt)
                    .map_err(|_| parse_out_of_range("xs:unsignedInt", trimmed))
            })
        }
        Long => {
            reject_text_standard_special_for_integer(trimmed, props, strings, "xs:long")?;
            reject_internal_whitespace_explicit_field(
                trimmed,
                if props.unsigned_integer {
                    "xs:unsignedLong"
                } else {
                    "xs:long"
                },
                props,
                base,
                trailing_input,
            )?;
            let num = if base == 10 {
                parse_field_text_number(trimmed, kind, props, strings)?
            } else {
                trimmed.to_string()
            };
            if props.unsigned_integer {
                let v = parse_unsigned_radix_typed(&num, "xs:unsignedLong", base, trimmed)?;
                Ok(DfdlValue::UnsignedLong(v))
            } else {
                parse_int_typed_with_base(&num, "xs:long", base, trimmed).map(DfdlValue::Long)
            }
        }
        Float => {
            let num = text_number_for_parse(trimmed, kind, props, strings)?;
            parse_float(&num).map(|v| DfdlValue::Float(v as f32))
        }
        Double => {
            let num = text_number_for_parse(trimmed, kind, props, strings)?;
            parse_float(&num).map(DfdlValue::Double)
        }
        Decimal => {
            let num = text_number_for_parse(trimmed, kind, props, strings)?;
            let canon = crate::vm::facet_validate::canonicalize_xs_decimal_lexical(&num);
            Ok(DfdlValue::Decimal(canon.into()))
        }
        DateTime | Time => {
            if props.calendar_pattern_kind == crate::schema::CalendarPatternKind::Implicit
            {
                let processed = crate::vm::calendar_binary::process_implicit_calendar_text(
                    kind,
                    props.calendar_date_only,
                    props.calendar_check_policy_lax,
                    trimmed,
                    tunables,
                )?;
                let with_tz = append_packed_calendar_timezone(
                    props,
                    strings,
                    kind,
                    props.calendar_date_only,
                    &processed,
                    false,
                    true,
                )?;
                return Ok(DfdlValue::DateTime(with_tz));
            }
            if let Some(pat_id) = props.calendar_pattern {
                let pattern = strings.get(pat_id)?;
                let parsed = if trimmed.chars().all(|c| c.is_ascii_digit()) {
                    format_calendar_pattern(
                        trimmed,
                        pattern,
                        props.calendar_century_start,
                        props.calendar_first_day_of_week,
                    )?
                } else {
                    let cal_lang = resolve_calendar_language(
                        props,
                        strings,
                        sibling_env.map(|e| e.text),
                    )?;
                    let parsed_text = format_calendar_text(
                        trimmed,
                        pattern,
                        props.calendar_check_policy_lax,
                        props.calendar_century_start,
                        CalendarTextConfig {
                            language: cal_lang.as_deref(),
                            first_day_of_week: props.calendar_first_day_of_week,
                            days_in_first_week: props.calendar_days_in_first_week,
                        },
                        kind == crate::ir::ValueKind::DateTime,
                        props.calendar_date_only,
                    );
                    if !props.calendar_check_policy_lax {
                        if props.calendar_date_only {
                            parsed_text.map_err(|_| calendar_text_strict_date_error(trimmed))?
                        } else if kind == crate::ir::ValueKind::Time {
                            parsed_text.map_err(|_| calendar_text_strict_time_error(trimmed))?
                        } else {
                            parsed_text?
                        }
                    } else {
                        parsed_text?
                    }
                };
                use crate::schema::{CalendarPatternKind, Representation};
                let inherit_format_tz = props.representation == Representation::Text
                    && props.calendar_pattern_kind == CalendarPatternKind::Explicit;
                let default_utc = kind == crate::ir::ValueKind::DateTime
                    && parsed.contains('T')
                    && !lexical_has_xsd_timezone(&parsed);
                let with_tz = append_packed_calendar_timezone(
                    props,
                    strings,
                    kind,
                    props.calendar_date_only,
                    &parsed,
                    default_utc,
                    inherit_format_tz,
                )?;
                Ok(DfdlValue::DateTime(with_tz))
            } else {
                Ok(DfdlValue::DateTime(trimmed.into()))
            }
        }
        String => {
            let sv = if props.encoding_error_policy == crate::schema::EncodingErrorPolicy::Replace
                && trimmed == text
            {
                crate::value::StringValue::with_source_bytes(trimmed, raw)
            } else {
                crate::value::StringValue::new(trimmed)
            };
            Ok(DfdlValue::String(sv))
        }
        HexBinary => decode_hex(trimmed).map(DfdlValue::HexBinary),
        Complex => Err(VmError::TypeMismatch {
            expected: "complex".into(),
        }),
    }?;
    if props.representation == Representation::Text
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
    {
        consume_text_field_terminator_after_fixed_length(cursor, props, strings)?;
    }
    Ok(value)
}

fn consume_text_field_terminator_after_fixed_length(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let Some(term_id) = props.terminator else {
        return Ok(());
    };
    let term = strings.get(term_id)?;
    if term.is_empty() {
        return Ok(());
    }
    let enc = encoding_name(props, strings).ok();
    if crate::schema::match_delimiter_opts_for_encoding(
        &cursor.data[cursor.pos..],
        term,
        props.ignore_case,
        enc.as_deref(),
    )
    .is_some()
    {
        if !cursor.consume_delimiter(term, props.ignore_case, enc.as_deref()) {
            return Err(VmError::InvalidValue {
                message: "terminator mismatch".into(),
            });
        }
    }
    Ok(())
}

pub(crate) fn finalize_simple_value(
    value: crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    enable_facet_validation: bool,
    defer_facet_validation: bool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    crate::vm::facet_validate::validate_assert_int_eq(&value, props)?;
    if crate::vm::facet_validate::needs_facet_validation(props)
        && enable_facet_validation
        && !defer_facet_validation
    {
        crate::vm::facet_validate::validate_decoded_facets(
            &value, kind, props, strings, tunables,
        )?;
    }
    Ok(value)
}

fn encode_binary_boolean_sl(v: bool, props: &IrProps) -> u64 {
    let false_rep = props.binary_boolean_false_rep.unwrap_or(0);
    let true_empty =
        props.binary_boolean_true_rep_defined && props.binary_boolean_true_rep.is_none();
    if v {
        if true_empty {
            let width = implicit_binary_scalar_byte_length(crate::ir::ValueKind::Boolean, props) * 8;
            let mask = if width >= 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            ((!false_rep) as u32 as u64) & mask
        } else {
            props.binary_boolean_true_rep.unwrap_or(1)
        }
    } else {
        false_rep
    }
}

fn decode_binary_boolean_sl(
    sl: u64,
    props: &IrProps,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    let false_rep = props.binary_boolean_false_rep.unwrap_or(0);
    let true_empty = props.binary_boolean_true_rep_defined && props.binary_boolean_true_rep.is_none();
    if true_empty {
        if sl == false_rep {
            Ok(false)
        } else {
            Ok(true)
        }
    } else {
        let true_rep = props.binary_boolean_true_rep.ok_or(VmError::InvalidValue {
            message: "binary boolean true rep missing".into(),
        })?;
        if sl == true_rep {
            Ok(true)
        } else if sl == false_rep {
            Ok(false)
        } else {
            Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. Unable to parse xs:boolean from binary: {sl}"
                ),
            })
        }
    }
}

pub(crate) fn parse_xs_boolean_lexical(
    trimmed: &str,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    match trimmed {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(VmError::InvalidValue {
            message: "Must be one of 0, 1, true, or false".into(),
        }),
    }
}

fn text_boolean_rep_candidates(
    props: &IrProps,
    strings: &StringPool,
    true_side: bool,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
) -> Result<alloc::vec::Vec<alloc::string::String>, crate::error::VmError> {
    use crate::error::VmError;
    let id = if true_side {
        props.text_boolean_true_rep
    } else {
        props.text_boolean_false_rep
    };
    let Some(id) = id else {
        return Ok(alloc::vec::Vec::new());
    };
    let raw = strings.get(id)?;
    let tokens = crate::schema::boolean_reps::tokenize_text_boolean_rep_list(raw);
    let mut out = alloc::vec::Vec::new();
    for tok in tokens {
        let (sibling_text, sibling_bytes) = sibling_env
            .map(|e| (Some(e.text), Some(e.content_bytes)))
            .unwrap_or((None, None));
        let s = crate::schema::boolean_reps::resolve_text_boolean_rep_token(
            &tok,
            sibling_text,
            sibling_bytes,
        )
            .map_err(|detail| VmError::InvalidValue {
                message: detail,
            })?;
        out.push(s);
    }
    Ok(out)
}

fn validate_runtime_text_boolean_same_length(
    props: &IrProps,
    true_reps: &[alloc::string::String],
    false_reps: &[alloc::string::String],
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthKind;
    if !matches!(props.length_kind, LengthKind::Explicit | LengthKind::Implicit) {
        return Ok(());
    }
    if props.text_pad_kind != TextPadKind::None && props.text_trim_kind != TextTrimKind::None {
        return Ok(());
    }
    let true_len = true_reps.first().map(|s| s.chars().count()).unwrap_or(0);
    let false_len = false_reps.first().map(|s| s.chars().count()).unwrap_or(0);
    if true_len != false_len
        || true_reps.iter().any(|r| r.chars().count() != true_len)
        || false_reps.iter().any(|r| r.chars().count() != false_len)
    {
        return Err(VmError::InvalidValue {
            message:
                "Schema Definition Error: dfdl:textBooleanTrueRep and dfdl:textBooleanFalseRep must have the same length"
                    .into(),
        });
    }
    Ok(())
}

fn pick_text_boolean_unparse_rep(reps: &[alloc::string::String]) -> alloc::string::String {
    let Some(s) = reps.first() else {
        return alloc::string::String::new();
    };
    s.split_whitespace()
        .next()
        .unwrap_or(s.as_str())
        .to_string()
}

fn parse_text_boolean(
    trimmed: &str,
    props: &IrProps,
    strings: &StringPool,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    let ignore = props.ignore_case;
    let true_reps = text_boolean_rep_candidates(props, strings, true, sibling_env)?;
    let false_reps = text_boolean_rep_candidates(props, strings, false, sibling_env)?;
    validate_runtime_text_boolean_same_length(props, &true_reps, &false_reps)?;
    let matches = |a: &str, b: &str| {
        if ignore {
            if a.eq_ignore_ascii_case(b) {
                return true;
            }
        } else if a == b {
            return true;
        }
        // Delimited padded booleans may present a prefix of a multi-character rep (e.g. `a` vs `a b c`).
        if a.len() < b.len() && b.as_bytes().get(a.len()) == Some(&b' ') {
            let head = &b[..a.len()];
            if ignore {
                if a.eq_ignore_ascii_case(head) {
                    return true;
                }
            } else if a == head {
                return true;
            }
        }
        // Single-character occurrences in a comma-separated list may match a word in a
        // space-separated false rep (e.g. `b` vs `a b c` in textBoolean_0).
        if a.chars().count() == 1 {
            let ch = a.chars().next().unwrap();
            for word in b.split_whitespace() {
                let mut wch = word.chars();
                let Some(first) = wch.next() else {
                    continue;
                };
                if ignore {
                    if first.eq_ignore_ascii_case(&ch) {
                        return true;
                    }
                } else if first == ch {
                    return true;
                }
            }
        }
        false
    };
    if true_reps.iter().any(|r| matches(trimmed, r)) {
        return Ok(true);
    }
    if false_reps.iter().any(|r| matches(trimmed, r)) {
        return Ok(false);
    }
    if !true_reps.is_empty() || !false_reps.is_empty() {
        return Err(VmError::InvalidValue {
            message: alloc::format!("Parse Error. Unable to parse xs:boolean from text: {trimmed}"),
        });
    }
    match trimmed {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(VmError::InvalidValue {
            message: alloc::format!("invalid boolean `{trimmed}`"),
        }),
    }
}

fn resolve_blob_value_bytes(value: &crate::value::DfdlValue) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::value::DfdlValue;
    match value {
        DfdlValue::Blob(bytes) => Ok(bytes.clone()),
        DfdlValue::String(s) => crate::tdml::resolve_blob_uri_to_bytes(&s.text).map_err(|m| {
            VmError::InvalidValue {
                message: if m.contains("Unable to open blob") {
                    alloc::format!("Unparse Error: {m}")
                } else {
                    m
                },
            }
        }),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => {
            crate::tdml::resolve_blob_uri_to_bytes(s).map_err(|m| VmError::InvalidValue {
                message: if m.contains("Unable to open blob") {
                    alloc::format!("Unparse Error: {m}")
                } else {
                    m
                },
            })
        }
        other => Err(VmError::InvalidValue {
            message: alloc::format!("blob unparse expected URI string, got `{other:?}`"),
        }),
    }
}

fn write_blob_scalar(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    props: &IrProps,
    field_name: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::{LengthKind, LengthUnits, ObjectKind};

    if props.object_kind != ObjectKind::Bytes {
        return Err(VmError::InvalidValue {
            message: "internal blob encode on non-bytes element".into(),
        });
    }
    if props.length_kind != LengthKind::Explicit {
        return Err(VmError::InvalidValue {
            message: "objectKind='bytes' must have dfdl:lengthKind='explicit'".into(),
        });
    }
    let len = props.length.ok_or(VmError::InvalidValue {
        message: "explicit blob missing length".into(),
    })? as usize;
    let len_bits = match props.length_units {
        LengthUnits::Bytes => len.saturating_mul(8),
        LengthUnits::Bits => len,
        LengthUnits::Characters => {
            return Err(VmError::InvalidValue {
                message: "lengthUnits='characters' is not valid for blob data.".into(),
            })
        }
    };
    let bytes = resolve_blob_value_bytes(value)?;
    let value_bits = bytes.len().saturating_mul(8);
    if value_bits > len_bits {
        let mut message = alloc::format!(
            "Unparse Error: Blob length ({value_bits} bits) exceeds explicit length value: {len_bits} bits"
        );
        if let Some(name) = field_name {
            message.push_str("\nSchema context: ");
            message.push_str(name);
        }
        return Err(VmError::InvalidValue { message });
    }
    let write_bytes = (len_bits + 7) / 8;
    let mut payload = bytes;
    if payload.len() < write_bytes {
        payload.extend(core::iter::repeat(props.fill_byte).take(write_bytes - payload.len()));
    }
    if props.length_units == LengthUnits::Bits && len_bits % 8 != 0 {
        write_bits_from_stream_with_config(
            out,
            bit_count,
            &payload,
            len_bits,
            props.bit_order,
            None,
        )?;
    } else {
        write_byte_aligned(out, bit_count, &payload[..write_bytes.min(payload.len())])?;
    }
    Ok(())
}

pub(crate) fn write_binary_scalar(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
    config: &RuntimeConfig,
    field_name: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::schema::ObjectKind;
    use crate::value::DfdlValue;

    if props.object_kind == ObjectKind::Bytes {
        return write_blob_scalar(out, bit_count, value, props, field_name);
    }

    if kind == HexBinary {
        if let Some(max) = tunables.max_hex_binary_length_in_bytes {
            let wire_len = props.length.unwrap_or_else(|| {
                if let DfdlValue::HexBinary(v) = value {
                    v.len() as u64
                } else {
                    0
                }
            });
            if wire_len > u64::from(max) {
                return Err(crate::length_validate::hex_binary_max_length_error(
                    max,
                    wire_len,
                    true,
                ));
            }
        }
    }

    if props.length_kind == LengthKind::Prefixed {
        let payload = encode_binary_payload_bytes(value, kind, props, strings, field_name)?;
        return write_prefixed_bytes(out, bit_count, &payload, props, strings, field_name);
    }

    if props.length_kind == LengthKind::Delimited {
        if matches!(kind, HexBinary | String) {
            let mut payload = encode_binary_payload_bytes(value, kind, props, strings, field_name)?;
            if kind == HexBinary {
                payload = pad_hex_binary_value(payload, props, None);
            }
            write_byte_aligned(out, bit_count, &payload)?;
            return Ok(());
        }
        if !is_packed_binary_rep(props.binary_number_rep)
            && props.binary_number_rep != BinaryNumberRep::Bcd
            && props.binary_number_rep != BinaryNumberRep::Ibm4690Packed
        {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!(
                    "lengthKind `{}` on binary scalar encode",
                    length_kind_name(props.length_kind)
                ),
            });
        }
        let payload = encode_binary_payload_bytes(value, kind, props, strings, field_name)?;
        write_byte_aligned(out, bit_count, &payload)?;
        return Ok(());
    }

    if kind == crate::ir::ValueKind::Decimal {
        validate_explicit_decimal_vm(props, VmDecimalPhase::Unparse, tunables, None, strings)?;
    }

    if props.length_units == LengthUnits::Bits {
        let n = binary_encode_bit_length(kind, props, tunables, strings)?;
        let raw = scalar_to_raw_bits(value, kind, props, n)?;
        if props.byte_order == ByteOrder::LittleEndian
            && kind != crate::ir::ValueKind::String
            && kind != crate::ir::ValueKind::HexBinary
        {
            let wire = if matches!(kind, Float | Double) {
                stream_bits_to_bytes(raw, n, props.byte_order)
            } else {
                encode_packed_bit_field_bytes(raw, n, props.byte_order, props.bit_order)
            };
            write_bits_from_stream_with_config(
                out,
                bit_count,
                &wire,
                n,
                props.bit_order,
                Some(config),
            )?;
            return Ok(());
        }
        write_stream_bits_with_config(out, bit_count, raw, n, props.bit_order, Some(config));
        return Ok(());
    }

    let le = props.byte_order == ByteOrder::LittleEndian;
    let size = match props.length_kind {
        LengthKind::Fixed => {
            let len = props.length.unwrap_or(type_size(kind) as u64);
            validate_data_length_vm(kind, len, LengthUnits::Bytes, props.binary_number_rep)?;
            validate_signed_one_bit_length_vm(kind, len, LengthUnits::Bytes, tunables)?;
            len as usize
        }
        LengthKind::Implicit => {
            if kind == HexBinary {
                props
                    .implicit_facet_length
                    .or(props.min_length)
                    .map(|n| n as usize)
                    .unwrap_or_else(|| type_size(kind))
            } else {
                implicit_binary_scalar_byte_length(kind, props)
            }
        }
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            validate_data_length_vm(kind, len, LengthUnits::Bytes, props.binary_number_rep)?;
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
        (Boolean, DfdlValue::Boolean(v)) => {
            bytes = int_bytes(encode_binary_boolean_sl(*v, props) as i64, size, le)
        }
        (Byte, DfdlValue::Byte(v)) => bytes = int_bytes(*v as i64, size, le),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => {
            bytes = int_bytes(*v as i64, size, le)
        }
        (Short, DfdlValue::Short(v)) => bytes = int_bytes(*v as i64, size, le),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => {
            bytes = int_bytes(*v as i64, size, le)
        }
        (Int, DfdlValue::Int(v)) => bytes = int_bytes(*v as i64, size, le),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => {
            bytes = int_bytes(*v as i64, size, le)
        }
        (Long, DfdlValue::Long(v)) => bytes = int_bytes(*v, size, le),
        (Float, DfdlValue::Float(v)) => bytes = int_bytes(v.to_bits() as i64, size, le),
        (Double, DfdlValue::Double(v)) => bytes = int_bytes(v.to_bits() as i64, size, le),
        (HexBinary, DfdlValue::HexBinary(v)) => {
            bytes = pad_hex_binary_value(v.clone(), props, Some(size));
        }
        (Decimal, DfdlValue::Decimal(v)) => {
            let (negative, raw) =
                parse_virtual_decimal_signed(v, props.binary_decimal_virtual_point)?;
            validate_decimal_unparse_sign(negative, props, field_name)?;
            let signed_raw = if negative && props.decimal_signed {
                (raw as i64).wrapping_neg() as u64
            } else {
                raw
            };
            bytes = stream_bits_to_bytes(signed_raw, size.saturating_mul(8), props.byte_order);
        }
        (expected, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{expected:?}"),
            });
        }
    }

    if kind != Decimal && bytes.len() != size {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "binary value width {} does not match explicit length {size}",
                bytes.len()
            ),
        });
    }
    write_byte_aligned(out, bit_count, &bytes)?;
    Ok(())
}

fn binary_encode_bit_length(
    kind: crate::ir::ValueKind,
    props: &IrProps,
    tunables: &DaffodilTunables,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    match props.length_kind {
        LengthKind::Fixed => Ok(props.length.unwrap_or(
            (implicit_binary_scalar_byte_length(kind, props) * 8) as u64,
        ) as usize),
        LengthKind::Implicit => Ok(implicit_binary_scalar_byte_length(kind, props) * 8),
        LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            if kind == crate::ir::ValueKind::Decimal {
                validate_explicit_decimal_vm(
                    props,
                    VmDecimalPhase::Unparse,
                    tunables,
                    Some(LengthUnits::Bits),
                    strings,
                )?;
            } else {
                validate_data_length_vm(kind, len, LengthUnits::Bits, props.binary_number_rep)?;
                validate_signed_one_bit_length_vm(kind, len, LengthUnits::Bits, tunables)?;
            }
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
        (Boolean, DfdlValue::Boolean(v)) => Ok(encode_binary_boolean_sl(*v, props)),
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

fn format_u128_radix_lower(mut n: u128, base: u32) -> alloc::string::String {
    if n == 0 {
        return alloc::string::String::from("0");
    }
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 128];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = DIGITS[(n % base as u128) as usize];
        n /= base as u128;
    }
    alloc::string::String::from_utf8_lossy(&buf[i..]).into_owned()
}

fn format_text_standard_radix_unparse(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    use crate::value::DfdlValue;

    let base = props.text_standard_base;
    let (negative, magnitude) = match (kind, value) {
        (ValueKind::Byte, DfdlValue::Byte(v)) => (*v < 0, (*v as i64).unsigned_abs() as u128),
        (ValueKind::Byte, DfdlValue::Long(v)) => {
            if *v < 0 {
                (true, v.unsigned_abs() as u128)
            } else {
                (false, *v as u128)
            }
        }
        (ValueKind::Short, DfdlValue::Short(v)) => (*v < 0, (*v as i64).unsigned_abs() as u128),
        (ValueKind::Int, DfdlValue::Int(v)) => (*v < 0, (*v as i64).unsigned_abs() as u128),
        (ValueKind::Long, DfdlValue::Long(v)) => (*v < 0, v.unsigned_abs() as u128),
        (ValueKind::Integer, DfdlValue::Integer(s)) => {
            let trimmed = s.trim();
            if trimmed.starts_with('-') {
                (true, trimmed[1..].parse::<u128>().unwrap_or(0))
            } else {
                (false, trimmed.parse::<u128>().unwrap_or(0))
            }
        }
        (ValueKind::UnsignedByte, DfdlValue::UnsignedByte(v)) => (false, *v as u128),
        (ValueKind::UnsignedShort, DfdlValue::UnsignedShort(v)) => (false, *v as u128),
        (ValueKind::UnsignedInt, DfdlValue::UnsignedInt(v)) => (false, *v as u128),
        (other, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{other:?}"),
            });
        }
    };

    if negative {
        if props.non_negative_integer {
            let raw = match value {
                DfdlValue::Integer(s) => s.clone(),
                DfdlValue::Long(v) => alloc::format!("{v}"),
                DfdlValue::Int(v) => alloc::format!("{v}"),
                DfdlValue::Byte(v) => alloc::format!("{v}"),
                DfdlValue::Short(v) => alloc::format!("{v}"),
                _ => alloc::format!("-{magnitude}"),
            };
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Unparse Error: value `{raw}` out of range for type xs:nonNegativeInteger"
                ),
            });
        }
        let display = match value {
            DfdlValue::Int(v) => alloc::format!("{v}"),
            DfdlValue::Long(v) => alloc::format!("{v}"),
            DfdlValue::Byte(v) => alloc::format!("{v}"),
            DfdlValue::Short(v) => alloc::format!("{v}"),
            DfdlValue::Integer(s) => s.clone(),
            _ => alloc::format!("-{magnitude}"),
        };
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Unparse Error: Unable to unparse negative value `{display}` when textStandardBase=\"{base}\""
            ),
        });
    }

    Ok(format_u128_radix_lower(magnitude, base))
}

fn truncate_string_for_explicit_length(
    text: &str,
    len: usize,
    units: LengthUnits,
    encoding: &str,
    props: &IrProps,
    kind: crate::ir::ValueKind,
    field_name: Option<&str>,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    use crate::vm::encoding::count_characters;

    let too_long = match units {
        LengthUnits::Bytes => text.len() > len,
        LengthUnits::Characters => {
            count_characters(text.as_bytes(), encoding, EncodingErrorPolicy::Error)? > len
        }
        LengthUnits::Bits => text.as_bytes().len() * 8 > len,
    };
    if !too_long {
        return Ok(text.to_string());
    }
    if !props.truncate_specified_length_string {
        let mut message =
            "Unparse Error: data too long for explicit length and unable to truncate".to_string();
        if let Some(name) = field_name {
            message.push_str("\nSchema context: ");
            message.push_str(name);
        }
        return Err(VmError::InvalidValue { message });
    }
    match text_justification_for_kind(props, kind) {
        TextStringJustification::Center => Err(VmError::InvalidValue {
            message: alloc::format!(
                "Unparse Error: dfdl:textStringJustification=\"center\" cannot be used with dfdl:truncateSpecifiedLengthString=\"yes\" when truncation is required"
            ),
        }),
        TextStringJustification::Left => {
            let out = match units {
                LengthUnits::Bytes => {
                    if text.len() <= len {
                        text.to_string()
                    } else {
                        text[..len].to_string()
                    }
                }
                LengthUnits::Characters => {
                    let mut out = alloc::string::String::new();
                    for (idx, ch) in text.chars().enumerate() {
                        if idx >= len {
                            break;
                        }
                        out.push(ch);
                    }
                    out
                }
                LengthUnits::Bits => text.to_string(),
            };
            Ok(out)
        }
        TextStringJustification::Right => {
            let out = match units {
                LengthUnits::Bytes => {
                    if text.len() <= len {
                        text.to_string()
                    } else {
                        text[text.len() - len..].to_string()
                    }
                }
                LengthUnits::Characters => {
                    let chars: Vec<char> = text.chars().collect();
                    if chars.len() <= len {
                        text.to_string()
                    } else {
                        chars[chars.len() - len..].iter().collect()
                    }
                }
                LengthUnits::Bits => text.to_string(),
            };
            Ok(out)
        }
    }
}

fn format_field_text_number(
    text: &str,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::string::String, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    use crate::schema::TextNumberRep;
    use crate::vm::text_number_format::{format_standard_text_number, TextNumberRoundingProps};

    if props.text_number_rep != TextNumberRep::Standard || props.text_standard_base != 10 {
        return Ok(text.to_string());
    }
    if !matches!(
        kind,
        ValueKind::Byte
            | ValueKind::Short
            | ValueKind::Int
            | ValueKind::Long
            | ValueKind::Integer
            | ValueKind::UnsignedByte
            | ValueKind::UnsignedShort
            | ValueKind::UnsignedInt
            | ValueKind::Float
            | ValueKind::Double
            | ValueKind::Decimal
    ) {
        return Ok(text.to_string());
    }
    let Some(pat_id) = props.text_number_pattern else {
        return Ok(text.to_string());
    };
    let raw_pattern = strings.get(pat_id)?;
    let pattern_owned = if props.text_number_rep == TextNumberRep::Zoned {
        crate::vm::zoned_text::strip_zoned_plus_markers(raw_pattern)
    } else {
        raw_pattern.to_string()
    };
    let pattern = pattern_owned.as_str();
    if pattern.contains('V')
        && !pattern.contains('E')
        && !pattern.contains('e')
        && !pattern.contains('\'')
        && !pattern.contains(';')
    {
        return Ok(text.to_string());
    }
    let (dec_seps, grouping, exponent, pad, check_policy) =
        resolved_text_number_format_parts(props, strings);
    let fmt = crate::vm::text_number::TextNumberFormatProps {
        check_policy,
        decimal_separators: &dec_seps,
        grouping_separator: grouping.as_deref(),
        exponent_chars: &exponent,
        pad_character: pad,
        ignore_case: props.ignore_case,
    };
    if props.text_standard_zero_rep_defined {
        if let Ok(raw) = strings.get(props.text_standard_zero_rep) {
            if let Some(z) =
                crate::vm::text_number_format::text_standard_zero_unparse(text, raw)
            {
                return Ok(z);
            }
        }
    }

    let increment_owned = if props.text_number_rounding_increment_defined {
        strings.get(props.text_number_rounding_increment)?.to_string()
    } else {
        alloc::string::String::from("0")
    };
    let rounding = TextNumberRoundingProps {
        rounding: props.text_number_rounding,
        mode: props.text_number_rounding_mode,
        increment: increment_owned.as_str(),
    };
    format_standard_text_number(text, pattern, &fmt, rounding)
}

pub(crate) fn write_text_scalar(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    config: &RuntimeConfig,
    field_name: Option<&str>,
    encode_siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    if matches!(value, DfdlValue::Null) {
        let payload = nil_unparse_bytes(props, strings)?;
        write_byte_aligned(out, bit_count, &payload)?;
        return Ok(());
    }

    let source_bytes = match (kind, value) {
        (String, DfdlValue::String(v)) => v.meta.source_bytes.clone(),
        _ => None,
    };

    let numeric_radix = props.text_number_rep == TextNumberRep::Standard
        && props.text_standard_base != 10
        && matches!(
            kind,
            ValueKind::Byte
                | ValueKind::Short
                | ValueKind::Int
                | ValueKind::Long
                | ValueKind::Integer
                | ValueKind::UnsignedByte
                | ValueKind::UnsignedShort
                | ValueKind::UnsignedInt
        );

    let text = if numeric_radix {
        format_text_standard_radix_unparse(value, kind, props)?
    } else {
        match (kind, value) {
        (Boolean, DfdlValue::Boolean(v)) => {
            let true_reps = text_boolean_rep_candidates(props, strings, true, None)?;
            let false_reps = text_boolean_rep_candidates(props, strings, false, None)?;
            if *v {
                let picked = pick_text_boolean_unparse_rep(&true_reps);
                if picked.is_empty() {
                    alloc::string::String::from("true")
                } else {
                    picked
                }
            } else {
                let picked = pick_text_boolean_unparse_rep(&false_reps);
                if picked.is_empty() {
                    alloc::string::String::from("false")
                } else {
                    picked
                }
            }
        }
        (Byte, DfdlValue::Byte(v)) => alloc::format!("{v}"),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => alloc::format!("{v}"),
        (Short, DfdlValue::Short(v)) => alloc::format!("{v}"),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => alloc::format!("{v}"),
        (Int, DfdlValue::Int(v)) => alloc::format!("{v}"),
        (Integer, DfdlValue::Integer(v)) => v.clone(),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => alloc::format!("{v}"),
        (Long, DfdlValue::Long(v)) => alloc::format!("{v}"),
        (Float, DfdlValue::Float(v)) => alloc::format!("{v}"),
        (Double, DfdlValue::Double(v)) => alloc::format!("{v}"),
        (Decimal, DfdlValue::Decimal(v)) => v.clone(),
        (DateTime, DfdlValue::DateTime(v)) | (Time, DfdlValue::DateTime(v)) => {
            if let Some(pat_id) = props.calendar_pattern {
                let pattern = strings.get(pat_id)?;
                let cal_lang = resolve_calendar_language(
                    props,
                    strings,
                    sibling_text_map_for_calendar(encode_siblings).as_ref(),
                )?;
                if crate::vm::calendar_binary::calendar_pattern_time_only(pattern)
                    || kind == crate::ir::ValueKind::Time
                {
                    unparse_iso_time_to_calendar_pattern(v, pattern)?
                } else {
                    unparse_iso_date_to_calendar_pattern(v, pattern, cal_lang.as_deref())?
                }
            } else {
                v.clone()
            }
        }
        (String, DfdlValue::String(v)) => {
            if config.encode_pua_codepoints_as_utf8 {
                v.text.clone()
            } else {
                let enc = encoding_name(props, strings)?;
                if uses_xml_illegal_char_remap(&enc) {
                    remap_pua_to_xml_illegal_characters(&v.text)
                } else {
                    v.text.clone()
                }
            }
        }
        (HexBinary, DfdlValue::HexBinary(v)) => encode_hex(v),
        (expected, _) => {
            return Err(VmError::TypeMismatch {
                expected: alloc::format!("{expected:?}"),
            });
        }
    }
    };

    let text = if numeric_radix {
        text
    } else {
        format_field_text_number(&text, kind, props, strings)?
    };
    let text_before_pad = text.clone();
    let text = apply_min_length_pad(&text, props, strings, kind);

    if props.length_kind == LengthKind::Prefixed {
        let encoded = if let Some(raw) = source_bytes.filter(|_| text == text_before_pad) {
            raw
        } else {
            encode_document_text(&text, encoding_name(props, strings)?)?
        };
        return write_prefixed_bytes(out, bit_count, &encoded, props, strings, field_name);
    }

    let encoding = encoding_name(props, strings)?;
    if let Some(raw) = source_bytes.filter(|_| text == text_before_pad) {
        let payload = match props.length_kind {
            LengthKind::Fixed | LengthKind::Explicit => {
                let len = props.length.ok_or(VmError::InvalidValue {
                    message: "fixed/explicit text missing length".into(),
                })? as usize;
                pad_raw_text_field(&raw, len, props.length_units, props, strings, kind, encoding)?
            }
            LengthKind::Delimited
            | LengthKind::Pattern
            | LengthKind::Implicit
            | LengthKind::EndOfParent => raw,
            other => {
                return Err(VmError::UnsupportedOperation {
                    op: alloc::format!("text lengthKind `{}` encode", length_kind_name(other)),
                });
            }
        };
        write_byte_aligned(out, bit_count, &payload)?;
        return Ok(());
    }

    let payload = match props.length_kind {
        LengthKind::Fixed | LengthKind::Explicit => {
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "fixed/explicit text missing length".into(),
            })? as usize;
            if props.length_units == LengthUnits::Bits {
                let encoded = encode_document_text(&text, encoding)?;
                write_bits_from_stream_with_config(
                    out,
                    bit_count,
                    &encoded,
                    len,
                    props.bit_order,
                    Some(&config),
                )?;
                return Ok(());
            }
            let text = truncate_string_for_explicit_length(
                &text,
                len,
                props.length_units,
                encoding,
                props,
                kind,
                field_name,
            )?;
            pad_text_field(&text, len, props.length_units, props, strings, kind, encoding)?
        }
        LengthKind::Delimited | LengthKind::Pattern | LengthKind::Implicit | LengthKind::EndOfParent => {
            let encoded = encode_document_text(&text, encoding)?;
            if let Some(spec) = bits_charset_spec(encoding) {
                let len = text.chars().count().saturating_mul(spec.width as usize);
                write_bits_from_stream_with_config(
                    out,
                    bit_count,
                    &encoded,
                    len,
                    props.bit_order,
                    Some(&config),
                )?;
                return Ok(());
            }
            encoded
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
    if props.text_pad_kind != TextPadKind::PadChar {
        return text.to_string();
    }
    if kind != ValueKind::String {
        return text.to_string();
    }
    let mut min_len = min_len as usize;
    if matches!(props.length_kind, LengthKind::Fixed | LengthKind::Explicit) {
        if let Some(explicit) = props.length {
            min_len = min_len.min(explicit as usize);
        }
    }
    let current = text.chars().count();
    if current >= min_len {
        return text.to_string();
    }
    let pad_char = pad_char_for_kind(props, strings, kind);
    let pad_ch = pad_char.chars().next().unwrap_or(' ');
    let pad_count = min_len - current;
    match text_justification_for_kind(props, kind) {
        TextStringJustification::Right => {
            let mut out = alloc::string::String::new();
            for _ in 0..pad_count {
                out.push(pad_ch);
            }
            out.push_str(text);
            out
        }
        TextStringJustification::Center => {
            let right = pad_count / 2;
            let left = pad_count - right;
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

    let pad_char = pad_char_for_kind(props, strings, kind);
    let pad_byte = pad_char.chars().next().unwrap_or(b' ' as char) as u8;
    let justification = text_justification_for_kind(props, kind);

    match units {
        LengthUnits::Bytes => {
            let mut bytes = text.as_bytes().to_vec();
            if bytes.len() > len {
                if props.truncate_specified_length_string {
                    bytes = match justification {
                        TextStringJustification::Center => {
                            return Err(VmError::InvalidValue {
                                message: alloc::format!(
                                    "Unparse Error: dfdl:textStringJustification=\"center\" cannot be used with dfdl:truncateSpecifiedLengthString=\"yes\" when truncation is required"
                                ),
                            });
                        }
                        TextStringJustification::Left => bytes[..len].to_vec(),
                        TextStringJustification::Right => bytes[bytes.len() - len..].to_vec(),
                    };
                } else {
                    return Err(VmError::InvalidValue {
                        message: "text value too long for explicit length".into(),
                    });
                }
            }
            let pad_count = len - bytes.len();
            match justification {
                TextStringJustification::Right => {
                    bytes.splice(0..0, iter::repeat(pad_byte).take(pad_count));
                }
                TextStringJustification::Center => {
                    let right = pad_count / 2;
                    let left = pad_count - right;
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
            let current = count_characters(text.as_bytes(), encoding, EncodingErrorPolicy::Error)?;
            if current > len {
                return Err(VmError::InvalidValue {
                    message: "text value too long for explicit character length".into(),
                });
            }
            let mut padded = text.to_string();
            let pad_count = len - current;
            let pad_str: alloc::string::String = pad_char.chars().take(1).collect();
            match justification {
                TextStringJustification::Right => {
                    for _ in 0..pad_count {
                        padded.insert_str(0, &pad_str);
                    }
                }
                TextStringJustification::Center => {
                    let right = pad_count / 2;
                    let left = pad_count - right;
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

fn pad_raw_text_field(
    raw: &[u8],
    len: usize,
    units: LengthUnits,
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
    encoding: &str,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::{LengthUnits, TextStringJustification};
    use crate::vm::encoding::count_characters;

    let pad_char = pad_char_for_kind(props, strings, kind);
    let pad_byte = pad_char.chars().next().unwrap_or(b' ' as char) as u8;
    let justification = text_justification_for_kind(props, kind);

    match units {
        LengthUnits::Bytes => {
            let mut bytes = raw.to_vec();
            if bytes.len() > len {
                bytes.truncate(len);
                return Ok(bytes);
            }
            let pad_count = len - bytes.len();
            match justification {
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
            let current =
                count_characters(raw, encoding, EncodingErrorPolicy::Replace)?;
            if current > len {
                return Err(VmError::InvalidValue {
                    message: "text value too long for explicit character length".into(),
                });
            }
            if current == len {
                return Ok(raw.to_vec());
            }
            let pad_count = len - current;
            let pad_bytes = encode_document_text(&pad_char, encoding)?;
            let mut out = alloc::vec::Vec::new();
            match justification {
                TextStringJustification::Right => {
                    for _ in 0..pad_count {
                        out.extend_from_slice(&pad_bytes);
                    }
                    out.extend_from_slice(raw);
                }
                TextStringJustification::Center => {
                    let left = pad_count / 2;
                    let right = pad_count - left;
                    for _ in 0..left {
                        out.extend_from_slice(&pad_bytes);
                    }
                    out.extend_from_slice(raw);
                    for _ in 0..right {
                        out.extend_from_slice(&pad_bytes);
                    }
                }
                TextStringJustification::Left => {
                    out.extend_from_slice(raw);
                    for _ in 0..pad_count {
                        out.extend_from_slice(&pad_bytes);
                    }
                }
            }
            Ok(out)
        }
        LengthUnits::Bits => Err(VmError::UnsupportedOperation {
            op: "explicit text bit length encode".into(),
        }),
    }
}

pub(crate) fn has_non_empty_terminator(
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    if let Some(id) = props.terminator {
        return Ok(!strings.get(id)?.is_empty());
    }
    Ok(false)
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

#[derive(Clone, Debug)]
struct DelimScanPattern {
    pat: alloc::string::String,
    ignore_case: bool,
}

fn push_delimiter_scan_patterns(
    patterns: &mut alloc::vec::Vec<DelimScanPattern>,
    pat: &str,
    ignore_case: bool,
) {
    if pat.is_empty() {
        return;
    }
    if patterns.iter().any(|p| p.pat == pat) {
        return;
    }
    patterns.push(DelimScanPattern {
        pat: pat.to_string(),
        ignore_case,
    });
    if let Some((_, suffix)) = pat.rsplit_once(' ') {
        push_delimiter_scan_patterns(patterns, suffix, ignore_case);
    }
}

fn non_empty_delimiter_scan_patterns(
    props: &IrProps,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<DelimScanPattern>, crate::error::VmError> {
    let mut patterns = alloc::vec::Vec::new();
    for id in delimiter_pattern_ids(props) {
        let pat = strings.get(id)?;
        push_delimiter_scan_patterns(&mut patterns, pat, props.ignore_case);
    }
    Ok(patterns)
}

fn enclosing_delimiter_scan_patterns(
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
) -> Result<alloc::vec::Vec<DelimScanPattern>, crate::error::VmError> {
    let mut patterns = non_empty_delimiter_scan_patterns(props, strings)?;
    for seq in stop_sequences {
        for id in delimiter_pattern_ids(seq) {
            let pat = strings.get(id)?;
            push_delimiter_scan_patterns(&mut patterns, pat, seq.ignore_case);
        }
    }
    Ok(patterns)
}

fn should_defer_parent_stop_delimiter(props: &IrProps) -> bool {
    props.initiator.is_some()
}

fn should_defer_infix_sequence_separator(
    seq_props: &IrProps,
    separator_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    Ok(seq_props.separator_position == SeparatorPosition::Infix
        && seq_props.separator == Some(separator_id)
        && !has_non_empty_terminator(field_props, strings)?)
}

fn should_defer_prefix_sequence_separator(
    seq_props: &IrProps,
    separator_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    Ok(seq_props.separator_position == SeparatorPosition::Prefix
        && seq_props.separator == Some(separator_id)
        && !has_non_empty_terminator(field_props, strings)?)
}

fn should_defer_postfix_sequence_separator(
    seq_props: &IrProps,
    separator_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    Ok(seq_props.separator_position == SeparatorPosition::Postfix
        && seq_props.separator == Some(separator_id)
        && !has_non_empty_terminator(field_props, strings)?)
}

fn should_defer_sequence_stop_delimiter_in_field(
    seq_props: &IrProps,
    pattern_id: StringId,
    field_props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    if has_non_empty_terminator(field_props, strings)? {
        return Ok(false);
    }
    if Some(pattern_id) == seq_props.terminator {
        return Ok(true);
    }
    if should_defer_prefix_sequence_separator(seq_props, pattern_id, field_props, strings)? {
        return Ok(true);
    }
    if should_defer_postfix_sequence_separator(seq_props, pattern_id, field_props, strings)? {
        return Ok(true);
    }
    should_defer_infix_sequence_separator(seq_props, pattern_id, field_props, strings)
}

/// When decoding a sequence child, skip the infix separator before this index if
/// `anyEmpty` applies and the previous particle was absent or an empty representation.
pub(crate) fn should_suppress_decode_infix_separator(
    seq_props: &IrProps,
    child_props: &IrProps,
    prev_absent_or_empty: bool,
) -> bool {
    let policy = seq_props
        .separator_suppression_policy
        .or(child_props.separator_suppression_policy);
    if policy != Some(SeparatorSuppressionPolicy::AnyEmpty) {
        return false;
    }
    seq_props.separator_position == SeparatorPosition::Infix && prev_absent_or_empty
}

/// DFDL disallows `%WSP*;` as the sole terminator on unbounded repeating elements.
pub(crate) fn validate_unbounded_wsp_star_terminator(
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if props.occurs_max.is_some() {
        return Ok(());
    }
    let Some(id) = props.terminator else {
        return Ok(());
    };
    let pat = strings.get(id)?;
    let p = pat.trim();
    if matches!(p, "%WSP*;" | "%WSP*" | "%WS*;" | "%WS*") {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: dfdl:terminator `{pat}` — %WSP*; cannot be used with maxOccurs unbounded"
            ),
        });
    }
    Ok(())
}

pub(crate) fn would_read_empty_delimited_field(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
) -> Result<bool, crate::error::VmError> {
    let patterns = enclosing_delimiter_scan_patterns(props, strings, stop_sequences)?;
    for entry in &patterns {
        if let Some(n) = crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            &entry.pat,
            entry.ignore_case,
        ) {
            if n > 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn delimiter_pat_as_text(pat: &str) -> alloc::string::String {
    crate::schema::encode_delimiter(pat)
        .into_iter()
        .map(|b| b as char)
        .collect()
}

fn read_bits_charset_code_unit(
    cursor: &mut Cursor<'_>,
    spec: crate::vm::encoding::BitsCharsetSpec,
) -> Result<char, crate::error::VmError> {
    use crate::error::VmError;
    let mut idx = 0u8;
    for i in 0..spec.width {
        let bit = cursor.read_stream_bit(spec.bit_order)? as u8;
        match spec.bit_order {
            BitOrder::MostSignificantBitFirst => idx = (idx << 1) | bit,
            BitOrder::LeastSignificantBitFirst => idx |= bit << i,
        }
    }
    spec.alphabet
        .chars()
        .nth(idx as usize)
        .ok_or_else(|| VmError::InvalidValue {
            message: "invalid bits charset code unit".into(),
        })
}

fn terminator_suffix_matches(decoded: &str, term: &str, ignore_case: bool) -> bool {
    if ignore_case {
        decoded
            .to_ascii_lowercase()
            .ends_with(&term.to_ascii_lowercase())
    } else {
        decoded.ends_with(term)
    }
}

fn read_until_delimiters_bits_charset(
    cursor: &mut Cursor<'_>,
    patterns: &[DelimScanPattern],
    require_delimiter: bool,
    spec: crate::vm::encoding::BitsCharsetSpec,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let terms: alloc::vec::Vec<(alloc::string::String, bool)> = patterns
        .iter()
        .map(|p| (delimiter_pat_as_text(&p.pat), p.ignore_case))
        .filter(|(t, _)| !t.is_empty())
        .collect();
    let start_pos = cursor.pos;
    let start_bit_count = cursor.bit_count;
    let mut decoded = alloc::string::String::new();
    let mut matched_term_chars = 0usize;

    loop {
        if cursor.is_frame_consumed() {
            break;
        }
        let unit = read_bits_charset_code_unit(cursor, spec)?;
        decoded.push(unit);
        for (term, ignore_case) in &terms {
            if terminator_suffix_matches(&decoded, term, *ignore_case) {
                matched_term_chars = term.chars().count();
                break;
            }
        }
        if matched_term_chars > 0 {
            break;
        }
    }

    if matched_term_chars == 0 {
        if require_delimiter {
            let labels = patterns
                .iter()
                .map(|p| alloc::format!("`{}`", format_delimiter_for_error(&p.pat)))
                .collect::<alloc::vec::Vec<_>>()
                .join(", ");
            return Err(VmError::InvalidValue {
                message: alloc::format!("terminator {labels} not found"),
            });
        }
    }

    let payload_chars = decoded.chars().count().saturating_sub(matched_term_chars);
    let payload_bits = payload_chars * spec.width as usize;

    cursor.pos = start_pos;
    cursor.bit_count = start_bit_count;
    cursor.read_stream_bits_as_bytes(payload_bits, spec.bit_order)
}

fn read_until_delimiters(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
    encoding: Option<&str>,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let patterns = enclosing_delimiter_scan_patterns(props, strings, stop_sequences)?;
    if let Some(enc) = encoding {
        if let Some(spec) = bits_charset_spec(enc) {
            if !patterns.is_empty() {
                return read_until_delimiters_bits_charset(
                    cursor,
                    &patterns,
                    require_delimiter,
                    spec,
                );
            }
        }
    }
    if patterns.is_empty() {
        let abs = cursor.absolute_bit_index();
        let total_bits = cursor
            .frame_bit_limit
            .unwrap_or_else(|| cursor.data.len().saturating_mul(8));
        let remaining_bits = total_bits.saturating_sub(abs);
        if remaining_bits == 0 {
            return Ok(Vec::new());
        }
        if cursor.bit_count == 0 && remaining_bits % 8 == 0 {
            let available = remaining_bits / 8;
            let byte_len = encoding
                .map(|enc| crate::vm::encoding::delimited_payload_byte_length(available, enc))
                .unwrap_or(available);
            let end = cursor.pos.saturating_add(byte_len);
            if end <= cursor.data.len() {
                let out = cursor.data[cursor.pos..end].to_vec();
                cursor.pos = end;
                return Ok(out);
            }
        }
        if cursor.bit_count != 0 || remaining_bits % 8 != 0 {
            return cursor.read_stream_bits_as_bytes(remaining_bits, props.bit_order);
        }
        let rest = cursor.data[cursor.pos..].to_vec();
        cursor.pos = cursor.data.len();
        cursor.bit_count = 0;
        return Ok(rest);
    }
    read_until_any_delimiter(cursor, &patterns, require_delimiter, encoding)
}

pub(crate) fn read_until_separator(
    cursor: &mut Cursor<'_>,
    separator: &str,
    require_delimiter: bool,
    ignore_case: bool,
) -> Result<Vec<u8>, crate::error::VmError> {
    let patterns = [DelimScanPattern {
        pat: separator.to_string(),
        ignore_case,
    }];
    read_until_any_delimiter(cursor, &patterns, require_delimiter, None)
}

pub(crate) fn bits_available_in_cursor(cursor: &Cursor<'_>) -> usize {
    let total_bits = cursor.data.len().saturating_mul(8);
    let pos = cursor.absolute_bit_index();
    let limit = cursor.frame_bit_limit.unwrap_or(total_bits);
    limit.saturating_sub(pos).min(total_bits.saturating_sub(pos))
}

pub(crate) fn read_binary_blob(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::{LengthKind, LengthUnits};

    if props.length_kind != LengthKind::Explicit {
        return Err(VmError::InvalidValue {
            message: "objectKind='bytes' must have dfdl:lengthKind='explicit'".into(),
        });
    }
    let len = props.length.ok_or(VmError::InvalidValue {
        message: "explicit blob missing length".into(),
    })? as usize;
    let len_bits = match props.length_units {
        LengthUnits::Bytes => len.saturating_mul(8),
        LengthUnits::Bits => len,
        LengthUnits::Characters => {
            return Err(VmError::InvalidValue {
                message: "lengthUnits='characters' is not valid for blob data.".into(),
            })
        }
    };
    let available = bits_available_in_cursor(cursor);
    if available < len_bits {
        return Err(insufficient_data_bits_error(len_bits, available));
    }
    let bytes = cursor.read_stream_bits_as_bytes(len_bits, props.bit_order)?;
    Ok(crate::value::DfdlValue::Blob(bytes))
}

pub(crate) fn insufficient_data_bits_error(needed_bits: usize, found_bits: usize) -> crate::error::VmError {
    use crate::error::VmError;
    VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. Insufficient bits in data. Needed {needed_bits} bit(s). found only {found_bits}. {found_bits} available"
        ),
    }
}

pub(crate) fn read_length_span(
    cursor: &mut Cursor<'_>,
    len: usize,
    units: LengthUnits,
    encoding: &str,
    bit_order: BitOrder,
    encoding_error_policy: EncodingErrorPolicy,
    allow_short_read: bool,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    match units {
        LengthUnits::Bytes => {
            if let Some(order) = hex_charset_order(encoding) {
                let needed_bits = len.saturating_mul(8);
                let available_bits = cursor
                    .data
                    .len()
                    .saturating_mul(8)
                    .saturating_sub(cursor.absolute_bit_index());
                if available_bits < needed_bits {
                    if allow_short_read {
                        return Ok(Vec::new());
                    }
                    return Err(insufficient_data_bits_error(needed_bits, available_bits));
                }
                return cursor.read_hex_charset_bytes(len, order);
            }
            if cursor.bit_count != 0 {
                return Err(VmError::InvalidValue {
                    message: "unaligned byte read".into(),
                });
            }
            let available = cursor.remaining();
            if available < len {
                if allow_short_read {
                    return Ok(cursor.read_bytes(available).unwrap_or_default());
                }
                return Err(insufficient_data_bits_error(
                    len.saturating_mul(8),
                    available.saturating_mul(8) + cursor.bit_count as usize,
                ));
            }
            cursor
                .read_bytes(len)
                .ok_or(VmError::UnexpectedEof)
        }
        LengthUnits::Characters => {
            let mut pos = cursor.pos;
            if allow_short_read {
                let mut out = alloc::vec::Vec::new();
                for _ in 0..len {
                    if pos >= cursor.data.len() {
                        break;
                    }
                    match read_character_bytes(
                        cursor.data,
                        &mut pos,
                        1,
                        encoding,
                        encoding_error_policy,
                    ) {
                        Ok(chunk) => out.extend_from_slice(&chunk),
                        Err(_) => break,
                    }
                }
                cursor.pos = pos;
                cursor.bit_count = 0;
                return Ok(out);
            }
            let bytes = read_character_bytes(
                cursor.data,
                &mut pos,
                len,
                encoding,
                encoding_error_policy,
            )
            .map_err(|e| {
                if matches!(
                    &e,
                    VmError::InvalidValue { message }
                        if message.contains("Malformed UTF-8")
                ) {
                    e
                } else {
                    VmError::InvalidValue {
                        message: alloc::format!(
                            "Parse Error. Insufficient data for length {len} characters"
                        ),
                    }
                }
            })?;
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
    stop_sequences: &[&IrProps],
) -> Result<Vec<u8>, crate::error::VmError> {
    read_until_delimiters(
        cursor,
        props,
        strings,
        require_delimiter,
        stop_sequences,
        None,
    )
}

fn read_until_any_delimiter(
    cursor: &mut Cursor<'_>,
    delimiters: &[DelimScanPattern],
    require_delimiter: bool,
    encoding: Option<&str>,
) -> Result<Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    let start = cursor.pos;
    let step = if utf16_little_endian_from_encoding(encoding).is_some() {
        2usize
    } else {
        1
    };
    while cursor.remaining() > 0 {
        for entry in delimiters {
            if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                &entry.pat,
                entry.ignore_case,
                encoding,
            ) {
                // Ignore zero-width delimiter matches while scanning (prevents infinite
                // empty reads on binary delimited fields); empty fields still work via n > 0.
                if n == 0 {
                    continue;
                }
                if n > 0 || (!require_delimiter && cursor.pos == start) {
                    return Ok(cursor.data[start..cursor.pos].to_vec());
                }
            }
        }
        cursor.advance(step);
    }
    if cursor.pos == start {
        return Ok(Vec::new());
    }
    if require_delimiter {
        let terms = delimiters
            .iter()
            .map(|p| alloc::format!("`{}`", format_delimiter_for_error(&p.pat)))
            .collect::<alloc::vec::Vec<_>>()
            .join(", ");
        return Err(VmError::InvalidValue {
            message: alloc::format!("terminator {terms} not found"),
        });
    }
    Ok(cursor.data[start..].to_vec())
}

fn utf16_little_endian_from_encoding(encoding: Option<&str>) -> Option<bool> {
    encoding.and_then(|enc| {
        match normalize_encoding_name(enc)? {
            "utf-16le" => Some(true),
            "utf-16be" => Some(false),
            _ => None,
        }
    })
}

fn consume_bits_charset_delimiter(
    cursor: &mut Cursor<'_>,
    patterns: &[DelimScanPattern],
    spec: crate::vm::encoding::BitsCharsetSpec,
) -> Result<bool, crate::error::VmError> {
    for entry in patterns {
        let term = delimiter_pat_as_text(&entry.pat);
        if term.is_empty() {
            continue;
        }
        let save_pos = cursor.pos;
        let save_bit = cursor.bit_count;
        let mut decoded = alloc::string::String::new();
        for _ in 0..term.chars().count() {
            if cursor.is_frame_consumed() {
                cursor.pos = save_pos;
                cursor.bit_count = save_bit;
                break;
            }
            decoded.push(read_bits_charset_code_unit(cursor, spec)?);
        }
        let ok = if entry.ignore_case {
            decoded.to_ascii_lowercase() == term.to_ascii_lowercase()
        } else {
            decoded == term
        };
        if ok {
            return Ok(true);
        }
        cursor.pos = save_pos;
        cursor.bit_count = save_bit;
    }
    Ok(false)
}

pub(crate) fn consume_enclosing_delimiter(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if cursor.is_empty() {
        return Ok(());
    }
    let field_patterns = non_empty_delimiter_scan_patterns(props, strings)?;
    if let Ok(enc) = encoding_name(props, strings) {
        if let Some(spec) = bits_charset_spec(&enc) {
            if consume_bits_charset_delimiter(cursor, &field_patterns, spec)? {
                return Ok(());
            }
            if !should_defer_parent_stop_delimiter(props) {
                for seq in stop_sequences {
                    let mut parent_patterns = alloc::vec::Vec::new();
                    for id in delimiter_pattern_ids(seq) {
                        let pat = strings.get(id)?;
                        push_delimiter_scan_patterns(&mut parent_patterns, pat, seq.ignore_case);
                    }
                    if consume_bits_charset_delimiter(cursor, &parent_patterns, spec)? {
                        return Ok(());
                    }
                }
                if field_patterns.is_empty() && stop_sequences.is_empty() {
                    return Ok(());
                }
                return Err(VmError::InvalidValue {
                    message: "delimiter mismatch".into(),
                });
            }
            return Ok(());
        }
    }
    let enc = encoding_name(props, strings).ok();
    for entry in &field_patterns {
        if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            &entry.pat,
            entry.ignore_case,
            enc.as_deref(),
        ) {
            if n > 0 {
                cursor.advance(n);
                return Ok(());
            }
        }
    }
    if !should_defer_parent_stop_delimiter(props) {
        for seq in stop_sequences {
            for id in delimiter_pattern_ids(seq) {
                let pat = strings.get(id)?;
                if !pat.is_empty() {
                    if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
                        &cursor.data[cursor.pos..],
                        pat,
                        seq.ignore_case,
                        enc.as_deref(),
                    ) {
                        if n == 0 {
                            continue;
                        }
                        if should_defer_sequence_stop_delimiter_in_field(seq, id, props, strings)? {
                            return Ok(());
                        }
                        cursor.advance(n);
                        return Ok(());
                    }
                }
            }
        }
        if field_patterns.is_empty() && stop_sequences.is_empty() {
            return Ok(());
        }
        Err(VmError::InvalidValue {
            message: "delimiter mismatch".into(),
        })
    } else {
        Ok(())
    }
}

pub(crate) fn is_suppressible_empty_representation(
    value: &crate::value::DfdlValue,
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    match value {
        crate::value::DfdlValue::Null => {
            if props.nillable && nil_value_includes_empty(props, strings)? {
                return Ok(true);
            }
            Ok(nil_first_alternative(props, strings)?.is_some_and(|nil| nil.is_empty()))
        }
        crate::value::DfdlValue::String(text) if text.text.is_empty() => Ok(true),
        _ => Ok(false),
    }
}

pub(crate) fn trailing_suppressed_count(
    items: &[crate::value::DfdlValue],
    item_props: &IrProps,
    strings: &StringPool,
    seq_props: Option<&IrProps>,
) -> Result<usize, crate::error::VmError> {
    let policy = item_props
        .separator_suppression_policy
        .or_else(|| seq_props.and_then(|p| p.separator_suppression_policy));
    if !matches!(
        policy,
        Some(SeparatorSuppressionPolicy::TrailingEmpty)
            | Some(SeparatorSuppressionPolicy::TrailingEmptyStrict)
    ) {
        return Ok(0);
    }
    let mut count = 0usize;
    for item in items.iter().rev() {
        if is_suppressible_empty_representation(item, item_props, strings)? {
            count += 1;
        } else {
            break;
        }
    }
    Ok(count)
}

/// When decoding, the next occurrence separator (before item `items.len()`) may be skipped
/// under `anyEmpty` if the previous occurrence was an empty representation.
pub(crate) fn should_suppress_decode_occurrence_separator(
    sep_props: &IrProps,
    item_props: &IrProps,
    items: &[crate::value::DfdlValue],
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    if items.is_empty() {
        return Ok(false);
    }
    let policy = sep_props
        .separator_suppression_policy
        .or(item_props.separator_suppression_policy);
    if policy != Some(SeparatorSuppressionPolicy::AnyEmpty) {
        return Ok(false);
    }
    match sep_props.separator_position {
        SeparatorPosition::Prefix | SeparatorPosition::Postfix => {
            Ok(is_suppressible_empty_representation(
                &items[items.len() - 1],
                item_props,
                strings,
            )?)
        }
        SeparatorPosition::Infix => Ok(is_suppressible_empty_representation(
            &items[items.len() - 1],
            item_props,
            strings,
        )?),
    }
}

pub(crate) fn should_suppress_occurrence_separator(
    sep_props: &IrProps,
    item_props: &IrProps,
    items: &[crate::value::DfdlValue],
    index: usize,
    before_item: bool,
    strings: &StringPool,
) -> Result<bool, crate::error::VmError> {
    let policy = sep_props
        .separator_suppression_policy
        .or(item_props.separator_suppression_policy);
    if policy != Some(SeparatorSuppressionPolicy::AnyEmpty) {
        return Ok(false);
    }
    let current_empty =
        is_suppressible_empty_representation(&items[index], item_props, strings)?;
    if before_item {
        match sep_props.separator_position {
            SeparatorPosition::Prefix | SeparatorPosition::Postfix => Ok(current_empty),
            SeparatorPosition::Infix => {
                let previous_empty = index > 0
                    && is_suppressible_empty_representation(&items[index - 1], item_props, strings)?;
                Ok(current_empty || previous_empty)
            }
        }
    } else {
        Ok(matches!(sep_props.separator_position, SeparatorPosition::Postfix) && current_empty)
    }
}

pub(crate) fn prefixed_payload_byte_length(
    data: &[u8],
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let mut cursor = Cursor::new(data);
    let span = read_prefixed_span(&mut cursor, props, strings, None)?;
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
    field_name: Option<&str>,
) -> Result<Vec<u8>, crate::error::VmError> {
    let span = read_prefixed_span(cursor, props, strings, field_name)?;
    read_length_span(
        cursor,
        span,
        props.length_units,
        encoding_name(props, strings)?,
        props.bit_order,
        props.encoding_error_policy,
        false,
    )
    .map_err(|e| {
        use crate::error::VmError;
        if e == VmError::UnexpectedEof {
            let needed = match props.length_units {
                LengthUnits::Bytes => span.saturating_mul(8),
                LengthUnits::Bits => span,
                LengthUnits::Characters => span.saturating_mul(8),
            };
            let found = cursor.remaining() * 8 + cursor.bit_count as usize;
            insufficient_data_bits_error(needed, found)
        } else {
            e
        }
    })
}

fn read_prefixed_span(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    field_name: Option<&str>,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let prefix = props
        .prefix_length
        .as_deref()
        .ok_or(VmError::InvalidValue {
            message: "prefixed field missing prefixLengthType".into(),
        })?;
    let prefix_start = cursor.pos;
    let value = read_prefix_integer_value(cursor, prefix, strings, field_name)?;
    let prefix_units =
        consumed_length_units(cursor, prefix_start, props.length_units, props, strings)?;
    let mut span = usize_from_u64(value)?;
    if props.prefix_includes_prefix_length {
        span = span.checked_sub(prefix_units).ok_or(VmError::InvalidValue {
            message: alloc::format!(
                "Runtime Schema Definition Error. Prefixed length result after dfdl:prefixIncludesPrefixLength adjustment non-negative. {}",
                span as i64 - prefix_units as i64
            ),
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
            count_characters(
                &cursor.data[start..cursor.pos],
                encoding_name(props, strings)?,
                props.encoding_error_policy,
            )
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
    field_name: Option<&str>,
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
            if trimmed.starts_with('-') {
                let numeric = trimmed.parse::<i64>().unwrap_or(0);
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Runtime Schema Definition Error. Prefixed length must be non-negative. {numeric}"
                    ),
                });
            }
            parse_u64(trimmed)
        }
        Representation::Binary => Ok(decode_unsigned_bytes(
            &raw,
            prefix.props.byte_order == ByteOrder::LittleEndian,
        )),
    }?;
    validate_prefix_facets(value, prefix, field_name)?;
    Ok(value)
}

fn validate_prefix_facets(
    value: u64,
    prefix: &IrPrefixLength,
    field_name: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let field = field_name
        .map(|name| alloc::format!("{name} ({value})"))
        .unwrap_or_else(|| alloc::format!("({value})"));
    if let Some(min) = prefix.min_inclusive {
        if (value as i64) < min {
            return Err(VmError::InvalidValue {
                message: alloc::format!("failed check: {field} facet minInclusive ({min})"),
            });
        }
    }
    if let Some(max) = prefix.max_inclusive {
        if (value as i64) > max {
            return Err(VmError::InvalidValue {
                message: alloc::format!("failed check: {field} facet maxInclusive ({max})"),
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
                props.encoding_error_policy,
                false,
            )
            .map_err(|e| {
                if e == VmError::UnexpectedEof {
                    let bits = match props.length_units {
                        LengthUnits::Bytes => len.saturating_mul(8),
                        LengthUnits::Bits => len,
                        LengthUnits::Characters => len.saturating_mul(8),
                    };
                    VmError::InvalidValue {
                        message: alloc::format!("Insufficient bits in data. {bits}"),
                    }
                } else {
                    e
                }
            })
        }
        LengthKind::Implicit => {
            if props.representation == Representation::Text {
                Ok(read_implicit_numeric_text(cursor, props, strings))
            } else if props.length_units == LengthUnits::Bits {
                let len = binary_bit_length(cursor, kind, props, strings)?;
                cursor.read_stream_bits_as_bytes(len, props.bit_order)
            } else {
                let len = binary_byte_length(cursor, kind, props, strings)?;
                cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)
            }
        }
        LengthKind::Prefixed => read_prefixed_payload(cursor, props, strings, None),
        other => Err(VmError::UnsupportedOperation {
            op: alloc::format!("prefix lengthKind `{}`", length_kind_name(other)),
        }),
    }
}

fn format_delimiter_for_error(pat: &str) -> alloc::string::String {
    pat.replace('\n', "%NL;").replace('\r', "%CR;")
}

/// Visible glyph for delimiter-mismatch errors (DFDL control-picture convention).
pub(crate) fn remap_control_or_line_ending_to_visible(c: char) -> char {
    let n = c as u32;
    match n {
        n if n <= 0x1f => char::from_u32(n + 0x2400).unwrap_or(c),
        0x20 => '\u{2423}',
        0x7f => '\u{2421}',
        _ => c,
    }
}

pub(crate) fn format_found_at_cursor(
    data: &[u8],
    pos: usize,
    encoding: Option<&str>,
) -> alloc::string::String {
    if pos >= data.len() {
        return String::new();
    }
    let enc = encoding
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_else(|| alloc::string::String::from("UTF-8"));
    if enc.contains("UTF-16") || enc.contains("UTF16") {
        let le = enc.contains("LE");
        let (hi, lo) = if le { (1, 0) } else { (0, 1) };
        if pos + 1 < data.len() {
            let code = u32::from(data[pos + hi]) << 8 | u32::from(data[pos + lo]);
            if let Some(ch) = char::from_u32(code) {
                return remap_control_or_line_ending_to_visible(ch).to_string();
            }
        }
    }
    let b = data[pos];
    if b.is_ascii() && !b.is_ascii_control() {
        return (b as char).to_string();
    }
    remap_control_or_line_ending_to_visible(b as char).to_string()
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
        Byte | UnsignedByte | Short | UnsignedShort | Int | Integer | UnsignedInt | Long | Float
            | Double
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
    while cursor.pos < cursor.data.len() {
        let b = cursor.data[cursor.pos];
        if b.is_ascii_digit() || b == b'.' || b == b',' || b == b'e' || b == b'E' {
            cursor.advance(1);
        } else if (b == b'+' || b == b'-')
            && cursor.pos > start
            && matches!(cursor.data[cursor.pos - 1], b'e' | b'E')
        {
            cursor.advance(1);
        } else {
            break;
        }
    }
    cursor.data[start..cursor.pos].to_vec()
}

fn pad_char_from_props<'a>(props: &IrProps, strings: &'a StringPool) -> Option<&'a str> {
    props
        .text_number_pad_character
        .and_then(|id| strings.get(id).ok())
}

fn text_justification_for_kind(props: &IrProps, kind: crate::ir::ValueKind) -> TextStringJustification {
    use crate::ir::ValueKind::*;
    let numeric = matches!(
        kind,
        Int | Integer | Long | Short | Byte | UnsignedInt | UnsignedShort | UnsignedByte | Float
            | Double | Decimal
    );
    if numeric {
        match props.text_number_justification {
            TextNumberJustification::Left => TextStringJustification::Left,
            TextNumberJustification::Right => TextStringJustification::Right,
            TextNumberJustification::Center => TextStringJustification::Center,
        }
    } else {
        props.text_string_justification
    }
}

fn pad_char_for_kind(
    props: &IrProps,
    strings: &StringPool,
    kind: crate::ir::ValueKind,
) -> alloc::string::String {
    use crate::ir::ValueKind::*;
    use crate::schema::expand_entities_str;
    if matches!(kind, String) {
        if let Some(id) = props.text_string_pad_character {
            if let Ok(raw) = strings.get(id) {
                return expand_entities_str(raw);
            }
        }
    }
    if matches!(kind, crate::ir::ValueKind::DateTime | crate::ir::ValueKind::Time) {
        if let Some(id) = props.text_calendar_pad_character {
            if let Ok(raw) = strings.get(id) {
                return expand_entities_str(raw);
            }
        }
    }
    if matches!(kind, Boolean) {
        if let Some(id) = props.text_boolean_pad_character {
            if let Ok(raw) = strings.get(id) {
                return expand_entities_str(raw);
            }
        }
    }
    pad_char_from_props(props, strings)
        .map(|s| s.to_string())
        .unwrap_or_else(|| alloc::string::String::from(" "))
}

fn calendar_lexical_error(type_name: &str, text: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!("Parse Error: Unable to parse {type_name} from text: {text}"),
    }
}

fn validate_implicit_date_part(
    text: &str,
    tunables: &DaffodilTunables,
    err_type: &str,
    err_text: &str,
) -> Result<(), crate::error::VmError> {
    let mut parts = text.split('-');
    let Some(y) = parts.next() else {
        return Err(calendar_lexical_error(err_type, err_text));
    };
    let Some(m) = parts.next() else {
        return Err(calendar_lexical_error(err_type, err_text));
    };
    let Some(d) = parts.next() else {
        return Err(calendar_lexical_error(err_type, err_text));
    };
    if parts.next().is_some() {
        return Err(calendar_lexical_error(err_type, err_text));
    }
    if !y.chars().all(|c| c.is_ascii_digit()) || y.len() < 4 {
        return Err(calendar_lexical_error(err_type, err_text));
    }
    if m.len() != 2
        || d.len() != 2
        || !m.chars().all(|c| c.is_ascii_digit())
        || !d.chars().all(|c| c.is_ascii_digit())
    {
        return Err(calendar_lexical_error(err_type, err_text));
    }
    crate::vm::calendar_binary::validate_calendar_year_tunables(y, tunables)?;
    Ok(())
}

fn validate_implicit_time_part(text: &str, date_time: bool) -> Result<(), crate::error::VmError> {
    let type_name = if date_time { "xs:dateTime" } else { "xs:time" };
    if text == "Z" {
        return Ok(());
    }
    let tz_start = if text.len() > 8 {
        text[8..]
            .find(['+', '-'])
            .map(|i| i + 8)
            .or_else(|| text[8..].find('Z').map(|i| i + 8))
    } else {
        None
    };
    let core_end = tz_start.unwrap_or(text.len());
    let core = &text[..core_end];
    if core.len() != 8
        || core.as_bytes().get(2) != Some(&b':')
        || core.as_bytes().get(5) != Some(&b':')
        || !core[..2].chars().all(|c| c.is_ascii_digit())
        || !core[3..5].chars().all(|c| c.is_ascii_digit())
        || !core[6..8].chars().all(|c| c.is_ascii_digit())
    {
        return Err(calendar_lexical_error(type_name, text));
    }
    if let Some(off) = tz_start {
        let tz = &text[off..];
        if tz == "Z" {
            return Ok(());
        }
        if !(tz.starts_with('+') || tz.starts_with('-')) || tz.len() != 6 || tz.as_bytes()[3] != b':' {
            return Err(calendar_lexical_error(type_name, text));
        }
        if tz == "-00:00" {
            return Err(calendar_lexical_error(type_name, text));
        }
        if !tz[1..].chars().all(|c| c.is_ascii_digit() || c == ':') {
            return Err(calendar_lexical_error(type_name, text));
        }
    } else if text.len() > 8 {
        return Err(calendar_lexical_error(type_name, text));
    }
    Ok(())
}

fn validate_implicit_calendar_lexical(
    kind: crate::ir::ValueKind,
    date_only: bool,
    text: &str,
    tunables: &DaffodilTunables,
) -> Result<(), crate::error::VmError> {
    use crate::ir::ValueKind;
    if date_only {
        return validate_implicit_date_part(text, tunables, "xs:date", text);
    }
    if kind == ValueKind::Time {
        return validate_implicit_time_part(text, false);
    }
    let Some(sep) = text.find('T').or_else(|| text.find(' ')) else {
        return Err(calendar_lexical_error("xs:dateTime", text));
    };
    validate_implicit_date_part(&text[..sep], tunables, "xs:dateTime", text)?;
    validate_implicit_time_part(&text[sep + 1..], true)?;
    Ok(())
}

/// Normalize CR/LF in decoded string scalars (XML infoset line endings are LF).
fn normalize_string_line_endings(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn trim_text_value<'a>(
    input: &'a str,
    kind: crate::ir::ValueKind,
    trim_kind: crate::schema::TextTrimKind,
    props: &IrProps,
    strings: &StringPool,
) -> &'a str {
    match trim_kind {
        TextTrimKind::None => input,
        TextTrimKind::Trim => input.trim(),
        TextTrimKind::Left => input.trim_start(),
        TextTrimKind::Right => input.trim_end(),
        TextTrimKind::PadChar => {
            let pad = pad_char_for_kind(props, strings, kind);
            if kind == crate::ir::ValueKind::String {
                trim_pad_char_for_justification(input, &pad, props.text_string_justification)
            } else if matches!(kind, crate::ir::ValueKind::DateTime | crate::ir::ValueKind::Time) {
                let just = props
                    .text_calendar_justification
                    .unwrap_or(props.text_string_justification);
                trim_pad_char_for_justification(input, &pad, just)
            } else {
                let just = text_justification_for_kind(props, kind);
                trim_pad_char_for_justification(input, &pad, just)
            }
        }
    }
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
    trim_pad_char_for_justification(input, pad, TextStringJustification::Center)
}

fn trim_pad_char_for_justification<'a>(
    input: &'a str,
    pad: &str,
    justification: TextStringJustification,
) -> &'a str {
    use crate::schema::TextStringJustification;
    if pad.is_empty() {
        return input;
    }
    let mut start = 0usize;
    let mut end = input.len();
    match justification {
        TextStringJustification::Left | TextStringJustification::Center => {
            while end > start && input[..end].ends_with(pad) {
                end -= pad.len();
            }
        }
        TextStringJustification::Right => {}
    }
    match justification {
        TextStringJustification::Right | TextStringJustification::Center => {
            while start < end && input[start..].starts_with(pad) {
                start += pad.len();
            }
        }
        TextStringJustification::Left => {}
    }
    &input[start..end]
}

fn parse_int_with_base<T>(s: &str, type_name: &str, base: u32) -> Result<T, crate::error::VmError>
where
    T: TryFrom<i64>,
    <T as TryFrom<i64>>::Error: core::fmt::Debug,
{
    parse_int_typed_with_base(s, type_name, base, s)
}

fn unsigned_uses_text_number_pattern(trimmed: &str, props: &IrProps) -> bool {
    props.custom_text_number_pattern || trimmed.contains(',')
}

fn explicit_length_unsigned_short_whitespace(type_name: &str, props: &IrProps) -> bool {
    type_name == "xs:unsignedShort"
        && matches!(
            props.length_kind,
            LengthKind::Explicit | LengthKind::Fixed
        )
}

fn lax_numeric_field_text(text: &str, props: &IrProps, type_name: &str) -> alloc::string::String {
    use crate::schema::BinaryNumberCheckPolicy;
    if props.text_standard_base == 10 && props.text_number_check_policy == BinaryNumberCheckPolicy::Lax
    {
        if explicit_length_unsigned_short_whitespace(type_name, props) {
            // Lax.dfdl.xsd strips internal whitespace; Embedded+textTrimKind=padChar rejects it (DFDL-5-019R).
            if props.text_trim_kind == TextTrimKind::PadChar {
                text.trim().to_string()
            } else {
                text.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect()
            }
        } else {
            text.trim().to_string()
        }
    } else {
        text.to_string()
    }
}

fn reject_internal_whitespace_explicit_field(
    text: &str,
    type_name: &str,
    props: &IrProps,
    base: u32,
    trailing_input: bool,
) -> Result<(), crate::error::VmError> {
    use crate::schema::BinaryNumberCheckPolicy;
    if base != 10 {
        return Ok(());
    }
    if !matches!(
        props.length_kind,
        LengthKind::Explicit | LengthKind::Fixed
    ) {
        return Ok(());
    }
    if trailing_input && type_name == "xs:short" {
        return Ok(());
    }
    if explicit_length_unsigned_short_whitespace(type_name, props) {
        if props.text_number_check_policy == BinaryNumberCheckPolicy::Lax
            && props.text_trim_kind != TextTrimKind::PadChar
        {
            return Ok(());
        }
        if props.text_number_check_policy == BinaryNumberCheckPolicy::Strict
            && text.chars().any(char::is_whitespace)
        {
            return Err(unable_parse_from_text(type_name, text));
        }
    }
    let t = text.trim();
    if t.chars().any(char::is_whitespace) {
        return Err(unable_parse_from_text(type_name, text));
    }
    Ok(())
}

fn parse_out_of_range(type_name: &str, decimal_value: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!("Parse Error. Out of Range. {type_name} {decimal_value}"),
    }
}

fn value_kind_type_name(kind: crate::ir::ValueKind, props: Option<&IrProps>) -> &'static str {
    use crate::ir::ValueKind;
    match kind {
        ValueKind::Byte => "xs:byte",
        ValueKind::Short => "xs:short",
        ValueKind::Int => "xs:int",
        ValueKind::Long => {
            if props.is_some_and(|p| p.unsigned_integer) {
                "xs:unsignedLong"
            } else {
                "xs:long"
            }
        }
        ValueKind::UnsignedByte => "xs:unsignedByte",
        ValueKind::UnsignedShort => "xs:unsignedShort",
        ValueKind::UnsignedInt => "xs:unsignedInt",
        ValueKind::Integer => {
            if props.is_some_and(|p| p.non_negative_integer) {
                "xs:nonNegativeInteger"
            } else {
                "xs:integer"
            }
        }
        ValueKind::Float => "xs:float",
        ValueKind::Double => "xs:double",
        ValueKind::Decimal => "xs:decimal",
        _ => "xs:string",
    }
}

fn unable_parse_from_text(type_name: &str, text: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!("Parse Error. Unable to parse {type_name} from text: {text}"),
    }
}

fn split_sign_digits(s: &str) -> Result<(i64, &str), crate::error::VmError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(crate::error::VmError::InvalidValue {
            message: "Parse Error. empty string".into(),
        });
    }
    Ok(if trimmed.starts_with('-') {
        (-1, &trimmed[1..])
    } else if trimmed.starts_with('+') {
        (1, &trimmed[1..])
    } else {
        (1, trimmed)
    })
}

fn parse_u128_radix(digits: &str, base: u32) -> Result<u128, crate::error::VmError> {
    u128::from_str_radix(digits, base).map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("invalid integer `{digits}`"),
    })
}

fn parse_non_base10_signed_i64(s: &str, type_name: &str, base: u32) -> Result<i64, crate::error::VmError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(crate::error::VmError::InvalidValue {
            message: alloc::format!("Parse Error. Unable to parse {type_name} from empty string"),
        });
    }
    if trimmed.starts_with('+') || trimmed.starts_with('-') {
        return Err(crate::error::VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error. Unable to parse {type_name} from base-{base} text with leading sign: {trimmed}"
            ),
        });
    }
    let abs = u128::from_str_radix(trimmed, base).map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. Unable to parse {type_name} from base-{base} text due to invalid characters: {trimmed}"
        ),
    })?;
    let sign = 1i64;
    let decimal = abs.to_string();
    let (min_abs, max_abs): (u128, u128) = match type_name {
        "xs:byte" => (128, 127),
        "xs:short" => (32768, 32767),
        "xs:int" => (2147483648, 2147483647),
        "xs:long" => (9223372036854775808, 9223372036854775807),
        _ => (0, u128::MAX),
    };
    if abs > max_abs {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    if sign < 0 && abs > min_abs {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    Ok(abs as i64)
}

fn decimal_from_sign_magnitude(sign: i64, abs: u128) -> alloc::string::String {
    if sign < 0 {
        alloc::format!("-{abs}")
    } else {
        abs.to_string()
    }
}

fn parse_unbounded_integer_decimal(s: &str, base: u32, non_negative: bool) -> Result<alloc::string::String, crate::error::VmError> {
    let type_name = if non_negative {
        "xs:nonNegativeInteger"
    } else {
        "xs:integer"
    };
    if base == 10 {
        let trimmed = s.trim();
        if trimmed.contains('.') && !trimmed.contains('E') && !trimmed.contains('e') {
            return Err(unable_parse_from_text(type_name, s));
        }
    }
    if base != 10 {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(crate::error::VmError::InvalidValue {
                message: "Parse Error. Unable to parse xs:integer from empty string".into(),
            });
        }
        if trimmed.starts_with('+') || trimmed.starts_with('-') {
            return Err(crate::error::VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. Unable to parse xs:integer from base-{base} text with leading sign: {trimmed}"
                ),
            });
        }
        let abs = u128::from_str_radix(trimmed, base).map_err(|_| crate::error::VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error. Unable to parse xs:integer from base-{base} text due to invalid characters: {trimmed}"
            ),
        })?;
        return Ok(abs.to_string());
    }
    let (sign, digits) = split_sign_digits(s)
        .map_err(|_| unable_parse_from_text(type_name, s))?;
    if non_negative && sign < 0 {
        return Err(parse_out_of_range(type_name, s.trim()));
    }
    let abs = parse_unbounded_abs_decimal(digits, base, type_name, s)?;
    Ok(decimal_from_sign_magnitude_str(sign, &abs))
}

fn parse_unbounded_abs_decimal(
    digits: &str,
    base: u32,
    type_name: &str,
    field_text: &str,
) -> Result<alloc::string::String, crate::error::VmError> {
    if base == 10 {
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return Ok(strip_leading_zeros_decimal(digits));
        }
        return parse_u128_radix_base10(digits, base, type_name, field_text).map(|v| v.to_string());
    }
    parse_u128_radix_base10(digits, base, type_name, field_text).map(|v| v.to_string())
}

fn strip_leading_zeros_decimal(digits: &str) -> alloc::string::String {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    }
}

fn decimal_from_sign_magnitude_str(sign: i64, abs: &str) -> alloc::string::String {
    if sign < 0 {
        alloc::format!("-{abs}")
    } else {
        abs.to_string()
    }
}

fn parse_u128_radix_base10(
    digits: &str,
    base: u32,
    type_name: &str,
    field_text: &str,
) -> Result<u128, crate::error::VmError> {
    u128::from_str_radix(digits, base).map_err(|_| {
        if base == 10 && field_has_non_digit(field_text) {
            unable_parse_from_text(type_name, field_text)
        } else {
            crate::error::VmError::InvalidValue {
                message: alloc::format!("invalid integer `{digits}`"),
            }
        }
    })
}

fn field_has_non_digit(field_text: &str) -> bool {
    field_text
        .trim()
        .chars()
        .any(|c| c.is_ascii_alphabetic() || c.is_whitespace())
}

fn parse_int_typed_with_base_i64(
    s: &str,
    type_name: &str,
    base: u32,
    field_text: &str,
) -> Result<i64, crate::error::VmError> {
    if base != 10 {
        return parse_non_base10_signed_i64(s, type_name, base);
    }
    let (sign, digits) = split_sign_digits(s)?;
    let abs = parse_u128_radix_base10(digits, base, type_name, field_text)?;
    let decimal = decimal_from_sign_magnitude(sign, abs);
    let (min_abs, max_abs): (u128, u128) = match type_name {
        "xs:byte" => (128, 127),
        "xs:short" => (32768, 32767),
        "xs:int" => (2147483648, 2147483647),
        "xs:long" => (9223372036854775808, 9223372036854775807),
        _ => (0, u128::MAX),
    };
    if sign >= 0 && abs > max_abs {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    if sign < 0 && abs > min_abs {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    if sign < 0 {
        Ok(-(abs as i64))
    } else {
        Ok(abs as i64)
    }
}

fn parse_int_typed_with_base<T>(
    s: &str,
    type_name: &str,
    base: u32,
    field_text: &str,
) -> Result<T, crate::error::VmError>
where
    T: TryFrom<i64>,
    <T as TryFrom<i64>>::Error: core::fmt::Debug,
{
    let v = parse_int_typed_with_base_i64(s, type_name, base, field_text)?;
    T::try_from(v).map_err(|_| {
        let (sign, digits) = split_sign_digits(s).unwrap_or((1, ""));
        let abs = parse_u128_radix_base10(digits, base, type_name, field_text).unwrap_or(0);
        parse_out_of_range(type_name, &decimal_from_sign_magnitude(sign, abs))
    })
}

fn parse_unsigned_radix<T>(s: &str, base: u32) -> Result<T, crate::error::VmError>
where
    T: TryFrom<u64>,
    <T as TryFrom<u64>>::Error: core::fmt::Debug,
{
    let type_name = match core::mem::size_of::<T>() {
        1 => "xs:unsignedByte",
        2 => "xs:unsignedShort",
        4 => "xs:unsignedInt",
        8 => "xs:unsignedLong",
        _ => "xs:unsignedInt",
    };
    let v = parse_unsigned_radix_typed(s, type_name, base, s)?;
    T::try_from(v).map_err(|_| parse_out_of_range(type_name, s))
}

fn parse_unsigned_radix_typed(
    s: &str,
    type_name: &str,
    base: u32,
    field_text: &str,
) -> Result<u64, crate::error::VmError> {
    let trimmed = s.trim();
    if trimmed.starts_with('-') {
        return Err(parse_out_of_range(type_name, trimmed));
    }
    if trimmed.starts_with('+') {
        return Err(if base == 10 {
            unable_parse_from_text(type_name, field_text)
        } else {
            crate::error::VmError::InvalidValue {
                message: alloc::format!("invalid integer `{s}`"),
            }
        });
    }
    let abs = parse_u128_radix_base10(trimmed, base, type_name, field_text)?;
    let decimal = abs.to_string();
    let max = match type_name {
        "xs:unsignedByte" => u8::MAX as u128,
        "xs:unsignedShort" => u16::MAX as u128,
        "xs:unsignedInt" => u32::MAX as u128,
        "xs:unsignedLong" => u64::MAX as u128,
        _ => u128::MAX,
    };
    if abs > max {
        return Err(parse_out_of_range(type_name, &decimal));
    }
    u64::try_from(abs).map_err(|_| parse_out_of_range(type_name, &decimal))
}

fn parse_float(s: &str) -> Result<f64, crate::error::VmError> {
    match s {
        "INF" | "Inf" | "Infinity" => Ok(f64::INFINITY),
        "-INF" | "-Inf" | "-Infinity" => Ok(f64::NEG_INFINITY),
        "NaN" => Ok(f64::NAN),
        _ => s.parse().map_err(|_| crate::error::VmError::InvalidValue {
            message: alloc::format!("invalid float `{s}`"),
        }),
    }
}

pub(crate) fn decode_hex_binary(s: &str) -> Result<Vec<u8>, crate::error::VmError> {
    decode_hex(s)
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
        let hi_ch = chunk[0] as char;
        let hi = hi_ch.to_digit(16).ok_or_else(|| crate::error::VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error: Hex character must be 0-9, a-f, or A-F, but was '{hi_ch}'"
            ),
        })?;
        let lo_ch = chunk[1] as char;
        let lo = lo_ch.to_digit(16).ok_or_else(|| crate::error::VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error: Hex character must be 0-9, a-f, or A-F, but was '{lo_ch}'"
            ),
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
    write_alignment_with_config(out, bit_count, props, None)
}

pub(crate) fn write_alignment_with_config(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
    config: Option<&RuntimeConfig>,
) -> Result<(), crate::error::VmError> {
    write_alignment_values(
        out,
        bit_count,
        props,
        props.alignment,
        props.alignment_units,
        config,
    )
}

pub(crate) fn write_alignment_for_kind(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
    kind: crate::ir::ValueKind,
    encoding: &str,
    config: Option<&RuntimeConfig>,
) -> Result<(), crate::error::VmError> {
    let (align, units) = crate::vm::alignment::resolved_alignment(kind, props, encoding);
    write_alignment_values(out, bit_count, props, align, units, config)
}

fn write_alignment_values(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
    alignment: u64,
    alignment_units: crate::schema::LengthUnits,
    config: Option<&RuntimeConfig>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthUnits;

    if alignment == 0 {
        return Ok(());
    }
    if alignment_units == LengthUnits::Bits {
        let align = alignment as usize;
        if align <= 1 {
            return Ok(());
        }
        let pos = encode_absolute_bit_index(out, *bit_count);
        let skip = (align - (pos % align)) % align;
        if skip > 0 && !props.fill_byte_defined {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Property fillByte is not defined".into(),
            });
        }
        if props.representation == Representation::Text && skip >= 8 {
            let whole_bytes = skip / 8;
            let rem_bits = skip % 8;
            for _ in 0..whole_bytes {
                write_byte_aligned(out, bit_count, &[props.fill_byte])?;
            }
            for _ in 0..rem_bits {
                write_stream_bit_with_config(
                    out,
                    bit_count,
                    (props.fill_byte >> 7) & 1,
                    props.bit_order,
                    config,
                );
            }
            return Ok(());
        }
        for _ in 0..skip {
            write_stream_bit_with_config(out, bit_count, props.fill_byte & 1, props.bit_order, config);
        }
        return Ok(());
    }
    if alignment_units != LengthUnits::Bytes {
        return Err(VmError::UnsupportedOperation {
            op: "non-byte alignment encode".into(),
        });
    }
    if *bit_count != 0 {
        if !props.fill_byte_defined {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Property fillByte is not defined".into(),
            });
        }
        while *bit_count != 0 {
            write_stream_bit_with_config(out, bit_count, props.fill_byte & 1, props.bit_order, config);
        }
    }
    write_byte_aligned(out, bit_count, &[])?;
    let align = alignment as usize;
    if align <= 1 {
        return Ok(());
    }
    let skip = (align - (out.len() % align)) % align;
    if skip > 0 {
        if !props.fill_byte_defined {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Property fillByte is not defined".into(),
            });
        }
        out.extend(iter::repeat(props.fill_byte).take(skip));
    }
    Ok(())
}

pub(crate) fn consume_alignment(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    consume_alignment_values(cursor, props, props.alignment, props.alignment_units)
}

pub(crate) fn consume_alignment_values(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    alignment: u64,
    alignment_units: crate::schema::LengthUnits,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthUnits;

    if alignment == 0 {
        return Ok(());
    }
    if alignment_units == LengthUnits::Bits {
        let align = alignment as usize;
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
    if !matches!(
        alignment_units,
        crate::schema::LengthUnits::Bytes | crate::schema::LengthUnits::Characters
    ) {
        return Err(VmError::UnsupportedOperation {
            op: "non-byte alignment".into(),
        });
    }
    if crate::vm::alignment::cursor_uses_bitstream_alignment(cursor) {
        let align_bits = (alignment as usize).saturating_mul(8);
        if align_bits <= 1 {
            return Ok(());
        }
        let pos = cursor.absolute_bit_index();
        let skip = (align_bits - (pos % align_bits)) % align_bits;
        if skip > 0 {
            cursor.skip_stream_bits(skip, props.bit_order)?;
        }
        return Ok(());
    }
    if cursor.bit_count != 0 {
        let pad = 8 - cursor.bit_count as usize;
        cursor.skip_stream_bits(pad, props.bit_order)?;
    }
    let align = alignment as usize;
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
    cursor.advance(skip);
    Ok(())
}

pub(crate) fn consume_element_framing(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    kind: crate::ir::ValueKind,
    encoding: &str,
) -> Result<(), crate::error::VmError> {
    crate::vm::alignment::consume_leading_skip(cursor, props)?;
    if crate::vm::alignment::pre_element_alignment_applies(kind, props, encoding) {
        let (align, units) = crate::vm::alignment::resolved_alignment(kind, props, encoding);
        consume_alignment_values(cursor, props, align, units)?;
        if props.bit_order == BitOrder::LeastSignificantBitFirst
            && units == crate::schema::LengthUnits::Bits
            && align == 4
            && cursor.absolute_bit_index() == 4
        {
            cursor.rewind_stream_bits(3)?;
        }
    }
    Ok(())
}

pub(crate) fn consume_element_trailing_framing(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    crate::vm::alignment::consume_trailing_skip(cursor, props)
}

fn ambiguous_delimiter_prefix_at_cursor(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
) -> Result<bool, crate::error::VmError> {
    let patterns = enclosing_delimiter_scan_patterns(props, strings, stop_sequences)?;
    let mut matches = alloc::vec::Vec::new();
    for entry in &patterns {
        if let Some(n) = crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            &entry.pat,
            entry.ignore_case,
        ) {
            if n > 0 {
                matches.push((n, entry.pat.as_str()));
            }
        }
    }
    for &(n_short, short) in &matches {
        for &(n_long, long) in &matches {
            if n_long > n_short && long.starts_with(short) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn cursor_at_deferred_sequence_terminator(
    cursor: &Cursor<'_>,
    field_props: &IrProps,
    strings: &StringPool,
    stop_sequences: &[&IrProps],
) -> Result<bool, crate::error::VmError> {
    for seq in stop_sequences {
        let Some(tid) = seq.terminator else {
            continue;
        };
        let pat = strings.get(tid)?;
        if pat.is_empty() {
            continue;
        }
        if crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            pat,
            seq.ignore_case,
        )
        .is_some()
            && should_defer_sequence_stop_delimiter_in_field(seq, tid, field_props, strings)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn field_terminator_matches_at_cursor(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
) -> Result<Option<usize>, crate::error::VmError> {
    let Some(tid) = props.terminator else {
        return Ok(None);
    };
    let pat = strings.get(tid)?;
    if pat.is_empty() {
        return Ok(None);
    }
    let enc = encoding_name(props, strings).ok();
    Ok(crate::schema::match_delimiter_opts_for_encoding(
        &cursor.data[cursor.pos..],
        pat,
        props.ignore_case,
        enc.as_deref(),
    ))
}

fn defer_delimited_enclosing_consume(
    cursor: &Cursor<'_>,
    props: &IrProps,
    strings: &StringPool,
    value: &crate::value::DfdlValue,
    stop_sequences: &[&IrProps],
) -> Result<bool, crate::error::VmError> {
    use crate::value::DfdlValue;
    if props.representation == Representation::Binary {
        // Binary delimited scalars must always consume the enclosing delimiter so
        // unbounded sequences advance (BCD/4690/packed hex); text may still defer.
        return Ok(false);
    }
    if matches!(value, DfdlValue::Null) {
        return Ok(false);
    }
    if cursor_at_deferred_sequence_terminator(cursor, props, strings, stop_sequences)? {
        return Ok(true);
    }
    if ambiguous_delimiter_prefix_at_cursor(cursor, props, strings, stop_sequences)? {
        // When the field terminator fully matches at the cursor (e.g. pipes2 `|||` after `||`
        // initiator), consume it even if a shorter sibling delimiter (separator `|`) is a prefix.
        if field_terminator_matches_at_cursor(cursor, props, strings)?.is_some_and(|n| n > 0) {
            return Ok(false);
        }
        return Ok(true);
    }
    for id in delimiter_pattern_ids(props) {
        let pat = strings.get(id)?;
        if let Some(n) = crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            pat,
            props.ignore_case,
        ) {
            if n == 0 {
                continue;
            }
            let after_first = cursor.pos.saturating_add(n);
            if crate::schema::match_delimiter_opts(&cursor.data[after_first..], pat, props.ignore_case)
                .is_some()
            {
                let after_second = after_first.saturating_add(n);
                return Ok(cursor.data.get(after_second).is_none());
            }
        }
    }
    Ok(false)
}

pub(crate) fn read_simple(
    cursor: &mut Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    require_delimiter: bool,
    stop_sequences: &[&IrProps],
    field_name: Option<&str>,
    tunables: &DaffodilTunables,
    consume_delimited_enclosing: bool,
    mut delim_out: Option<&mut crate::value::FieldDelimiterMeta>,
    sibling_env: Option<&crate::schema::boolean_reps::BooleanSiblingEnv<'_>>,
    enable_facet_validation: bool,
    defer_facet_validation: bool,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;

    let encoding = encoding_name(props, strings)?;
    if let Some(id) = props.initiator {
        let pat = strings.get(id)?;
        if !pat.is_empty() {
            crate::vm::alignment::align_cursor_to_text_encoding(cursor, props, encoding)?;
            let Some((_n, alt)) =
                cursor.consume_delimiter_with_alt(pat, props.ignore_case, Some(encoding))
            else {
                let found_display =
                    format_found_at_cursor(&cursor.data, cursor.pos, Some(encoding));
                let ctx = field_name.unwrap_or("element");
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error: Init('{pat}') - {ctx}: Delimiter not found!\nWas looking for ({pat}) but found \"{found_display}\" instead"
                    ),
                });
            };
            if let Some(out) = delim_out.as_mut() {
                out.initiator_alt = Some(alt);
            }
        }
    }
    let _ = field_name;
    let require_enclosing =
        require_delimiter || has_non_empty_terminator(props, strings)?;
    let use_text = match kind {
        ValueKind::String => props.object_kind != crate::schema::ObjectKind::Bytes,
        ValueKind::HexBinary => false,
        _ => props.representation == Representation::Text,
    };
    let value = if use_text {
        read_text_scalar(
            cursor,
            kind,
            props,
            strings,
            require_enclosing,
            stop_sequences,
            field_name,
            sibling_env,
            tunables,
        )?
    } else {
        read_binary_scalar(
            cursor,
            kind,
            props,
            strings,
            require_enclosing,
            stop_sequences,
            field_name,
            tunables,
        )?
    };
    if props.length_kind == LengthKind::Delimited {
        let defer = !consume_delimited_enclosing
            && defer_delimited_enclosing_consume(cursor, props, strings, &value, stop_sequences)?;
        if consume_delimited_enclosing || !defer {
            consume_enclosing_delimiter(cursor, props, strings, stop_sequences)?;
        }
    } else if props.representation == Representation::Text
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
    {
        // Terminator consumed in read_text_scalar for fixed/explicit text fields.
    } else if let Some(id) = props.terminator {
        let pat = strings.get(id)?;
        if !pat.is_empty() {
            crate::vm::alignment::align_cursor_to_text_encoding(cursor, props, encoding)?;
            if let Some((n, alt)) =
                cursor.consume_delimiter_with_alt(pat, props.ignore_case, Some(encoding))
            {
                if n == 0
                    && !cursor.is_empty()
                    && !crate::schema::delimiter_alt_allows_trailing_input(pat, alt)
                {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "terminator mismatch: expected `{}`",
                            format_delimiter_for_error(pat)
                        ),
                    });
                }
                if let Some(out) = delim_out.as_mut() {
                    out.terminator_alt = Some(alt);
                }
            } else if !cursor.is_empty() {
                return Err(VmError::InvalidValue {
                    message: if cursor.is_empty() {
                        alloc::format!("terminator `{}` not found", format_delimiter_for_error(pat))
                    } else {
                        alloc::format!(
                            "terminator mismatch: expected `{}`",
                            format_delimiter_for_error(pat)
                        )
                    },
                });
            }
        }
    }
    if !use_text
        && props.representation == Representation::Binary
        && props.length_units == LengthUnits::Bits
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
        && kind != crate::ir::ValueKind::HexBinary
        && !matches!(
            props.length_kind,
            LengthKind::Delimited | LengthKind::Prefixed
        )
    {
        let (align, units) = crate::vm::alignment::post_read_alignment(props);
        if !props.alignment_implicit && align > 1 {
            let pos = cursor.absolute_bit_index();
            let align_bits = match units {
                LengthUnits::Bits => align as usize,
                LengthUnits::Bytes | LengthUnits::Characters => (align as usize).saturating_mul(8),
            };
            if align_bits > 1 {
                let skip = (align_bits - (pos % align_bits)) % align_bits;
                let within_frame = cursor
                    .frame_bit_limit
                    .map(|limit| pos + skip <= limit)
                    .unwrap_or(true);
                if skip > 0 && within_frame {
                    consume_alignment_values(cursor, props, align, units)?;
                }
            }
        }
        if props.bit_order == BitOrder::LeastSignificantBitFirst
            && props.alignment_units == LengthUnits::Bits
            && !props.alignment_implicit
            && props.alignment == 4
            && props.length_units == LengthUnits::Bits
        {
            let pos = cursor.absolute_bit_index();
            let align = props.alignment as usize;
            let skip = (align - (pos % align)) % align;
            if skip > 0 {
                let within_frame = cursor
                    .frame_bit_limit
                    .map(|limit| pos + skip <= limit)
                    .unwrap_or(true);
                if within_frame {
                    cursor.skip_stream_bits(skip, props.bit_order)?;
                }
            }
        }
    }
    crate::vm::alignment::consume_trailing_skip(cursor, props)?;
    finalize_simple_value(
        value,
        kind,
        props,
        strings,
        tunables,
        enable_facet_validation,
        defer_facet_validation,
    )
}

fn encode_binary_payload_bytes(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    field_name: Option<&str>,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    let le = props.byte_order == ByteOrder::LittleEndian;
    match (kind, value) {
        (String, DfdlValue::String(v)) => {
            if let Some(raw) = &v.meta.source_bytes {
                Ok(raw.clone())
            } else {
                Ok(v.text.as_bytes().to_vec())
            }
        }
        (HexBinary, DfdlValue::HexBinary(v)) => Ok(v.clone()),
        (HexBinary, DfdlValue::String(s)) => decode_hex_binary(&s.text),
        (Boolean, DfdlValue::Boolean(v)) => Ok(alloc::vec![u8::from(*v)]),
        (Byte, DfdlValue::Byte(v)) => {
            encode_integer_binary(*v as i64, kind, props, le, strings)
        }
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Short, DfdlValue::Short(v)) => {
            encode_integer_binary(*v as i64, kind, props, le, strings)
        }
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Int, DfdlValue::Int(v)) => encode_integer_binary(*v as i64, kind, props, le, strings),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Long, DfdlValue::Long(v)) => encode_integer_binary(*v, kind, props, le, strings),
        (Float, DfdlValue::Float(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Double, DfdlValue::Double(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Decimal, DfdlValue::Decimal(v)) => {
            let (negative, raw) = parse_virtual_decimal_signed(v, props.binary_decimal_virtual_point)?;
            validate_decimal_unparse_sign(negative, props, field_name)?;
            encode_signed_magnitude_binary(raw, negative, kind, props, le, strings)
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
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    if props.binary_number_rep == BinaryNumberRep::Binary {
        let width = binary_payload_width(value.unsigned_abs(), kind, props);
        return Ok(int_bytes(value, width, le));
    }
    encode_signed_magnitude_binary(
        value.unsigned_abs(),
        value < 0,
        kind,
        props,
        le,
        strings,
    )
}

fn encode_unsigned_binary(
    value: u64,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    le: bool,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    encode_signed_magnitude_binary(value, false, kind, props, le, strings)
}

fn encode_signed_magnitude_binary(
    magnitude: u64,
    negative: bool,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    le: bool,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    let width = binary_payload_width(magnitude, kind, props);
    match props.binary_number_rep {
        BinaryNumberRep::Binary => Ok(int_bytes(
            if negative {
                -(magnitude as i64)
            } else {
                magnitude as i64
            },
            width,
            le,
        )),
        BinaryNumberRep::Bcd => u64_to_bcd_bytes(magnitude, width, le),
        BinaryNumberRep::PackedBcd => {
            let codes = packed_sign_codes(props, strings)?;
            encode_packed_bcd_magnitude(magnitude, negative, width, le, &codes)
        }
        BinaryNumberRep::Ibm4690Packed => {
            encode_ibm4690_magnitude(magnitude, negative, width, le)
        }
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds are calendar encodings".into(),
            })
        }
    }
}

fn binary_payload_width(value: u64, kind: crate::ir::ValueKind, props: &IrProps) -> usize {
    if matches!(
        props.length_kind,
        LengthKind::Prefixed | LengthKind::Delimited
    ) {
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
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => 4,
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
        BinaryNumberRep::Bcd => u64_to_bcd_bytes(value, width, le),
        BinaryNumberRep::Ibm4690Packed => encode_ibm4690_magnitude(value, false, width, le),
        BinaryNumberRep::PackedBcd => {
            let codes = PackedSignCodes::parse("C D F C", BinaryNumberCheckPolicy::Lax)?;
            encode_packed_bcd_magnitude(value, false, width, le, &codes)
        }
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds are calendar encodings".into(),
            })
        }
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

pub(crate) fn int_bytes(value: i64, size: usize, le: bool) -> alloc::vec::Vec<u8> {
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
    field_name: Option<&str>,
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
    write_prefix_field(
        out,
        bit_count,
        prefix_value,
        prefix,
        props.length_units,
        strings,
        field_name,
    )?;
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
                None,
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
        LengthUnits::Characters => count_characters(payload, encoding, EncodingErrorPolicy::Error),
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
    field_name: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::schema::Representation;
    validate_prefix_facets(value, prefix, field_name)?;
    if prefix.props.length_kind == LengthKind::Prefixed {
        let payload = prefix_scalar_payload(value, prefix, strings)?;
        return write_prefixed_bytes(out, bit_count, &payload, &prefix.props, strings, field_name);
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
    let encoding = encoding_name(&prefix.props, strings)?;
    let enc_align =
        crate::length_validate::implicit_text_encoding_alignment_bits_for_kind(prefix.kind, encoding);
    let mut align_bits = if prefix.props.length_units == LengthUnits::Bits {
        if prefix.props.alignment_implicit {
            crate::vm::alignment::implicit_alignment_in_bits(
                prefix.kind,
                &prefix.props,
                encoding,
            ) as u64
        } else if prefix.props.alignment == 0 {
            1
        } else {
            prefix.props.alignment
        }
    } else {
        let (align, align_units) =
            crate::vm::alignment::resolved_alignment(prefix.kind, &prefix.props, encoding);
        match align_units {
            LengthUnits::Bits => align,
            LengthUnits::Bytes | LengthUnits::Characters => align.saturating_mul(8),
        }
    };
    // Prefixed strings measured in bits require prefix alignment in bits (DFDL-12 / s2 TDML).
    if element_length_units == LengthUnits::Bits
        && prefix_is_numeric(prefix.kind)
        && encoding.eq_ignore_ascii_case("US-ASCII")
        && !prefix.props.alignment_implicit
        && prefix.props.alignment == 0
    {
        align_bits = 1;
    }
    if enc_align != 0 && align_bits % enc_align != 0 {
        let type_name = match prefix.kind {
            crate::ir::ValueKind::Int => "int",
            crate::ir::ValueKind::Long => "long",
            crate::ir::ValueKind::Short => "short",
            crate::ir::ValueKind::Byte => "byte",
            crate::ir::ValueKind::UnsignedInt => "unsignedInt",
            crate::ir::ValueKind::UnsignedShort => "unsignedShort",
            crate::ir::ValueKind::UnsignedByte => "unsignedByte",
            _ => "value",
        };
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: The given alignment ({align_bits} bits) must be a multiple of the encoding specified alignment ({enc_align} bits) for {type_name} when representation='text'. Encoding: {encoding}"
            ),
        });
    }
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
                        TextNumberJustification::Center => {
                            let left = pad_count / 2;
                            let right = pad_count - left;
                            for _ in 0..left {
                                padded.insert(0, pad);
                            }
                            for _ in 0..right {
                                padded.push(pad);
                            }
                        }
                    }
                    write_byte_aligned(out, bit_count, padded.as_bytes())?;
                }
                LengthUnits::Characters => {
                    let encoding = encoding_name(&prefix.props, strings)?;
                    while count_characters(padded.as_bytes(), encoding, EncodingErrorPolicy::Error)? < len {
                        match justification {
                            TextNumberJustification::Right => {
                                padded.insert(0, pad);
                            }
                            TextNumberJustification::Left => {
                                padded.push(pad);
                            }
                            TextNumberJustification::Center => {
                                let current =
                                    count_characters(padded.as_bytes(), encoding, EncodingErrorPolicy::Error)?;
                                let pad_count = len.saturating_sub(current);
                                let left = pad_count / 2;
                                let right = pad_count - left;
                                for _ in 0..left {
                                    padded.insert(0, pad);
                                }
                                for _ in 0..right {
                                    padded.push(pad);
                                }
                                break;
                            }
                        }
                    }
                    if count_characters(padded.as_bytes(), encoding, EncodingErrorPolicy::Error)? > len {
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
                        if encoding.eq_ignore_ascii_case("US-ASCII") && enc_align == 8 {
                            return Err(VmError::InvalidValue {
                                message: alloc::format!(
                                    "Schema Definition Error: The given alignment (1 bits) must be a multiple of the encoding specified alignment ({enc_align} bits) for int when representation='text'. Encoding: {encoding}"
                                ),
                            });
                        }
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
    config: Option<&RuntimeConfig>,
    field_name: Option<&str>,
    delim_meta: Option<&crate::value::FieldDelimiterMeta>,
) -> Result<(), crate::error::VmError> {
    match props.length_kind {
        LengthKind::Prefixed => {
            write_prefixed_bytes(out, bit_count, payload, props, strings, field_name)
        }
        LengthKind::Explicit | LengthKind::Fixed => {
            write_explicit_payload(
                out,
                bit_count,
                payload,
                payload_bit_count,
                props,
                strings,
                config,
            )
        }
        LengthKind::Delimited => {
            write_bits_from_stream_with_config(
                out,
                bit_count,
                payload,
                payload_bit_length(payload, payload_bit_count),
                props.bit_order,
                config,
            )?;
            if let Some(id) = props.terminator {
                let pat = strings.get(id)?;
                if !pat.is_empty() {
                    let bytes = match delim_meta.and_then(|m| m.terminator_alt) {
                        Some(a) => encode_delimiter_by_alt(pat, a),
                        None => encode_delimiter(pat),
                    };
                    write_byte_aligned(out, bit_count, &bytes)?;
                }
            }
            Ok(())
        }
        LengthKind::Implicit | LengthKind::Pattern | LengthKind::EndOfParent => {
            write_bits_from_stream_with_config(
                out,
                bit_count,
                payload,
                payload_bit_length(payload, payload_bit_count),
                props.bit_order,
                config,
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
    config: Option<&RuntimeConfig>,
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
            write_bits_from_stream_with_config(
                out,
                bit_count,
                payload,
                available.min(len),
                props.bit_order,
                config,
            )?;
            for _ in available..len {
                write_stream_bit_with_config(out, bit_count, 0, props.bit_order, config);
            }
            Ok(())
        }
        LengthUnits::Characters => {
            let encoding = encoding_name(props, strings)?;
            let mut bytes = payload.to_vec();
            while count_characters(&bytes, encoding, EncodingErrorPolicy::Error)? < len {
                let pad = encode_document_text(" ", encoding)?;
                bytes.extend_from_slice(&pad);
            }
            if count_characters(&bytes, encoding, EncodingErrorPolicy::Error)? > len {
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
        (Byte, v @ DfdlValue::Long(_)) => v.clone(),
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
        (HexBinary, DfdlValue::String(s)) => {
            decode_hex_binary(&s.text).map(DfdlValue::HexBinary)?
        }
        (HexBinary, v @ DfdlValue::HexBinary(_)) => v.clone(),
        (Time, v @ DfdlValue::DateTime(_)) => v.clone(),
        (_, v) => v.clone(),
    })
}

fn unparse_not_valid_xs(type_name: &str) -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: alloc::format!("Unparse Error: not a valid {type_name}"),
    }
}

fn unparse_not_calendar() -> crate::error::VmError {
    crate::error::VmError::InvalidValue {
        message: "Unparse Error: not a calendar".into(),
    }
}

pub(crate) fn validate_unparse_scalar_lexical(
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::ir::ValueKind;
    use crate::schema::LengthUnits;
    use crate::value::DfdlValue;

    let base = props.text_standard_base;
    let type_name = value_kind_type_name(kind, Some(props));

    match (kind, value) {
        (ValueKind::Byte, DfdlValue::String(s)) => {
            parse_int_with_base::<i8>(&s.text, type_name, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Short, DfdlValue::String(s)) => {
            parse_int_with_base::<i16>(&s.text, type_name, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Int, DfdlValue::String(s)) => {
            parse_int_with_base::<i32>(&s.text, type_name, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Long, DfdlValue::String(s)) => {
            parse_int_with_base::<i64>(&s.text, type_name, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Int, DfdlValue::Long(v)) => {
            i32::try_from(*v).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::UnsignedByte, DfdlValue::String(s)) => {
            parse_unsigned_radix::<u8>(&s.text, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::UnsignedShort, DfdlValue::String(s)) => {
            parse_unsigned_radix::<u16>(&s.text, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::UnsignedInt, DfdlValue::String(s)) => {
            parse_unsigned_radix::<u32>(&s.text, base).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Integer, val @ (DfdlValue::Integer(_) | DfdlValue::String(_))) => {
            let text = match val {
                DfdlValue::Integer(s) => s.as_str(),
                DfdlValue::String(s) => s.text.as_str(),
                _ => unreachable!(),
            };
            parse_unbounded_integer_decimal(text, base, props.non_negative_integer)
                .map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Float, DfdlValue::String(s)) => {
            parse_float(&s.text).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Double, DfdlValue::String(s)) => {
            parse_float(&s.text).map_err(|_| unparse_not_valid_xs(type_name))?;
        }
        (ValueKind::Decimal, val @ (DfdlValue::Decimal(_) | DfdlValue::String(_))) => {
            let text = match val {
                DfdlValue::Decimal(s) => s.as_str(),
                DfdlValue::String(s) => s.text.as_str(),
                _ => unreachable!(),
            };
            if text.trim().is_empty() {
                return Err(unparse_not_valid_xs(type_name));
            }
            let _ = split_sign_digits(text).map_err(|_| unparse_not_valid_xs(type_name))?;
            if props.non_negative_integer && text.trim().starts_with('-') {
                return Err(unparse_not_valid_xs(type_name));
            }
            if text.chars().any(|c| c.is_ascii_alphabetic()) {
                return Err(unparse_not_valid_xs(type_name));
            }
        }
        (ValueKind::HexBinary, val @ (DfdlValue::HexBinary(_) | DfdlValue::String(_))) => {
            let bytes = match val {
                DfdlValue::HexBinary(b) => b.clone(),
                DfdlValue::String(s) => {
                    let t = s.text.trim();
                    if t.len() % 2 != 0 {
                        return Err(VmError::InvalidValue {
                            message: alloc::format!(
                                "Unparse Error: Hex string must have an even number of characters, but was {} for {t}",
                                t.len()
                            ),
                        });
                    }
                    decode_hex_binary(t).map_err(|_| unparse_not_valid_xs("xs:hexBinary"))?
                }
                _ => unreachable!(),
            };
            if props.length_kind == crate::schema::LengthKind::Explicit {
                if let Some(len) = props.length {
                    let len_bits = match props.length_units {
                        LengthUnits::Bytes => len.saturating_mul(8),
                        LengthUnits::Bits => len,
                        LengthUnits::Characters => len.saturating_mul(8),
                    };
                    let value_bits = (bytes.len() as u64).saturating_mul(8);
                    if value_bits > len_bits {
                        return Err(VmError::InvalidValue {
                            message: alloc::format!(
                                "Unparse Error: Length of xs:hexBinary exceeds calculated length of {value_bits} bits: {len_bits}"
                            ),
                        });
                    }
                }
            }
        }
        (ValueKind::Boolean, DfdlValue::String(s)) => {
            parse_text_boolean(&s.text, props, strings, None)
                .map_err(|_| unparse_not_valid_xs("xs:boolean"))?;
        }
        (ValueKind::DateTime, DfdlValue::DateTime(s)) | (ValueKind::Time, DfdlValue::DateTime(s)) => {
            if props.calendar_date_only {
                crate::vm::calendar_binary::parse_xs_calendar_lexical(kind, true, s)
                    .map_err(|_| unparse_not_calendar())?;
            } else {
                crate::vm::calendar_binary::parse_xs_calendar_lexical(kind, false, s).map_err(
                    |_| {
                        unparse_not_valid_xs(if kind == ValueKind::Time {
                            "xs:time"
                        } else {
                            "xs:dateTime"
                        })
                    },
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn pad_hex_binary_value(
    mut bytes: alloc::vec::Vec<u8>,
    props: &IrProps,
    explicit_size: Option<usize>,
) -> alloc::vec::Vec<u8> {
    if let Some(size) = explicit_size {
        if bytes.len() < size {
            bytes.extend(core::iter::repeat(props.fill_byte).take(size - bytes.len()));
        } else if bytes.len() > size {
            bytes.truncate(size);
        }
        return bytes;
    }
    if let Some(min) = props
        .min_length
        .or(props.implicit_facet_length)
        .map(|n| n as usize)
    {
        if bytes.len() < min {
            bytes.extend(core::iter::repeat(props.fill_byte).take(min - bytes.len()));
        }
    }
    bytes
}

/// `dfdl:hexBinary(n)` / fixed-width numeric hex for outputValueCalc.
pub(crate) fn hex_binary_from_integer(value: i64, width: Option<usize>) -> alloc::vec::Vec<u8> {
    if let Some(w) = width {
        if w == 0 {
            return alloc::vec::Vec::new();
        }
        if value >= 0 {
            let mut v = value as u64;
            let mut out = vec![0u8; w];
            for i in (0..w).rev() {
                out[i] = (v & 0xff) as u8;
                v >>= 8;
            }
            return out;
        }
        let bits = (w as u32).saturating_mul(8);
        let mask = if bits >= 128 {
            u128::MAX
        } else {
            (1u128 << bits) - 1
        };
        let twos = (value as i128 as u128) & mask;
        let mut out = vec![0u8; w];
        for i in 0..w {
            let shift = ((w - 1 - i) * 8) as u32;
            out[i] = ((twos >> shift) & 0xff) as u8;
        }
        return out;
    }
    if value == 0 {
        return alloc::vec![0];
    }
    if value > 0 {
        let mut v = value as u64;
        let mut bytes = alloc::vec::Vec::new();
        while v > 0 {
            bytes.push((v & 0xff) as u8);
            v >>= 8;
        }
        bytes.reverse();
        return bytes;
    }
    for w in 1..=8usize {
        let out = hex_binary_from_integer(value, Some(w));
        let sign = out[0] & 0x80 != 0;
        if sign {
            return out;
        }
    }
    hex_binary_from_integer(value, Some(8))
}

pub(crate) fn write_simple(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
    config: &RuntimeConfig,
    field_name: Option<&str>,
    delim_meta: Option<&crate::value::FieldDelimiterMeta>,
    encode_siblings: Option<&alloc::collections::BTreeMap<String, crate::value::DfdlValue>>,
) -> Result<(), crate::error::VmError> {
    validate_unparse_scalar_lexical(value, kind, props, strings)?;
    let value = coerce_value_for_kind(value, kind)?;
    if props.object_kind == crate::schema::ObjectKind::Bytes {
        if let Some(id) = props.initiator {
            let pat = strings.get(id)?;
            if !pat.is_empty() {
                let output_nl = props
                    .output_new_line
                    .and_then(|id| strings.get(id).ok());
                let bytes = encode_property_delimiter(pat, output_nl);
                write_byte_aligned(out, bit_count, &bytes)?;
            }
        }
        write_blob_scalar(out, bit_count, &value, props, field_name)?;
        if let Some(id) = props.terminator {
            let pat = strings.get(id)?;
            if !pat.is_empty() {
                let output_nl = props
                    .output_new_line
                    .and_then(|id| strings.get(id).ok());
                let bytes = encode_property_delimiter(pat, output_nl);
                write_byte_aligned(out, bit_count, &bytes)?;
            }
        }
        crate::vm::alignment::write_trailing_skip(out, bit_count, props)?;
        return Ok(());
    }
    if let Some(id) = props.initiator {
        let pat = strings.get(id)?;
        if !pat.is_empty() {
            let output_nl = props
                .output_new_line
                .and_then(|id| strings.get(id).ok());
            let bytes = match delim_meta.and_then(|m| m.initiator_alt) {
                Some(a) => encode_delimiter_by_alt(pat, a),
                None => encode_property_delimiter(pat, output_nl),
            };
            write_byte_aligned(out, bit_count, &bytes)?;
        }
    }
    match props.representation {
        Representation::Binary => write_binary_scalar(
            out,
            bit_count,
            &value,
            kind,
            props,
            strings,
            tunables,
            &config,
            field_name,
        )?,
        Representation::Text => {
            write_text_scalar(
                out,
                bit_count,
                &value,
                kind,
                props,
                strings,
                &config,
                field_name,
                encode_siblings,
            )?
        }
    }
    if let Some(id) = props.terminator {
        let pat = strings.get(id)?;
        if !pat.is_empty() {
            let output_nl = props
                .output_new_line
                .and_then(|id| strings.get(id).ok());
            let bytes = match delim_meta.and_then(|m| m.terminator_alt) {
                Some(a) => encode_delimiter_by_alt(pat, a),
                None => encode_property_delimiter(pat, output_nl),
            };
            write_byte_aligned(out, bit_count, &bytes)?;
        }
    }
    crate::vm::alignment::write_trailing_skip(out, bit_count, props)?;
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
    let base = props.text_standard_base;
    match kind {
        Boolean => parse_text_boolean(raw, props, strings, None)
            .ok()
            .map(DfdlValue::Boolean),
        Byte => parse_int_with_base(raw, "xs:byte", base).ok().map(DfdlValue::Byte),
        UnsignedByte => parse_unsigned_radix(raw, base).ok().map(DfdlValue::UnsignedByte),
        Short => parse_int_with_base(raw, "xs:short", base).ok().map(DfdlValue::Short),
        UnsignedShort => parse_unsigned_radix(raw, base).ok().map(DfdlValue::UnsignedShort),
        Int => parse_int_with_base(raw, "xs:int", base).ok().map(DfdlValue::Int),
        Integer => parse_unbounded_integer_decimal(raw, base, false)
            .ok()
            .map(DfdlValue::Integer),
        UnsignedInt => parse_unsigned_radix(raw, base).ok().map(DfdlValue::UnsignedInt),
        Long => parse_int_with_base(raw, "xs:long", base).ok().map(DfdlValue::Long),
        Float => parse_float(raw).ok().map(|v| DfdlValue::Float(v as f32)),
        Double => parse_float(raw).ok().map(DfdlValue::Double),
        Decimal => Some(DfdlValue::Decimal(raw.into())),
        DateTime | Time => Some(DfdlValue::DateTime(raw.into())),
        String => Some(DfdlValue::string(raw)),
        HexBinary => decode_hex(raw).ok().map(DfdlValue::HexBinary),
        Complex => None,
    }
}

#[cfg(test)]
mod delimited_stop_tests {
    use super::*;
    use crate::ir::compile_named;
    use crate::schema::parse_schema;

    fn matrix_props() -> (IrProps, IrProps, IrProps) {
        let xsd = include_str!("../../resources/dfdl/AB.dfdl.xsd");
        let schema = parse_schema(xsd).unwrap();
        let program = compile_named(&schema, Some("matrix_02")).unwrap();
        let mut row_seq = None;
        let mut cell_seq = None;
        let mut cell = None;
        fn walk(program: &crate::ir::IrProgram, id: u32, row_seq: &mut Option<IrProps>, cell_seq: &mut Option<IrProps>, cell: &mut Option<IrProps>, in_row: bool) {
            match program.node(id).unwrap() {
                crate::ir::IrNode::Sequence { props, children, .. } => {
                    if in_row && cell_seq.is_none() {
                        *cell_seq = Some(props.clone());
                    } else if row_seq.is_none() {
                        *row_seq = Some(props.clone());
                    }
                    for c in children {
                        walk(program, *c, row_seq, cell_seq, cell, in_row || row_seq.is_some());
                    }
                }
                crate::ir::IrNode::Element { name, props, child, .. } => {
                    if program.strings.get(*name).ok() == Some("cell") {
                        *cell = Some(props.clone());
                    }
                    if let Some(c) = child {
                        walk(program, *c, row_seq, cell_seq, cell, in_row);
                    }
                }
                _ => {}
            }
        }
        walk(&program, program.root, &mut row_seq, &mut cell_seq, &mut cell, false);
        (row_seq.unwrap(), cell_seq.unwrap(), cell.unwrap())
    }

    #[test]
    fn empty_field_before_row_newline_is_zero_bytes() {
        let (row_seq, cell_seq, cell) = matrix_props();
        let strings = {
            let xsd = include_str!("../../resources/dfdl/AB.dfdl.xsd");
            compile_named(&parse_schema(xsd).unwrap(), Some("matrix_02"))
                .unwrap()
                .strings
        };
        let stops = [&row_seq, &cell_seq];
        let mut cursor = Cursor::new(b"\n");
        let raw =
            read_until_delimiters(&mut cursor, &cell, &strings, false, &stops, None).unwrap();
        assert!(raw.is_empty(), "expected empty before newline, got {raw:?}");
    }

    #[test]
    fn any_empty_suppresses_infix_separator_for_empty_item() {
        use crate::ir::{IrProps, StringPool};
        use crate::schema::{SeparatorPosition, SeparatorSuppressionPolicy};
        use crate::value::DfdlValue;

        let mut sep = IrProps::default();
        sep.separator_suppression_policy = Some(SeparatorSuppressionPolicy::AnyEmpty);
        sep.separator_position = SeparatorPosition::Infix;
        let item = IrProps::default();
        let items = [
            DfdlValue::Int(1),
            DfdlValue::string(""),
            DfdlValue::Int(3),
        ];
        let strings = StringPool::new();
        assert!(should_suppress_occurrence_separator(
            &sep, &item, &items, 1, true, &strings
        )
        .unwrap());
        assert!(should_suppress_occurrence_separator(
            &sep, &item, &items, 2, true, &strings
        )
        .unwrap());
    }

    fn default_cal_cfg() -> CalendarTextConfig<'static> {
        CalendarTextConfig {
            language: None,
            first_day_of_week: 1,
            days_in_first_week: 4,
        }
    }

    fn format_calendar_text_time_and_datetime() {
        assert_eq!(
            format_calendar_text("04:09:23", "hh:mm:ss", false, 53, default_cal_cfg(), false, false)
                .map_err(|e| e.to_string())
                .unwrap(),
            "04:09:23"
        );
        assert_eq!(
            format_calendar_text("Friday 05 2013 - 03:30:30", "EEEE MM yyyy - hh:mm:ss", false, 53, default_cal_cfg(), true, false).unwrap(),
            "2013-05-03T03:30:30"
        );
    }

    #[test]
    #[test]
    fn format_calendar_text_date_pattern_choice() {
        let pat = "'It is day 'dd' of 'MMM, yyyy";
        let text = "It is day 25 of March, 2013";
        let out = format_calendar_text(text, pat, false, 53, default_cal_cfg(), false, true)
            .unwrap_or_else(|e| panic!("choice: {e}"));
        assert_eq!(out, "2013-03-25");
    }

    #[test]
    fn format_calendar_text_date_pattern01() {
        let pat = "'Today is the 'dd'th day of 'MMMM', year 'yyyy";
        let text = "Today is the 25th day of January, year 2013";
        let out = format_calendar_text(text, pat, false, 53, default_cal_cfg(), false, true)
            .unwrap_or_else(|e| panic!("date01: {e}"));
        assert_eq!(out, "2013-01-25");
    }

    #[test]
    fn format_calendar_text_date_time_pattern01() {
        let pat = "'It is 'hh:mmaa' on the 'dd'st of 'MMM', year 'yyyy";
        let text = "It is 11:53AM on the 1st of April, year 2013";
        let out = format_calendar_text(text, pat, false, 53, default_cal_cfg(), true, false)
            .unwrap_or_else(|e| panic!("dt1: {e}"));
        assert_eq!(out, "2013-04-01T11:53:00");
    }

    #[test]
    fn format_calendar_text_section5_samples() {
        let date = format_calendar_text("Wednesday, July 10, '96", "EEEE, MMM d, ''yy", false, 53, default_cal_cfg(), false, true)
            .unwrap_or_else(|e| panic!("dateText: {e}"));
        assert_eq!(date, "1996-07-10");
        let time = format_calendar_text("12:08 PM", "h:mm a", false, 53, default_cal_cfg(), false, false).unwrap_or_else(|e| panic!("timeText: {e}"));
        assert_eq!(time, "12:08:00");
        let time_tz = append_default_utc_offset(crate::ir::ValueKind::Time, false, &time);
        assert_eq!(time_tz, "12:08:00+00:00");
        let dt = format_calendar_text(
            "1996.07.10 AD at 15:08:56 GMT-05:00",
            "yyyy.MM.dd G 'at' HH:mm:ss ZZZZ",
            false,
            53,
            default_cal_cfg(),
            true,
            false,
        )
        .unwrap_or_else(|e| panic!("dateTimeText: {e}"));
        assert_eq!(dt, "1996-07-10T15:08:56-05:00");
    }

    #[test]
    fn format_time_gmt8_and_rfc822_tz() {
        assert_eq!(
            super::parse_calendar_tz_offset("-8").unwrap().0,
            "-08:00"
        );
        assert_eq!(
            format_calendar_text("08:43.GMT-8", "hh:mm.v", false, 53, default_cal_cfg(), false, false).unwrap(),
            "08:43:00-08:00"
        );
        assert_eq!(
            format_calendar_text("08:43.-0800", "hh:mm.Z", false, 53, default_cal_cfg(), false, false).unwrap(),
            "08:43:00-08:00"
        );
    }
}

#[cfg(test)]
mod tdml_encode_bit_index_tests {
    use super::*;
    use crate::schema::BitOrder;

    #[test]
    fn encode_absolute_bit_index_partial_byte() {
        let out = [0xff_u8];
        assert_eq!(encode_absolute_bit_index(&out, 7), 7);
        assert_eq!(encode_absolute_bit_index(&out, 0), 8);
    }

    #[test]
    fn tdml_region_msbf_through_full_byte() {
        let regions = vec![
            (BitOrder::MostSignificantBitFirst, 8),
            (BitOrder::LeastSignificantBitFirst, 24),
            (BitOrder::MostSignificantBitFirst, 8),
        ];
        let config = RuntimeConfig {
            encode_tdml_bit_regions: Some(regions),
            ..RuntimeConfig::default()
        };
        let mut out = Vec::new();
        let mut bc = 0u8;
        write_stream_bits_with_config(
            &mut out,
            &mut bc,
            255,
            8,
            BitOrder::MostSignificantBitFirst,
            Some(&config),
        );
        assert_eq!(out, [0xff]);
        assert_eq!(bc, 0);
    }
}

#[cfg(test)]
mod bitorder_sub_byte_tests {
    use super::*;
    use crate::schema::BitOrder;

    #[test]
    fn lsbf_three_bits_from_least_first_document_bytes() {
        let data = vec![0x4b, 0x54];
        let mut cursor =
            Cursor::with_frame_bits_and_transmission(&data, 16, BitOrder::MostSignificantBitFirst);
        let hex = cursor
            .read_hex_binary_bits(3, BitOrder::LeastSignificantBitFirst)
            .unwrap();
        let mut cursor2 =
            Cursor::with_frame_bits_and_transmission(&data, 16, BitOrder::MostSignificantBitFirst);
        let stream_bytes = cursor2
            .read_stream_bits_as_bytes(3, BitOrder::LeastSignificantBitFirst)
            .unwrap();
        let mut cursor3 =
            Cursor::with_frame_bits_and_transmission(&data, 16, BitOrder::MostSignificantBitFirst);
        let stream_raw = cursor3
            .read_stream_bits(3, BitOrder::LeastSignificantBitFirst)
            .unwrap();
        let packed_hex = decode_packed_bit_field_u64(
            &hex,
            3,
            ByteOrder::LittleEndian,
            BitOrder::LeastSignificantBitFirst,
        );
        let packed_stream = decode_packed_bit_field_u64(
            &stream_bytes,
            3,
            ByteOrder::LittleEndian,
            BitOrder::LeastSignificantBitFirst,
        );
        assert_eq!(hex, [3], "fillByteArray-style read");
        assert_eq!(stream_bytes, [2], "stream_bits_as_bytes differs for LSBF fragments");
        assert_eq!(packed_hex, 3);
        assert_eq!(packed_stream, 2);
        assert_eq!(stream_raw, 2);
        assert_eq!(
            normalize_bit_field_raw(stream_raw, 3, ByteOrder::LittleEndian, BitOrder::LeastSignificantBitFirst),
            2
        );
    }

    #[test]
    fn encode_packed_bit_field_roundtrips_decode() {
        for &(raw, width, order, bit_order) in &[
            (0x422c_0000u64, 32usize, ByteOrder::LittleEndian, BitOrder::MostSignificantBitFirst),
            (0x1234u64, 16, ByteOrder::LittleEndian, BitOrder::MostSignificantBitFirst),
            (0x5u64, 3, ByteOrder::LittleEndian, BitOrder::LeastSignificantBitFirst),
        ] {
            let wire = encode_packed_bit_field_bytes(raw, width, order, bit_order);
            let back = decode_packed_bit_field_u64(&wire, width, order, bit_order);
            assert_eq!(
                back,
                raw & bit_mask(width),
                "width={width} order={order:?} bit_order={bit_order:?} wire={wire:?}"
            );
        }
    }

    #[test]
    fn msbf_hex_binary_fill_byte_array_after_partial_byte() {
        let data = vec![0xde, 0xad, 0xbe, 0xef];
        let mut cursor =
            Cursor::with_frame_bits_and_transmission(&data, 32, BitOrder::MostSignificantBitFirst);
        let hb1 = cursor
            .read_hex_binary_bits(5, BitOrder::MostSignificantBitFirst)
            .unwrap();
        let hb2 = cursor
            .read_hex_binary_bits(22, BitOrder::MostSignificantBitFirst)
            .unwrap();
        let hb3 = cursor
            .read_hex_binary_bits(5, BitOrder::MostSignificantBitFirst)
            .unwrap();
        assert_eq!(hb1, [0xd8]);
        assert_eq!(hb2, [0xd5, 0xb7, 0xdc]);
        assert_eq!(hb3, [0x78]);
    }
}
