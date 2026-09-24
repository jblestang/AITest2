use crate::ir::{IrProps, StringPool, ValueKind};
use crate::length_validate::{
    validate_decimal_data_length_vm, validate_decimal_signed_one_bit_length_vm,
    validate_packed_binary_bit_length_parse, DaffodilTunables, VmDecimalPhase,
};
use crate::schema::{LengthKind, LengthUnits};
use crate::vm::encoding::hex_charset_order;

pub(crate) fn validate_explicit_decimal_vm(
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
        if hex_charset_order(enc).is_some() {
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
    validate_decimal_data_length_vm(props.decimal_signed, len, units, phase, runtime_resolved)?;
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
