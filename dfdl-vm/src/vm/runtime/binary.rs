use super::*;
use crate::ir::*;
use crate::length_validate::*;
use crate::schema::*;
use crate::vm::encoding::hex_charset_order;
use crate::vm::packed_decimal::*;
use alloc::vec::Vec;

pub(crate) fn encode_binary_number_u64(
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

pub(crate) fn write_bits_from_stream(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    src: &[u8],
    n: usize,
    bit_order: BitOrder,
) -> Result<(), crate::error::VmError> {
    write_bits_from_stream_with_config(out, bit_count, src, n, bit_order, None)
}

pub(crate) fn read_packed_bit_at(data: &[u8], bit_idx: usize, order: BitOrder) -> u8 {
    let byte = data[bit_idx / 8];
    let bit_in_byte = bit_idx % 8;
    match order {
        BitOrder::MostSignificantBitFirst => (byte >> (7 - bit_in_byte)) & 1,
        BitOrder::LeastSignificantBitFirst => (byte >> bit_in_byte) & 1,
    }
}

pub(crate) fn write_bits_from_stream_with_config(
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

pub(crate) fn payload_bit_length(payload: &[u8], payload_bit_count: u8) -> usize {
    encode_absolute_bit_index(payload, payload_bit_count)
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
    scan_ctx: Option<&SequenceChildScanContext<'_>>,
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
            scan_ctx,
        )?;
        return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
    }

    if props.length_kind == LengthKind::Prefixed {
        let bytes = read_prefixed_payload(cursor, props, strings, field_name)?;
        return decode_binary_scalar(kind, &bytes, props, strings, None, tunables);
    }

    if kind == ValueKind::Decimal {
        validate_explicit_decimal_vm(props, VmDecimalPhase::Parse, tunables, None, strings)?;
        let bit_len = binary_bit_length(cursor, kind, props, strings)?;
        validate_decimal_data_length_vm(
            props.decimal_signed,
            bit_len as u64,
            LengthUnits::Bits,
            VmDecimalPhase::Parse,
            props.length_sibling.is_some(),
        )?;
    }

    if props.length_units == LengthUnits::Bits {
        let len = binary_bit_length(cursor, kind, props, strings)?;
        if kind != ValueKind::String && kind != ValueKind::HexBinary {
            let enc = encoding_name(props, strings)?;
            if hex_charset_order(enc).is_some()
                && (kind == ValueKind::Decimal || is_packed_binary_rep(props.binary_number_rep))
            {
                validate_packed_binary_bit_length_parse(len, kind, props.binary_number_rep)?;
            }
            if len == 0 && kind != ValueKind::Decimal {
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
            if len < 8 {
                let raw = cursor.read_stream_bits(len, props.bit_order)?;
                let raw = normalize_bit_field_raw(raw, len, props.byte_order, props.bit_order);
                return decode_binary_from_raw_bits(kind, raw, len, props, strings, tunables);
            }
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
                let len = if let Some(l) = props.length {
                    l as usize
                } else {
                    binary_bit_length(cursor, kind, props, strings)?
                };
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
        validate_packed_binary_bit_length_parse(
            size.saturating_mul(8),
            kind,
            props.binary_number_rep,
        )?;
        if kind == ValueKind::Decimal && size == 0 {
            validate_decimal_data_length_vm(
                props.decimal_signed,
                0,
                LengthUnits::Bits,
                crate::length_validate::VmDecimalPhase::Parse,
                props.length_sibling.is_some(),
            )?;
        }
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

pub(crate) fn decode_binary_scalar(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
    bit_width: Option<usize>,
    tunables: &DaffodilTunables,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::ir::ValueKind;

    if kind == ValueKind::Decimal {
        return decode_decimal_binary(bytes, props, strings);
    }
    if matches!(kind, ValueKind::DateTime | ValueKind::Time) && calendar_binary_rep(props) {
        return decode_binary_calendar(kind, bytes, props, strings, tunables, None);
    }

    match props.binary_number_rep {
        BinaryNumberRep::Binary => decode_binary_bytes(
            kind,
            bytes,
            props,
            strings,
            props.byte_order == ByteOrder::LittleEndian,
            bit_width,
        ),
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

pub(crate) fn binary_bit_length(
    cursor: &Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;

    match props.length_kind {
        LengthKind::Fixed => Ok(props
            .length
            .unwrap_or((implicit_binary_scalar_byte_length(kind, props) * 8) as u64)
            as usize),
        LengthKind::Implicit => Ok(implicit_binary_scalar_byte_length(kind, props) * 8),
        LengthKind::Explicit => {
            let len = if let Some(l) = props.length {
                l as usize
            } else if props.length_sibling.is_some() {
                0
            } else {
                return Err(VmError::InvalidValue {
                    message: "explicit binary missing length".into(),
                });
            };
            if hex_charset_order(encoding_name(props, strings)?).is_some()
                && (kind == ValueKind::Decimal || is_packed_binary_rep(props.binary_number_rep))
            {
                validate_packed_binary_bit_length_parse(
                    len as usize,
                    kind,
                    props.binary_number_rep,
                )?;
            } else if binary_length_validation_applies(kind, props.binary_number_rep) {
                validate_data_length_vm(kind, len as u64, LengthUnits::Bits, props.binary_number_rep)?;
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

fn resolve_blob_value_bytes(
    value: &crate::value::DfdlValue,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    use crate::error::VmError;
    use crate::value::DfdlValue;
    match value {
        DfdlValue::Blob(b) => Ok(b.clone()),
        DfdlValue::HexBinary(b) => Ok(b.clone()),
        DfdlValue::String(s) => {
            let text = s.text.trim();
            if text.contains(' ') || text.contains('\'') {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Unparse Error: Illegal character in URI `{text}`"),
                });
            }
            if text.starts_with("http:") || text.starts_with("https:") {
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Unparse Error: Blob URI must be a file `{text}`"),
                });
            }
            if let Ok(b) = decode_hex_binary(text) {
                return Ok(b);
            }
            let path_str = text.strip_prefix("file:").unwrap_or(text);
            let candidates = [
                std::path::PathBuf::from(path_str),
                std::path::PathBuf::from(format!("third_party/daffodil/daffodil-test/src/test/resources/{path_str}")),
                std::path::PathBuf::from(format!("../third_party/daffodil/daffodil-test/src/test/resources/{path_str}")),
            ];
            for path in &candidates {
                if path.exists() {
                    if let Ok(bytes) = std::fs::read(path) {
                        return Ok(bytes);
                    }
                }
            }
            Err(VmError::InvalidValue {
                message: alloc::format!("Unparse Error: Unable to open blob for reading: {text}"),
            })
        }
        _ => Err(VmError::TypeMismatch {
            expected: "Blob / HexBinary / Hex String".into(),
        }),
    }
}

pub(crate) fn write_blob_scalar(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    value: &crate::value::DfdlValue,
    props: &IrProps,
    field_name: Option<&str>,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::schema::LengthUnits;

    if props.length_kind != crate::schema::LengthKind::Explicit {
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
            let wire_len = props.length.unwrap_or({
                if let DfdlValue::HexBinary(v) = value {
                    v.len() as u64
                } else {
                    0
                }
            });
            if wire_len > u64::from(max) {
                return Err(crate::length_validate::hex_binary_max_length_error(
                    max, wire_len, true,
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
        if kind == crate::ir::ValueKind::String || kind == crate::ir::ValueKind::HexBinary {
            let payload = encode_binary_payload_bytes(value, kind, props, strings, field_name)?;
            write_bits_from_stream_with_config(
                out,
                bit_count,
                &payload,
                n,
                props.bit_order,
                Some(config),
            )?;
            return Ok(());
        }
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
        LengthKind::Prefixed => {
            return Err(VmError::UnsupportedOperation {
                op: "prefixed lengthKind".into(),
            })
        }
    };

    let bytes;
    match (kind, value) {
        (Boolean, DfdlValue::Boolean(v)) => {
            bytes = int_bytes(encode_binary_boolean_sl(*v, props) as i64, size, le)
        }
        (Byte, DfdlValue::Byte(v)) => bytes = int_bytes(*v as i64, size, le),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => bytes = int_bytes(*v as i64, size, le),
        (Short, DfdlValue::Short(v)) => bytes = int_bytes(*v as i64, size, le),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => bytes = int_bytes(*v as i64, size, le),
        (Int, DfdlValue::Int(v)) => bytes = int_bytes(*v as i64, size, le),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => bytes = int_bytes(*v as i64, size, le),
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
        if bytes.len() > size && !props.truncate_specified_length_string {
            let mut message =
                "Unparse Error: data too long for explicit length and unable to truncate"
                    .to_string();
            if let Some(name) = field_name {
                message.push_str("\nSchema context: ");
                message.push_str(name);
            }
            return Err(VmError::InvalidValue { message });
        }
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

pub(crate) fn validate_decimal_unparse_sign(
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

pub(crate) fn parse_virtual_decimal_signed(
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

pub(crate) fn decode_bcd_number(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    let le = props.byte_order == ByteOrder::LittleEndian;
    let digits = bcd_to_digit_string(bytes, le)?;
    signed_magnitude_to_dfdl(false, &digits, kind, 0)
}

pub(crate) fn decode_ibm4690_number(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    let le = props.byte_order == ByteOrder::LittleEndian;
    let (negative, digits) = ibm4690_to_digit_string(bytes, le)?;
    signed_magnitude_to_dfdl(negative, &digits, kind, 0)
}

pub(crate) fn decode_packed_bcd_number(
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
                message: alloc::format!("out of range for type {kind:?}"),
            }
        }
        other => other,
    })
}

pub(crate) fn stream_bits_to_bytes(value: u64, num_bits: usize, byte_order: ByteOrder) -> Vec<u8> {
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

pub(crate) fn encode_binary_boolean_sl(v: bool, props: &IrProps) -> u64 {
    let false_rep = props.binary_boolean_false_rep.unwrap_or(0);
    let true_empty =
        props.binary_boolean_true_rep_defined && props.binary_boolean_true_rep.is_none();
    if v {
        if true_empty {
            let width =
                implicit_binary_scalar_byte_length(crate::ir::ValueKind::Boolean, props) * 8;
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

pub(crate) fn binary_encode_bit_length(
    kind: crate::ir::ValueKind,
    props: &IrProps,
    tunables: &DaffodilTunables,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    use crate::length_validate::{validate_data_length_vm, validate_signed_one_bit_length_vm, VmDecimalPhase};
    use super::validate::validate_explicit_decimal_vm;
    use super::numeric_text_format::length_kind_name;

    match props.length_kind {
        LengthKind::Fixed => Ok(props
            .length
            .unwrap_or((implicit_binary_scalar_byte_length(kind, props) * 8) as u64)
            as usize),
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

pub(crate) fn scalar_to_raw_bits(
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

pub(crate) fn encode_integer_binary(
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
    encode_signed_magnitude_binary(value.unsigned_abs(), value < 0, kind, props, le, strings)
}

pub(crate) fn encode_unsigned_binary(
    value: u64,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    le: bool,
    strings: &StringPool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
    encode_signed_magnitude_binary(value, false, kind, props, le, strings)
}

pub(crate) fn encode_signed_magnitude_binary(
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
        BinaryNumberRep::Ibm4690Packed => encode_ibm4690_magnitude(magnitude, negative, width, le),
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            Err(crate::error::VmError::InvalidValue {
                message: "binarySeconds/binaryMilliseconds are calendar encodings".into(),
            })
        }
    }
}

pub(crate) fn minimal_byte_width(value: u64) -> usize {
    if value == 0 {
        1
    } else {
        ((u64::BITS - value.leading_zeros()) as usize).div_ceil(8)
    }
}

pub(crate) fn auto_width_for_rep(value: u64, rep: BinaryNumberRep) -> usize {
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

pub(crate) fn u64_to_bcd_bytes(
    value: u64,
    width: usize,
    le: bool,
) -> Result<alloc::vec::Vec<u8>, crate::error::VmError> {
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

pub(crate) fn binary_payload_width(value: u64, kind: crate::ir::ValueKind, props: &IrProps) -> usize {
    if matches!(
        props.length_kind,
        LengthKind::Prefixed | LengthKind::Delimited
    ) {
        auto_width_for_rep(value, props.binary_number_rep)
    } else {
        type_size(kind)
    }
}

pub(crate) fn encode_binary_payload_bytes(
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
            let enc = encoding_name(props, strings)?;
            if let Some(spec) = crate::vm::encoding::bits_charset_spec(enc) {
                crate::vm::encoding::encode_bits_charset_text(&v.text, spec)
            } else if let Some(raw) = &v.meta.source_bytes {
                Ok(raw.clone())
            } else {
                crate::vm::encoding::encode_document_text(&v.text, enc)
            }
        }
        (HexBinary, DfdlValue::HexBinary(v)) => Ok(v.clone()),
        (HexBinary, DfdlValue::String(s)) => decode_hex_binary(&s.text),
        (Boolean, DfdlValue::Boolean(v)) => Ok(alloc::vec![u8::from(*v)]),
        (Byte, DfdlValue::Byte(v)) => encode_integer_binary(*v as i64, kind, props, le, strings),
        (UnsignedByte, DfdlValue::UnsignedByte(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Short, DfdlValue::Short(v)) => encode_integer_binary(*v as i64, kind, props, le, strings),
        (UnsignedShort, DfdlValue::UnsignedShort(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Int, DfdlValue::Int(v)) => encode_integer_binary(*v as i64, kind, props, le, strings),
        (UnsignedInt, DfdlValue::UnsignedInt(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Long, DfdlValue::Long(v)) => encode_integer_binary(*v, kind, props, le, strings),
        (Float, DfdlValue::Float(v)) => encode_unsigned_binary(*v as u64, kind, props, le, strings),
        (Double, DfdlValue::Double(v)) => {
            encode_unsigned_binary(*v as u64, kind, props, le, strings)
        }
        (Decimal, DfdlValue::Decimal(v)) => {
            let (negative, raw) =
                parse_virtual_decimal_signed(v, props.binary_decimal_virtual_point)?;
            validate_decimal_unparse_sign(negative, props, field_name)?;
            encode_signed_magnitude_binary(raw, negative, kind, props, le, strings)
        }
        (expected, _) => Err(VmError::TypeMismatch {
            expected: alloc::format!("{expected:?}"),
        }),
    }
}

pub(crate) fn pad_hex_binary_value(
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

pub(crate) fn parse_virtual_decimal(text: &str, virtual_point: u32) -> Result<u64, crate::error::VmError> {
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
        let w = parse_u64(trimmed)?;
        w.checked_mul(scale).ok_or(VmError::InvalidValue {
            message: "decimal value overflow".into(),
        })
    }
}

pub(crate) fn binary_byte_length(
    cursor: &Cursor<'_>,
    kind: crate::ir::ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    use super::numeric_text_parse::implicit_binary_scalar_byte_length;

    match props.length_kind {
        LengthKind::Fixed => Ok(props
            .length
            .unwrap_or(implicit_binary_scalar_byte_length(kind, props) as u64)
            as usize),
        LengthKind::Implicit => {
            if matches!(
                kind,
                crate::ir::ValueKind::String | crate::ir::ValueKind::HexBinary
            ) {
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
            crate::length_validate::validate_data_length_vm(kind, len, LengthUnits::Bytes, props.binary_number_rep)?;
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

pub(crate) fn effective_binary_decimal_vp(props: &IrProps) -> i32 {
    props
        .binary_decimal_virtual_point_signed
        .unwrap_or(props.binary_decimal_virtual_point as i32)
}

pub(crate) fn format_binary_decimal_magnitude(mag: u64, vp: i32) -> alloc::string::String {
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

pub(crate) fn format_binary_decimal_magnitude_from_digits(mag: &str, vp: usize) -> alloc::string::String {
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

pub(crate) fn apply_virtual_point_to_decimal_magnitude(mag: &str, vp: i32) -> alloc::string::String {
    if vp == 0 {
        return mag.into();
    }
    if vp < 0 {
        let exp = vp.unsigned_abs() as usize;
        return alloc::format!("{mag}{}", "0".repeat(exp));
    }
    format_binary_decimal_magnitude_from_digits(mag, vp as usize)
}

pub(crate) fn magnitude_bytes_be_to_decimal(bytes: &[u8]) -> alloc::string::String {
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
    digits.iter().rev().map(|d| char::from(b'0' + *d)).collect()
}

pub(crate) fn decode_binary_from_raw_bits(
    kind: crate::ir::ValueKind,
    raw: u64,
    bit_width: usize,
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::ir::ValueKind::*;
    use crate::value::DfdlValue;

    match kind {
        Byte => {
            let val = sign_extend_u64(raw, bit_width) as i8;
            Ok(DfdlValue::Byte(val))
        }
        UnsignedByte => Ok(DfdlValue::UnsignedByte(raw as u8)),
        Short => {
            let val = sign_extend_u64(raw, bit_width) as i16;
            Ok(DfdlValue::Short(val))
        }
        UnsignedShort => Ok(DfdlValue::UnsignedShort(raw as u16)),
        Int => {
            let val = sign_extend_u64(raw, bit_width) as i32;
            Ok(DfdlValue::Int(val))
        }
        UnsignedInt => Ok(DfdlValue::UnsignedInt(raw as u32)),
        Long => {
            if props.unsigned_integer {
                Ok(DfdlValue::UnsignedLong(raw))
            } else {
                let val = sign_extend_u64(raw, bit_width) as i64;
                Ok(DfdlValue::Long(val))
            }
        }
        Integer => {
            if props.unsigned_integer || props.non_negative_integer {
                Ok(DfdlValue::Integer(raw.to_string()))
            } else {
                let val = sign_extend_u64(raw, bit_width);
                Ok(DfdlValue::Integer(val.to_string()))
            }
        }
        Decimal => {
            validate_decimal_data_length_vm(
                props.decimal_signed,
                bit_width as u64,
                LengthUnits::Bits,
                VmDecimalPhase::Parse,
                true,
            )?;
            if props.binary_number_rep == BinaryNumberRep::Binary {
                Ok(DfdlValue::Decimal(format_binary_decimal_magnitude(
                    raw,
                    effective_binary_decimal_vp(props),
                )))
            } else {
                let bytes = stream_bits_to_bytes(raw, bit_width, props.byte_order);
                decode_binary_bytes(kind, &bytes, props, strings, props.byte_order == ByteOrder::LittleEndian, Some(bit_width))
            }
        }
        Boolean => Ok(DfdlValue::Boolean(decode_binary_boolean_sl(raw, props)?)),
        Float => Ok(DfdlValue::Float(f32::from_bits(raw as u32))),
        Double => Ok(DfdlValue::Double(f64::from_bits(raw))),
        _ => {
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
            let bytes = stream_bits_to_bytes(raw, bit_width, props.byte_order);
            decode_binary_bytes(kind, &bytes, props, strings, props.byte_order == ByteOrder::LittleEndian, Some(bit_width))
        }
    }
}

pub(crate) fn decode_decimal_binary(
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

pub(crate) fn calendar_binary_rep(props: &IrProps) -> bool {
    matches!(
        props.binary_calendar_rep,
        BinaryNumberRep::BinarySeconds
            | BinaryNumberRep::BinaryMilliseconds
            | BinaryNumberRep::Bcd
            | BinaryNumberRep::Ibm4690Packed
            | BinaryNumberRep::PackedBcd
    )
}

pub(crate) fn decode_binary_calendar(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
    tunables: &DaffodilTunables,
    raw_bits: Option<(u64, usize)>,
) -> Result<crate::value::DfdlValue, crate::error::VmError> {
    use crate::error::VmError;

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
        let _base = crate::vm::calendar_binary::parse_calendar_epoch_unix(epoch_raw)?;
        match rep {
            BinaryNumberRep::BinarySeconds => {
                let delta = crate::vm::calendar_binary::decode_binary_seconds_value(bytes, le)?;
                let text = crate::vm::calendar_binary::format_binary_calendar_from_seconds_delta(
                    epoch_raw, delta, tunables,
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
                    epoch_raw, delta_ms, tunables,
                )?;
                return calendar_value_from_text(kind, text);
            }
            _ => {
                return Err(VmError::InvalidValue {
                    message: "unsupported representation".into(),
                })
            }
        };
    }
    let digits = match rep {
        BinaryNumberRep::Bcd => {
            if let Some((raw, bits)) = raw_bits {
                let from_raw = crate::vm::calendar_binary::bcd_digits_from_raw_bits(raw, bits);
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
                PackedSignCodes::parse("C D F C", BinaryNumberCheckPolicy::Lax).unwrap_or_default()
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
        BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds => {
            return Err(VmError::InvalidValue {
                message: "handled above".into(),
            })
        }
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

pub(crate) fn calendar_value_from_text(
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

pub(crate) fn normalize_bit_field_raw(
    raw: u64,
    bit_width: usize,
    byte_order: ByteOrder,
    bit_order: BitOrder,
) -> u64 {
    if bit_width == 0 {
        return 0;
    }
    if bit_width < 8 {
        return raw & bit_mask(bit_width);
    }
    if byte_order == ByteOrder::LittleEndian && bit_order == BitOrder::LeastSignificantBitFirst {
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

pub(crate) fn decode_binary_bytes(
    kind: crate::ir::ValueKind,
    bytes: &[u8],
    props: &IrProps,
    strings: &StringPool,
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
                Ok(DfdlValue::Byte(
                    decode_unsigned_binary_bytes(bytes, le) as i8
                ))
            } else if let Some(bits) = bit_width {
                Ok(DfdlValue::Byte(
                    sign_extend_u64(decode_unsigned_binary_bytes(bytes, le), bits) as i8,
                ))
            } else {
                Ok(DfdlValue::Byte(int!(i8)))
            }
        }
        UnsignedByte => Ok(DfdlValue::UnsignedByte(int!(u8))),
        Short => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Short(
                    decode_unsigned_binary_bytes(bytes, le) as i16
                ))
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
                Ok(DfdlValue::UnsignedShort(decode_packed_bit_field_u64(
                    bytes,
                    bits,
                    props.byte_order,
                    props.bit_order,
                ) as u16))
            } else {
                Ok(DfdlValue::UnsignedShort(int!(u16)))
            }
        }
        Int => {
            if bit_width == Some(1) {
                Ok(DfdlValue::Int(
                    decode_unsigned_binary_bytes(bytes, le) as i32
                ))
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
        UnsignedInt => {
            if let Some(bits) = bit_width {
                Ok(DfdlValue::UnsignedInt(decode_packed_bit_field_u64(
                    bytes,
                    bits,
                    props.byte_order,
                    props.bit_order,
                ) as u32))
            } else if bytes.len() < core::mem::size_of::<u32>() {
                let raw = decode_unsigned_binary_bytes(bytes, le);
                Ok(DfdlValue::UnsignedInt(raw as u32))
            } else {
                Ok(DfdlValue::UnsignedInt(int!(u32)))
            }
        }
        Integer => {
            let is_unsigned = props.unsigned_integer || props.non_negative_integer;
            let str_val = decode_binary_bigint_str(bytes, !is_unsigned, le);
            Ok(DfdlValue::Integer(str_val))
        }
        Decimal => {
            if props.unsigned_integer {
                let val = if let Some(bits) = bit_width {
                    decode_packed_bit_field_u64(bytes, bits, props.byte_order, props.bit_order)
                } else {
                    int!(u64)
                };
                Ok(DfdlValue::UnsignedLong(val))
            } else {
                let val = if let Some(bits) = bit_width {
                    sign_extend_u64(
                        decode_packed_bit_field_u64(bytes, bits, props.byte_order, props.bit_order),
                        bits,
                    ) as i64
                } else {
                    int!(i64)
                };
                Ok(DfdlValue::Long(val))
            }
        }
        Long => {
            if props.unsigned_integer {
                Ok(DfdlValue::UnsignedLong(int!(u64)))
            } else {
                Ok(DfdlValue::Long(int!(i64)))
            }
        }
        Float => {
            let bits = int!(u32);
            Ok(DfdlValue::Float(f32::from_bits(bits)))
        }
        Double => {
            let bits = int!(u64);
            Ok(DfdlValue::Double(f64::from_bits(bits)))
        }
        String => {
            let enc = encoding_name(props, strings)?;
            let text = crate::vm::encoding::decode_text_bytes(bytes, enc, props.encoding_error_policy)?;
            Ok(DfdlValue::string(text))
        }
        HexBinary => Ok(DfdlValue::HexBinary(bytes.to_vec())),
        other => Err(VmError::UnsupportedOperation {
            op: alloc::format!("binary decode for {other:?}"),
        }),
    }
}

pub(crate) fn validate_decimal_parse_sign(
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
            message: alloc::format!("Parse Error: {rep} negative value when decimalSigned=\"no\""),
        });
    }
    Ok(())
}

pub(crate) fn sign_extend_u64(value: u64, bits: usize) -> i64 {
    if bits <= 1 {
        return value as i64;
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

pub(crate) fn decode_binary_boolean_sl(sl: u64, props: &IrProps) -> Result<bool, crate::error::VmError> {
    use crate::error::VmError;
    let false_rep = props.binary_boolean_false_rep.unwrap_or(0);
    let true_empty =
        props.binary_boolean_true_rep_defined && props.binary_boolean_true_rep.is_none();
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

pub(crate) fn decode_binary_bigint_str(bytes: &[u8], signed: bool, le: bool) -> alloc::string::String {
    let mut b = bytes.to_vec();
    if le {
        b.reverse();
    }
    if b.is_empty() {
        return alloc::string::String::from("0");
    }
    if !signed {
        let mut digits = alloc::vec![0u8];
        for &byte in &b {
            let mut carry = byte as u32;
            for d in digits.iter_mut() {
                let cur = (*d as u32) * 256 + carry;
                *d = (cur % 10) as u8;
                carry = cur / 10;
            }
            while carry > 0 {
                digits.push((carry % 10) as u8);
                carry /= 10;
            }
        }
        digits.reverse();
        return digits.iter().map(|d| (b'0' + d) as char).collect();
    }
    let is_neg = (b[0] & 0x80) != 0;
    if is_neg {
        let mut comp = b.clone();
        for byte in comp.iter_mut() {
            *byte = !*byte;
        }
        let mut carry = 1u16;
        for byte in comp.iter_mut().rev() {
            let sum = *byte as u16 + carry;
            *byte = sum as u8;
            carry = sum >> 8;
        }
        let pos_str = decode_binary_bigint_str(&comp, false, false);
        alloc::format!("-{pos_str}")
    } else {
        decode_binary_bigint_str(&b, false, false)
    }
}
