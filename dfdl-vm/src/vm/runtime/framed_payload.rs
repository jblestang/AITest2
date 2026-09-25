use super::binary::*;
use super::cursor::Cursor;
use super::encoding_name;
use super::numeric_text_parse::{read_implicit_numeric_text, type_size};
use super::pad_trim::{pad_char_from_props, trim_numeric_text};
use super::property::{encode_framing_delimiter_bytes, resolve_output_new_line_for_encode};
use super::RuntimeConfig;
use super::numeric_text_format::length_kind_name;
use crate::ir::{IrPrefixLength, IrProps, StringPool};
use crate::schema::{
    BinaryNumberRep, BitOrder, ByteOrder, EncodingErrorPolicy, LengthKind,
    LengthUnits, Representation, TextNumberJustification,
};
use crate::vm::encoding::{count_characters, encode_document_text};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub(crate) fn bits_available_in_cursor(cursor: &Cursor<'_>) -> usize {
    let total_bits = cursor.data.len().saturating_mul(8);
    let pos = cursor.absolute_bit_index();
    let limit = cursor.frame_bit_limit.unwrap_or(total_bits);
    limit
        .saturating_sub(pos)
        .min(total_bits.saturating_sub(pos))
}

pub(crate) fn insufficient_data_bits_error(
    needed_bits: usize,
    found_bits: usize,
) -> crate::error::VmError {
    use crate::error::VmError;
    VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. Insufficient bits in data. Needed {needed_bits} bit(s) but found only {found_bits} ({found_bits} available)"
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
            if let Some(order) = crate::vm::encoding::hex_charset_order(encoding) {
                let needed_bits = len.saturating_mul(8);
                let available_bits = cursor
                    .data
                    .len()
                    .saturating_mul(8)
                    .saturating_sub(cursor.absolute_bit_index());
                if available_bits < needed_bits {
                    if allow_short_read {
                        let avail_bytes = available_bits / 8;
                        if avail_bytes > 0 {
                            return cursor.read_hex_charset_bytes(avail_bytes, order);
                        }
                        return Ok(Vec::new());
                    }
                    return Err(insufficient_data_bits_error(needed_bits, available_bits));
                }
                return cursor.read_hex_charset_bytes(len, order);
            }
            if cursor.bit_count != 0 {
                let mut bytes = Vec::with_capacity(len);
                for _ in 0..len {
                    let b = cursor.read_stream_bits(8, bit_order)?;
                    bytes.push(b as u8);
                }
                return Ok(bytes);
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
            cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)
        }
        LengthUnits::Characters => {
            let mut pos = cursor.pos;
            if allow_short_read {
                let mut out = alloc::vec::Vec::new();
                for _ in 0..len {
                    if pos >= cursor.data.len() {
                        break;
                    }
                    match crate::vm::encoding::read_character_bytes(
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
            let bytes = crate::vm::encoding::read_character_bytes(
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
        LengthUnits::Bits => span.checked_div(8).ok_or(VmError::InvalidValue {
            message: "prefixed bit span not byte-aligned".into(),
        }),
        LengthUnits::Characters => {
            crate::vm::encoding::character_span_byte_length(span, encoding_name(props, strings)?)
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
        let name = field_name.unwrap_or("element");
        crate::error::VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error. Failed to populate {name}. Cause: vm error: {e}"
            ),
        }
    })
}

pub(crate) fn read_prefixed_span(
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
        LengthUnits::Characters => count_characters(
            &cursor.data[start..cursor.pos],
            encoding_name(props, strings)?,
            props.encoding_error_policy,
        ),
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

pub(crate) fn parse_u64(s: &str) -> Result<u64, crate::error::VmError> {
    s.parse().map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("invalid non-negative integer `{s}`"),
    })
}

fn usize_from_u64(v: u64) -> Result<usize, crate::error::VmError> {
    usize::try_from(v).map_err(|_| crate::error::VmError::InvalidValue {
        message: alloc::format!("length value `{v}` out of range"),
    })
}

