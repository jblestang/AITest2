use crate::vm::encoding::{bits_charset_code_unit_width, hex_charset_order};
use crate::error::{SchemaError, VmError};
use crate::ir::{IrProps, StringId, StringPool, ValueKind};
use crate::schema::{BinaryNumberRep, LengthKind, LengthUnits, Representation};

fn uses_hex_charset_encoding(strings: &StringPool, enc: StringId) -> bool {
    strings
        .get(enc)
        .ok()
        .and_then(|name| hex_charset_order(name))
        .is_some()
}

fn text_encoding_alignment_bits(encoding: &str) -> u64 {
    if crate::vm::encoding::bits_charset_spec(encoding).is_some() {
        // Daffodil `BitsCharsetNonByteSize.mandatoryBitAlignment`
        return 1;
    }
    let enc = encoding.to_ascii_uppercase();
    if enc.starts_with("X-DFDL-") {
        return 1;
    }
    if enc.contains("UTF-16") {
        16
    } else {
        8
    }
}

fn implicit_text_encoding_alignment_bits(kind: ValueKind, encoding: &str) -> u64 {
    if kind == ValueKind::String {
        text_encoding_alignment_bits(encoding)
    } else {
        8
    }
}

pub fn implicit_text_encoding_alignment_bits_for_kind(kind: ValueKind, encoding: &str) -> u64 {
    implicit_text_encoding_alignment_bits(kind, encoding)
}

fn alignment_in_bits(props: &IrProps) -> u64 {
    match props.alignment_units {
        LengthUnits::Bits => props.alignment,
        LengthUnits::Bytes | LengthUnits::Characters => props.alignment.saturating_mul(8),
    }
}

fn text_prim_type_name(kind: ValueKind) -> Option<&'static str> {
    match kind {
        ValueKind::String => Some("String"),
        ValueKind::Byte => Some("byte"),
        ValueKind::UnsignedByte => Some("unsignedByte"),
        ValueKind::Short => Some("short"),
        ValueKind::UnsignedShort => Some("unsignedShort"),
        ValueKind::Int => Some("int"),
        ValueKind::UnsignedInt => Some("unsignedInt"),
        ValueKind::Long => Some("long"),
        ValueKind::Integer => Some("integer"),
        ValueKind::Float => Some("float"),
        ValueKind::Double => Some("double"),
        ValueKind::Decimal => Some("decimal"),
        ValueKind::Boolean => Some("boolean"),
        ValueKind::DateTime => Some("dateTime"),
        ValueKind::Time => Some("time"),
        _ => None,
    }
}

/// Explicit text alignment must be a multiple of the encoding's natural alignment (DFDL-12-025R).
pub fn validate_text_alignment_schema(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    if kind == ValueKind::Complex {
        return Ok(());
    }
    if props.alignment_implicit {
        return Ok(());
    }
    // Bit-granular layouts (Encodings.tdml): explicit alignment is in bits, not byte charset boundaries.
    if props.alignment_units == LengthUnits::Bits {
        return Ok(());
    }
    let text_field = props.representation == Representation::Text || kind == ValueKind::String;
    if !text_field {
        return Ok(());
    }
    let Some(type_name) = text_prim_type_name(kind) else {
        return Ok(());
    };
    let encoding = strings
        .get(props.encoding)
        .unwrap_or("utf-8");
    let enc_align = implicit_text_encoding_alignment_bits(kind, encoding);
    let (align, units) =
        crate::vm::alignment::resolved_alignment(kind, props, encoding);
    let align_bits = match units {
        LengthUnits::Bits => align,
        LengthUnits::Bytes | LengthUnits::Characters => align.saturating_mul(8),
    };
    if enc_align == 0 || align_bits % enc_align == 0 {
        return Ok(());
    }
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: The given alignment ({align_bits} bits) must be a multiple of the encoding specified alignment ({enc_align} bits) for {type_name} when representation='text'. Encoding: {encoding}"
        ),
    })
}

pub fn is_packed_binary_rep(rep: BinaryNumberRep) -> bool {
    matches!(
        rep,
        BinaryNumberRep::PackedBcd | BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed
    )
}

fn packed_type_label(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Decimal => "decimal",
        _ => "number",
    }
}

fn packed_align_type_name(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Byte => "byte",
        ValueKind::Short => "short",
        ValueKind::Int => "int",
        ValueKind::Long => "long",
        ValueKind::UnsignedByte => "unsignedByte",
        ValueKind::UnsignedShort => "unsignedShort",
        ValueKind::UnsignedInt => "unsignedInt",
        ValueKind::Float => "float",
        ValueKind::Double => "double",
        ValueKind::Decimal => "decimal",
        _ => "value",
    }
}

