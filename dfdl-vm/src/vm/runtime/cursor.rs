use crate::schema::BitOrder;
use crate::vm::encoding::HexCharsetOrder;
use crate::vm::runtime::insufficient_data_bits_error;
use alloc::vec::Vec;

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
            return self.tdml_bit_order_at(self.absolute_bit_index());
        }
        schema_order
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
                    value = (value << 1) | self.read_stream_bit_with_hint(n, start, field_order)?;
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
                        array[out_bit / 8] |= bit << (out_bit % 8);
                    }
                    BitOrder::MostSignificantBitFirst => {
                        array[out_bit / 8] |= bit << (7 - (out_bit % 8));
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

    pub(crate) fn read_stream_bit(
        &mut self,
        field_bit_order: BitOrder,
    ) -> Result<u64, crate::error::VmError> {
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
        let tx_order = self.effective_field_bit_order(field_bit_order);
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
        if std::env::var("DEBUG_LION").is_ok() {
            let rem = String::from_utf8_lossy(&self.data[self.pos..]);
            std::eprintln!(
                "CONSUME_DELIM pat={pattern:?} pos={} matched_n={n} rem={rem:?}",
                self.pos
            );
        }
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
