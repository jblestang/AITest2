use crate::ir::{IrProps, ValueKind};
use crate::schema::{BinaryNumberRep, LengthUnits, Representation};
use crate::vm::runtime::Cursor;

/// Skip count in bits for `dfdl:leadingSkip` / `dfdl:trailingSkip` (uses `dfdl:alignmentUnits`).
pub fn skip_units_to_bits(props: &IrProps, skip: u64) -> usize {
    match props.alignment_units {
        LengthUnits::Bits => skip as usize,
        LengthUnits::Bytes => skip as usize * 8,
        LengthUnits::Characters => skip as usize * 8,
    }
}

pub fn implicit_alignment_in_bits(kind: ValueKind, props: &IrProps, encoding: &str) -> usize {
    if props.representation == Representation::Text {
        return text_encoding_alignment_bits(encoding);
    }
    if kind == ValueKind::Complex {
        return match props.alignment_units {
            LengthUnits::Bits => 1,
            LengthUnits::Bytes | LengthUnits::Characters => 8,
        };
    }
    let packed = matches!(
        props.binary_number_rep,
        BinaryNumberRep::PackedBcd | BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed
    );
    match kind {
        ValueKind::Float | ValueKind::Boolean => 32,
        ValueKind::Double => 64,
        ValueKind::HexBinary => 8,
        ValueKind::Long | ValueKind::Integer => {
            if packed { 8 } else { 64 }
        }
        ValueKind::Int | ValueKind::UnsignedInt => {
            if packed { 8 } else { 32 }
        }
        ValueKind::Short | ValueKind::UnsignedShort => {
            if packed { 8 } else { 16 }
        }
        ValueKind::Byte | ValueKind::UnsignedByte | ValueKind::Decimal => 8,
        ValueKind::DateTime | ValueKind::Time => 64,
        ValueKind::String => text_encoding_alignment_bits(encoding),
        ValueKind::Complex => 8,
    }
}

fn text_encoding_alignment_bits(encoding: &str) -> usize {
    let enc = encoding.to_ascii_uppercase();
    if enc.contains("UTF-16") {
        16
    } else {
        8
    }
}

/// Resolved `(alignment, alignment_units)` for consume/write alignment helpers.
pub fn resolved_alignment(kind: ValueKind, props: &IrProps, encoding: &str) -> (u64, LengthUnits) {
    if !props.alignment_implicit {
        return (props.alignment, props.alignment_units);
    }
    let bits = implicit_alignment_in_bits(kind, props, encoding);
    match props.alignment_units {
        LengthUnits::Bits => (bits as u64, LengthUnits::Bits),
        LengthUnits::Bytes | LengthUnits::Characters => {
            ((bits / 8).max(1) as u64, LengthUnits::Bytes)
        }
    }
}

/// Align the bit stream to the encoding boundary before reading text delimiters or text data.
pub fn align_cursor_to_text_encoding(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    encoding: &str,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let enc_bits = text_encoding_alignment_bits(encoding);
    if enc_bits <= 1 {
        return Ok(());
    }
    let pos = cursor.absolute_bit_index();
    let skip = (enc_bits - (pos % enc_bits)) % enc_bits;
    if skip > 0 {
        cursor
            .skip_stream_bits(skip, props.bit_order)
            .map_err(|_| VmError::UnexpectedEof)?;
    }
    Ok(())
}

pub fn consume_leading_skip(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if props.leading_skip == 0 {
        return Ok(());
    }
    let skip = skip_units_to_bits(props, props.leading_skip);
    if skip == 0 {
        return Ok(());
    }
    cursor
        .skip_stream_bits(skip, props.bit_order)
        .map_err(|_| VmError::UnexpectedEof)
}

pub fn write_leading_skip(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::vm::runtime::write_stream_bit;
    if props.leading_skip == 0 {
        return Ok(());
    }
    let skip = skip_units_to_bits(props, props.leading_skip);
    for _ in 0..skip {
        write_stream_bit(out, bit_count, 0, props.bit_order);
    }
    Ok(())
}

pub fn consume_trailing_skip(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if props.trailing_skip == 0 {
        return Ok(());
    }
    let skip = skip_units_to_bits(props, props.trailing_skip);
    if skip == 0 {
        return Ok(());
    }
    cursor
        .skip_stream_bits(skip, props.bit_order)
        .map_err(|_| VmError::UnexpectedEof)
}