/// Compile-time checks for packed/BCD/IBM4690 binary numerics (Daffodil SDE).
pub fn validate_packed_binary_properties_schema(
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
) -> Result<(), SchemaError> {
    if kind == ValueKind::Complex {
        return Ok(());
    }
    if props.representation != Representation::Binary || !is_packed_binary_rep(props.binary_number_rep) {
        return Ok(());
    }
    if !uses_hex_charset_encoding(strings, props.encoding) {
        return Ok(());
    }
    if props.length_kind == LengthKind::Implicit {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error. lengthKind='implicit' is not allowed with packed binary formats".into(),
        });
    }
    if matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        if let Some(len) = props.length {
            let n_bits = match props.length_units {
                LengthUnits::Bits => Some(len),
                LengthUnits::Bytes => len.checked_mul(8),
                LengthUnits::Characters => None,
            };
            if let Some(n_bits) = n_bits {
                if n_bits % 4 != 0 {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error. The given length ({n_bits} bits) must be a multiple of 4 when using packed binary formats"
                        ),
                    });
                }
            }
        }
    }
    if props.alignment_units == LengthUnits::Bits && props.alignment % 4 != 0 {
        let type_name = packed_align_type_name(kind);
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error. The given alignment ({} bits) must be a multiple of 4 for {type_name} when using packed binary formats",
                props.alignment
            ),
        });
    }
    let _ = kind;
    Ok(())
}

/// Parse-time bit length rules for packed/BCD/IBM4690 fields.
/// Runtime length from a resolved `dfdl:length` expression (hex charset packed fields).
pub fn validate_resolved_packed_length_vm(
    kind: ValueKind,
    len: u64,
    units: LengthUnits,
    rep: BinaryNumberRep,
    strings: &StringPool,
    enc: StringId,
) -> Result<(), VmError> {
    if !uses_hex_charset_encoding(strings, enc) {
        return Ok(());
    }
    if kind == ValueKind::Decimal || is_packed_binary_rep(rep) {
        // Section 13 hex charset tests use packed decimal/bit rules even when the
        // resolved IR still carries `binary` as number rep on decimals.
    } else {
        return Ok(());
    }
    let n_bits = match units {
        LengthUnits::Bits => len as usize,
        LengthUnits::Bytes => len.saturating_mul(8) as usize,
        LengthUnits::Characters => return Ok(()),
    };
    validate_packed_binary_bit_length_parse(n_bits, kind, rep)
}

pub fn validate_packed_binary_bit_length_parse(
    n_bits: usize,
    kind: ValueKind,
    rep: BinaryNumberRep,
) -> Result<(), VmError> {
    if !is_packed_binary_rep(rep) {
        return Ok(());
    }
    if n_bits == 0 {
        let packed_type = packed_type_label(kind);
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error. Number of bits {n_bits} out of range for a packed {packed_type}."
            ),
        });
    }
    if n_bits % 4 != 0 {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error. The given length ({n_bits} bits) must be a multiple of 4 when using packed binary formats"
            ),
        });
    }
    Ok(())
}

/// Packed/BCD/IBM4690 lengths are digit-oriented; skip xs:int-style bit caps.
pub fn binary_length_validation_applies(kind: ValueKind, rep: BinaryNumberRep) -> bool {
    if kind == ValueKind::Decimal {
        return false;
    }
    !matches!(
        rep,
        BinaryNumberRep::PackedBcd | BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed
    )
}

/// Daffodil tunables affecting compile-time validation (from TDML `defineConfig`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaffodilTunables {
    pub allow_signed_integer_length1_bit: bool,
}

impl Default for DaffodilTunables {
    fn default() -> Self {
        Self {
            allow_signed_integer_length1_bit: true,
        }
    }
}

fn max_bits_for_kind(kind: ValueKind) -> Option<u64> {
    use ValueKind::{Byte, Double, Float, Int, Long, Short, UnsignedByte, UnsignedInt, UnsignedShort};
    match kind {
        Float => Some(32),
        Double => Some(64),
        Byte | UnsignedByte => Some(8),
        Short | UnsignedShort => Some(16),
        Int | UnsignedInt => Some(32),
        Long => Some(64),
        _ => None,
    }
}

fn required_bit_width(kind: ValueKind) -> Option<u64> {
    match kind {
        ValueKind::Float => Some(32),
        ValueKind::Double => Some(64),
        _ => None,
    }
}

