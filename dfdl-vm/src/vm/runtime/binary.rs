use super::*;
use crate::ir::*;
use crate::schema::*;
use alloc::vec::Vec;

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
        validate_packed_binary_bit_length_parse(
            size.saturating_mul(8),
            kind,
            props.binary_number_rep,
        )?;
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
            let len = props.length.ok_or(VmError::InvalidValue {
                message: "explicit binary missing length".into(),
            })?;
            if hex_charset_order(encoding_name(props, strings)?).is_some()
                && (kind == ValueKind::Decimal || is_packed_binary_rep(props.binary_number_rep))
            {
                validate_packed_binary_bit_length_parse(
                    len as usize,
                    kind,
                    props.binary_number_rep,
                )?;
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
