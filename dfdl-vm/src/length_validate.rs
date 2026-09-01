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
    use ValueKind::{Byte, Int, Long, Short, UnsignedByte, UnsignedInt, UnsignedShort};
    match kind {
        Byte | UnsignedByte => Some(8),
        Short | UnsignedShort => Some(16),
        Int | UnsignedInt => Some(32),
        Long => Some(64),
        _ => None,
    }
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

fn length_error_message(bit_length: u64, max_bits: u64) -> alloc::string::String {
    alloc::format!("{bit_length} out of range between 1 and {max_bits}")
}

/// Validate an explicit/fixed data length against DFDL bit/byte width rules for numeric types.
pub fn validate_data_length_vm(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
) -> Result<(), VmError> {
    validate_data_length_inner(kind, length, units).map_err(|(bit_length, max_bits)| {
        VmError::InvalidValue {
            message: length_error_message(bit_length, max_bits),
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
            message: length_error_message(bit_length, max_bits),
        }
    })
}

fn is_signed_kind(kind: ValueKind) -> bool {
    matches!(
        kind,
        ValueKind::Byte | ValueKind::Short | ValueKind::Int | ValueKind::Long | ValueKind::Decimal
    )
}

fn bit_length(length: u64, units: LengthUnits) -> Option<u64> {
    match units {
        LengthUnits::Bits => Some(length),
        LengthUnits::Bytes => length.checked_mul(8),
        LengthUnits::Characters => None,
    }
}

/// Reject 1-bit signed binary integers when the tunable disallows them.
pub fn validate_signed_one_bit_length_schema(
    kind: ValueKind,
    length: u64,
    units: LengthUnits,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if tunables.allow_signed_integer_length1_bit || !is_signed_kind(kind) {
        return Ok(());
    }
    if bit_length(length, units) == Some(1) {
        return Err(SchemaError::InvalidProperty {
            message: "signed binary integer length 1 bit(s) out of range".into(),
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
        let err = validate_data_length_vm(ValueKind::Long, 128, LengthUnits::Bits).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("128 out of range"));
        assert!(msg.contains("between 1 and 64"));
    }

    #[test]
    fn rejects_unsigned_long_byte_length_over_8_bytes() {
        let err =
            validate_data_length_schema(ValueKind::Long, 16, LengthUnits::Bytes).unwrap_err();
        assert!(err.to_string().contains("128 out of range"));
    }
}