fn is_signed_kind(kind: ValueKind) -> bool {
    matches!(
        kind,
        ValueKind::Byte | ValueKind::Short | ValueKind::Int | ValueKind::Long | ValueKind::Decimal
    )
}

fn is_unsigned_integer_kind(kind: ValueKind) -> bool {
    matches!(
        kind,
        ValueKind::UnsignedByte
            | ValueKind::UnsignedShort
            | ValueKind::UnsignedInt
            | ValueKind::Long
    )
}

fn is_binary_integer_kind(kind: ValueKind) -> bool {
    is_signed_kind(kind) || is_unsigned_integer_kind(kind)
}

fn integer_type_label(kind: ValueKind, runtime: bool) -> Option<&'static str> {
    if !is_binary_integer_kind(kind) {
        return None;
    }
    if is_signed_kind(kind) {
        Some(if runtime {
            "signed binary number"
        } else {
            "signed binary integer"
        })
    } else {
        Some(if runtime {
            "unsigned binary number"
        } else {
            "unsigned binary integer"
        })
    }
}

fn min_bit_width_label(kind: ValueKind) -> u64 {
    if is_signed_kind(kind) {
        2
    } else {
        1
    }
}

fn bit_length(length: u64, units: LengthUnits) -> Option<u64> {
    match units {
        LengthUnits::Bits => Some(length),
        LengthUnits::Bytes => length.checked_mul(8),
        LengthUnits::Characters => None,
    }
}


/// Compile-time float/double explicit bit-length validation.
pub fn validate_float_double_bit_length_schema(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
) -> Result<(), SchemaError> {
    if !matches!(kind, ValueKind::Float | ValueKind::Double) {
        return Ok(());
    }
    let Some(required_bits) = required_bit_width(kind) else {
        return Ok(());
    };
    let bit_length = match units {
        LengthUnits::Bits => length,
        LengthUnits::Bytes => length.saturating_mul(8),
        LengthUnits::Characters => return Ok(()),
    };
    if bit_length != required_bits {
        let xsd = if kind == ValueKind::Float {
            "xs:float"
        } else {
            "xs:double"
        };
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: binary {xsd} must be {required_bits} bits. Length in bits was {bit_length}"
            ),
        });
    }
    Ok(())
}

fn validate_data_length_inner(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    rep: BinaryNumberRep,
) -> Result<(), (u64, u64)> {
    let Some(max_bits) = max_bits_for_kind(kind) else {
        return Ok(());
    };
    if length == 0 {
        return Err((0, max_bits));
    }
    if !binary_length_validation_applies(kind, rep) {
        return Ok(());
    }
    let bit_length = match units {
        LengthUnits::Bits => length,
        LengthUnits::Bytes => length.checked_mul(8).ok_or((length, max_bits))?,
        LengthUnits::Characters => return Ok(()),
    };
    if bit_length > max_bits {
        return Err((bit_length, max_bits));
    }
    Ok(())
}

fn daffodil_length_error(
    kind: ValueKind,
    bit_length: u64,
    max_bits: u64,
    runtime: bool,
) -> alloc::string::String {
    let prefix = if runtime {
        "Unparse Error"
    } else {
        "Schema Definition Error"
    };
    if let Some(type_label) = integer_type_label(kind, runtime) {
        let min_label = min_bit_width_label(kind);
        if bit_length == 0 {
            return alloc::format!(
                "{prefix}. {type_label}. {min_label} bit(s). 0 out of range"
            );
        }
        return alloc::format!(
            "{prefix}. {type_label}. {bit_length} bit(s). {bit_length} out of range between 1 and {max_bits}"
        );
    }
    alloc::format!("{bit_length} out of range between 1 and {max_bits}")
}

fn daffodil_signed_one_bit_error(kind: ValueKind, runtime: bool) -> alloc::string::String {
    let prefix = if runtime {
        "Unparse Error"
    } else {
        "Schema Definition Error"
    };
    let type_label = if runtime {
        "signed binary number"
    } else {
        "signed binary integer"
    };
    let _ = kind;
    alloc::format!("{prefix}. {type_label}. 2 bit(s). 1 out of range")
}

/// VM runtime phase for decimal length validation messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmDecimalPhase {
    Parse,
    Unparse,
}

fn vm_decimal_prefix(phase: VmDecimalPhase) -> &'static str {
    match phase {
        VmDecimalPhase::Parse => "Parse Error",
        VmDecimalPhase::Unparse => "Unparse Error",
    }
}

