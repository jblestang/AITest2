use crate::error::{SchemaError, VmError};
use crate::ir::ValueKind;
use crate::schema::LengthUnits;

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
    if !matches!(kind, ValueKind::Float | ValueKind::Double) || units != LengthUnits::Bits {
        return Ok(());
    }
    if let Some(required) = required_bit_width(kind) {
        if length != required {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error. must be {required} bits"),
            });
        }
    }
    Ok(())
}

fn validate_data_length_inner(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
) -> Result<(), (u64, u64)> {
    let Some(max_bits) = max_bits_for_kind(kind) else {
        return Ok(());
    };
    if length == 0 {
        return Err((0, max_bits));
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

fn daffodil_decimal_length_error(
    signed: bool,
    bit_length: u64,
    runtime: bool,
) -> alloc::string::String {
    let prefix = if runtime {
        "Unparse Error"
    } else {
        "Schema Definition Error"
    };
    let type_label = if signed {
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

fn daffodil_decimal_signed_one_bit_error(signed: bool, runtime: bool) -> alloc::string::String {
    let prefix = if runtime {
        "Unparse Error"
    } else {
        "Schema Definition Error"
    };
    let type_label = if signed {
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
        message: daffodil_decimal_length_error(signed, bit_length, false),
    })
}

/// Runtime decimal explicit/fixed length validation.
pub fn validate_decimal_data_length_vm(
    signed: bool,
    length: u64,
    units: LengthUnits,
) -> Result<(), VmError> {
    validate_decimal_length_inner(length, units).map_err(|bit_length| VmError::InvalidValue {
        message: daffodil_decimal_length_error(signed, bit_length, true),
    })
}

pub fn validate_decimal_signed_one_bit_length_schema(
    signed: bool,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    validate_decimal_signed_one_bit_length_inner(signed, length, units, tunables, false)
        .map_err(|msg| SchemaError::InvalidProperty { message: msg })
}

pub fn validate_decimal_signed_one_bit_length_vm(
    signed: bool,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
) -> Result<(), VmError> {
    validate_decimal_signed_one_bit_length_inner(signed, length, units, tunables, true)
        .map_err(|msg| VmError::InvalidValue { message: msg })
}

fn validate_decimal_signed_one_bit_length_inner(
    signed: bool,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
    runtime: bool,
) -> Result<(), alloc::string::String> {
    if !signed || tunables.allow_signed_integer_length1_bit {
        return Ok(());
    }
    if bit_length(length, units) == Some(1) {
        return Err(daffodil_decimal_signed_one_bit_error(signed, runtime));
    }
    Ok(())
}

/// Validate an explicit/fixed data length against DFDL bit/byte width rules for numeric types.
pub fn validate_data_length_vm(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
) -> Result<(), VmError> {
    validate_data_length_inner(kind, length, units).map_err(|(bit_length, max_bits)| {
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
) -> Result<(), SchemaError> {
    validate_data_length_inner(kind, length, units).map_err(|(bit_length, max_bits)| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ValueKind;
    use crate::schema::LengthUnits;
    use alloc::string::ToString;

    #[test]
    fn rejects_long_bit_length_over_64() {
        let err = validate_data_length_vm(ValueKind::Long, 128, LengthUnits::Bits).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("128 out of range"));
        assert!(msg.contains("between 1 and 64"));
        assert!(msg.contains("Unparse Error"));
    }

    #[test]
    fn rejects_unsigned_long_byte_length_over_8_bytes() {
        let err =
            validate_data_length_schema(ValueKind::Long, 16, LengthUnits::Bytes).unwrap_err();
        assert!(err.to_string().contains("128 out of range"));
    }

    #[test]
    fn zero_bit_length_schema_message_matches_daffodil() {
        let err = validate_data_length_schema(ValueKind::UnsignedInt, 0, LengthUnits::Bits).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Schema Definition Error"));
        assert!(msg.contains("unsigned binary integer"));
        assert!(msg.contains("1 bit(s)"));
        assert!(msg.contains("0 out of range"));
    }
}