pub(crate) fn write_prefixed_bytes(
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
        LengthUnits::Bits => payload.len().checked_mul(8).ok_or(VmError::InvalidValue {
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
                    LengthUnits::Bits => len.checked_div(8).ok_or(VmError::InvalidValue {
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
            op: alloc::format!("prefix lengthKind `{}` encode", length_kind_name(other)),
        }),
    }
}

fn prefix_field_byte_length(prefix: &IrPrefixLength) -> Result<usize, crate::error::VmError> {
    use crate::error::VmError;
    let len = match prefix.props.length_kind {
        LengthKind::Explicit | LengthKind::Fixed => {
            prefix.props.length.ok_or(VmError::InvalidValue {
                message: "prefix type missing length".into(),
            })? as usize
        }
        LengthKind::Implicit => type_size(prefix.kind),
        other => {
            return Err(VmError::UnsupportedOperation {
                op: alloc::format!("prefix lengthKind `{}` encode", length_kind_name(other)),
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
    use crate::error::VmError;
    use crate::schema::Representation;

    validate_prefix_facets(value, prefix, field_name)?;
    if prefix.props.length_kind == LengthKind::Prefixed {
        return Err(VmError::InvalidValue {
            message:
                "Schema Definition Error. Nested dfdl:lengthKind=\"prefixed\" is not supported"
                    .into(),
        });
    }
    match prefix.props.representation {
        Representation::Text => {
            write_text_prefix_field(out, bit_count, value, prefix, element_length_units, strings)
        }
        Representation::Binary => write_binary_prefix_field(out, bit_count, value, prefix, strings),
    }
}

fn number_pad_char(props: &IrProps, strings: &StringPool, kind: crate::ir::ValueKind) -> char {
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
    let enc_align = crate::length_validate::implicit_text_encoding_alignment_bits_for_kind(
        prefix.kind,
        encoding,
    );
    let mut align_bits = if prefix.props.length_units == LengthUnits::Bits {
        if prefix.props.alignment_implicit {
            crate::vm::alignment::implicit_alignment_in_bits(prefix.kind, &prefix.props, encoding)
                as u64
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
                            padded.extend(core::iter::repeat(pad).take(pad_count));
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
                    while count_characters(padded.as_bytes(), encoding, EncodingErrorPolicy::Error)?
                        < len
                    {
                        match justification {
                            TextNumberJustification::Right => {
                                padded.insert(0, pad);
                            }
                            TextNumberJustification::Left => {
                                padded.push(pad);
                            }
                            TextNumberJustification::Center => {
                                let current = count_characters(
                                    padded.as_bytes(),
                                    encoding,
                                    EncodingErrorPolicy::Error,
                                )?;
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
                    if count_characters(padded.as_bytes(), encoding, EncodingErrorPolicy::Error)?
                        > len
                    {
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
                    while padded.len() < byte_len {
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
            op: alloc::format!("prefix lengthKind `{}` encode", length_kind_name(other)),
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
            bytes.splice(0..0, core::iter::repeat(0u8).take(pad));
        } else {
            bytes.extend(core::iter::repeat(0u8).take(pad));
        }
    }
    if le && prefix.props.binary_number_rep == BinaryNumberRep::Binary {
        bytes.reverse();
    }
    write_byte_aligned(out, bit_count, &bytes)?;
    Ok(())
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
            write_stream_bit_with_config(
                out,
                bit_count,
                props.fill_byte & 1,
                props.bit_order,
                config,
            );
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
            write_stream_bit_with_config(
                out,
                bit_count,
                props.fill_byte & 1,
                props.bit_order,
                config,
            );
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
        out.extend(std::iter::repeat_n(props.fill_byte, skip));
    }
    Ok(())
}

#[allow(dead_code)]
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
    strings: &crate::ir::StringPool,
) -> Result<(), crate::error::VmError> {
    crate::vm::alignment::consume_leading_skip(cursor, props)?;
    if props.representation == crate::schema::Representation::Binary && props.length_sibling.is_some() {
        if let Ok(0) = super::binary::binary_bit_length(cursor, kind, props, strings) {
            return Ok(());
        }
    }
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
    crate::vm::alignment::consume_trailing_skip(cursor, props)?;
    if props.framing_alignment > 1 {
        let align_bits = crate::vm::alignment::skip_units_to_bits(props, props.framing_alignment);
        if align_bits > 0 {
            let rem = cursor.absolute_bit_index() % align_bits;
            if rem != 0 {
                let pad = align_bits - rem;
                let _ = cursor.skip_stream_bits(pad, props.bit_order);
            }
        }
    }
    Ok(())
}

fn payload_bit_length(payload: &[u8], payload_bit_count: u8) -> usize {
    if payload_bit_count == 0 {
        payload.len() * 8
    } else {
        payload.len().saturating_sub(1) * 8 + payload_bit_count as usize
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
                bytes.extend(std::iter::repeat_n(pad, len - bytes.len()));
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
            let text = alloc::string::String::from_utf8_lossy(payload);
            let bytes = super::numeric_text_format::pad_text_field(
                &text,
                len,
                props.length_units,
                props,
                strings,
                crate::ir::ValueKind::String,
                encoding,
            )?;
            write_byte_aligned(out, bit_count, &bytes)?;
            Ok(())
        }
    }
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
    encode_siblings: Option<&BTreeMap<String, crate::value::DfdlValue>>,
) -> Result<(), crate::error::VmError> {
    match props.length_kind {
        LengthKind::Prefixed => {
            write_prefixed_bytes(out, bit_count, payload, props, strings, field_name)
        }
        LengthKind::Explicit | LengthKind::Fixed => write_explicit_payload(
            out,
            bit_count,
            payload,
            payload_bit_count,
            props,
            strings,
            config,
        ),
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
                    let encoding = encoding_name(props, strings)?;
                    let output_nl =
                        resolve_output_new_line_for_encode(props, encode_siblings, strings)?
                            .map(|s| s as alloc::string::String);
                    let bytes = encode_framing_delimiter_bytes(
                        pat,
                        output_nl.as_deref(),
                        encoding,
                        delim_meta.and_then(|m| m.terminator_alt),
                    );
                    write_byte_aligned(out, bit_count, &bytes)?;
                }
            }
            Ok(())
        }
        _ => {
            write_bits_from_stream_with_config(
                out,
                bit_count,
                payload,
                payload_bit_length(payload, payload_bit_count),
                props.bit_order,
                config,
            )?;
            Ok(())
        }
    }
}