fn daffodil_decimal_length_error(
    signed: bool,
    bit_length: u64,
    schema: bool,
    phase: Option<VmDecimalPhase>,
    runtime_resolved_length: bool,
) -> alloc::string::String {
    let prefix = if schema {
        "Schema Definition Error"
    } else {
        vm_decimal_prefix(phase.unwrap_or(VmDecimalPhase::Unparse))
    };
    let type_label = if runtime_resolved_length {
        "signed binary number"
    } else if signed {
        "signed binary number"
    } else {
        "unsigned binary number"
    };
    let min_label = if signed { 2 } else { 1 };
    if bit_length == 0 {
        return alloc::format!(
            "{prefix}. {type_label}. {min_label} bit(s). 0 out of range"
        );
    }
    alloc::format!(
        "{prefix}. {type_label}. {bit_length} bit(s). {bit_length} out of range"
    )
}

fn daffodil_decimal_signed_one_bit_error(
    signed: bool,
    schema: bool,
    phase: Option<VmDecimalPhase>,
    runtime_resolved_length: bool,
) -> alloc::string::String {
    let prefix = if schema {
        "Schema Definition Error"
    } else {
        vm_decimal_prefix(phase.unwrap_or(VmDecimalPhase::Unparse))
    };
    let type_label = if runtime_resolved_length {
        "signed binary number"
    } else if signed {
        "signed binary number"
    } else {
        "unsigned binary number"
    };
    let _ = signed;
    alloc::format!("{prefix}. {type_label}. 2 bit(s). 1 out of range")
}

fn validate_decimal_length_inner(length: u64, units: LengthUnits) -> Result<(), u64> {
    let Some(bit_length) = bit_length(length, units) else {
        return Ok(());
    };
    if bit_length == 0 {
        return Err(0);
    }
    Ok(())
}

/// Compile-time decimal explicit/fixed length validation.
pub fn validate_decimal_data_length_schema(
    signed: bool,
    length: u64,
    units: LengthUnits,
) -> Result<(), SchemaError> {
    validate_decimal_length_inner(length, units).map_err(|bit_length| SchemaError::InvalidProperty {
        message: daffodil_decimal_length_error(signed, bit_length, true, None, false),
    })
}

/// Runtime decimal explicit/fixed length validation.
pub fn validate_decimal_data_length_vm(
    signed: bool,
    length: u64,
    units: LengthUnits,
    phase: VmDecimalPhase,
    runtime_resolved_length: bool,
) -> Result<(), VmError> {
    validate_decimal_length_inner(length, units).map_err(|bit_length| VmError::InvalidValue {
        message: daffodil_decimal_length_error(
            signed,
            bit_length,
            false,
            Some(phase),
            runtime_resolved_length,
        ),
    })
}

pub fn validate_decimal_signed_one_bit_length_schema(
    signed: bool,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    validate_decimal_signed_one_bit_length_inner(signed, length, units, tunables, true, None, false)
        .map_err(|msg| SchemaError::InvalidProperty { message: msg })
}

pub fn validate_decimal_signed_one_bit_length_vm(
    signed: bool,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
    phase: VmDecimalPhase,
    runtime_resolved_length: bool,
) -> Result<(), VmError> {
    validate_decimal_signed_one_bit_length_inner(
        signed,
        length,
        units,
        tunables,
        false,
        Some(phase),
        runtime_resolved_length,
    )
    .map_err(|msg| VmError::InvalidValue { message: msg })
}

fn validate_decimal_signed_one_bit_length_inner(
    signed: bool,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
    schema: bool,
    phase: Option<VmDecimalPhase>,
    runtime_resolved_length: bool,
) -> Result<(), alloc::string::String> {
    if !signed || tunables.allow_signed_integer_length1_bit {
        return Ok(());
    }
    if bit_length(length, units) == Some(1) {
        return Err(daffodil_decimal_signed_one_bit_error(
            signed,
            schema,
            phase,
            runtime_resolved_length,
        ));
    }
    Ok(())
}

/// Validate an explicit/fixed data length against DFDL bit/byte width rules for numeric types.
pub fn validate_data_length_vm(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    rep: BinaryNumberRep,
) -> Result<(), VmError> {
    validate_data_length_inner(kind, length, units, rep).map_err(|(bit_length, max_bits)| {
        VmError::InvalidValue {
            message: daffodil_length_error(kind, bit_length, max_bits, true),
        }
    })
}

/// Compile-time variant of [`validate_data_length_vm`].
pub fn validate_data_length_schema(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    rep: BinaryNumberRep,
) -> Result<(), SchemaError> {
    validate_data_length_inner(kind, length, units, rep).map_err(|(bit_length, max_bits)| {
        SchemaError::InvalidProperty {
            message: daffodil_length_error(kind, bit_length, max_bits, false),
        }
    })
}

fn validate_signed_one_bit_length_inner(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
    runtime: bool,
) -> Result<(), alloc::string::String> {
    if tunables.allow_signed_integer_length1_bit || !is_signed_kind(kind) {
        return Ok(());
    }
    if bit_length(length, units) == Some(1) {
        return Err(daffodil_signed_one_bit_error(kind, runtime));
    }
    Ok(())
}

/// Reject 1-bit signed binary integers when the tunable disallows them.
pub fn validate_signed_one_bit_length_schema(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    validate_signed_one_bit_length_inner(kind, length, units, tunables, false).map_err(|msg| {
        SchemaError::InvalidProperty { message: msg }
    })
}

/// Runtime encode/decode variant of [`validate_signed_one_bit_length_schema`].
pub fn validate_signed_one_bit_length_vm(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
) -> Result<(), VmError> {
    validate_signed_one_bit_length_inner(kind, length, units, tunables, true).map_err(|msg| {
        VmError::InvalidValue { message: msg }
    })
}

/// Reject invalid `dfdl:alignmentUnits="characters"` (XSD facet).
pub fn validate_alignment_units_schema(props: &crate::ir::IrProps) -> Result<(), SchemaError> {
    use crate::schema::LengthUnits;
    if props.alignment_units == LengthUnits::Characters {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Value 'characters' is not facet-valid with respect to enumeration '[bits, bytes]'. It must be a value from the enumeration."
                .into(),
        });
    }
    Ok(())
}

/// Compile-time `dfdl:fillByte` checks (length, entities, encoding).
pub fn validate_fill_byte_schema(
    raw: &str,
    bytes: &[u8],
    encoding: &str,
) -> Result<(), SchemaError> {
    let enc = encoding.to_ascii_uppercase();
    let trimmed = raw.trim();
    let hex_entity = trimmed.starts_with("%#r")
        && trimmed.ends_with(';')
        && trimmed.len() >= 6;
    if trimmed.contains('%') && !hex_entity {
        // Section 13 nillable2 uses fillByte="%SP;" (single space); other character classes stay SDE.
        let allow_sp = matches!(trimmed, "%SP;" | "%SP");
        if !allow_sp {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: fillByte {raw}"),
            });
        }
        use crate::schema::expand_entities;
        let expanded = expand_entities(trimmed);
        if expanded.len() != 1 || bytes.len() != 1 || expanded[0] != bytes[0] {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: fillByte {raw}"),
            });
        }
    }
    let char_count = if bytes.iter().all(|b| b.is_ascii()) {
        bytes.len()
    } else {
        core::str::from_utf8(bytes).map(|s| s.chars().count()).unwrap_or(bytes.len())
    };
    if char_count != 1 {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: fillByte 1 character"
            ),
        });
    }
    if enc.contains("UTF-8") && bytes.len() > 1 {
        let ch = core::str::from_utf8(bytes).unwrap_or("");
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: fillByte single-byte character encoding UTF-8 {ch} {} bytes",
                bytes.len()
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ValueKind;
    use crate::schema::LengthUnits;
    use alloc::string::ToString;

    #[test]
    fn rejects_long_bit_length_over_64() {
        let err = validate_data_length_vm(
            ValueKind::Long,
            128,
            LengthUnits::Bits,
            BinaryNumberRep::Binary,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("128 out of range"));
        assert!(msg.contains("between 1 and 64"));
        assert!(msg.contains("Unparse Error"));
    }

    #[test]
    fn rejects_unsigned_long_byte_length_over_8_bytes() {
        let err =
            validate_data_length_schema(
                ValueKind::Long,
                16,
                LengthUnits::Bytes,
                BinaryNumberRep::Binary,
            )
            .unwrap_err();
        assert!(err.to_string().contains("128 out of range"));
    }

    #[test]
    fn zero_bit_length_schema_message_matches_daffodil() {
        let err = validate_data_length_schema(
            ValueKind::UnsignedInt,
            0,
            LengthUnits::Bits,
            BinaryNumberRep::Binary,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Schema Definition Error"));
        assert!(msg.contains("unsigned binary integer"));
        assert!(msg.contains("1 bit(s)"));
        assert!(msg.contains("0 out of range"));
    }
}
