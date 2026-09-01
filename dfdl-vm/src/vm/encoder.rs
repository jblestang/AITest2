use super::runtime::{write_alignment, write_byte_aligned, write_framed_payload, write_simple, validate_explicit_decimal_before_encode, trailing_suppressed_count, RuntimeConfig, VmContext};
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::{
    encode_delimiter, LengthKind, LengthUnits, OutputValueCalc, SeparatorPosition,
};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// DFDL encoder VM — executes compiled IR to serialize logical values.
pub struct Encoder<'a> {
    ctx: VmContext<'a>,
}

impl<'a> Encoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
        }
    }

    /// Encode `value` and append bytes to `output`. Returns trailing bit count in the last byte.
    pub fn encode(&self, value: &DfdlValue, output: &mut Vec<u8>) -> Result<()> {
        let _ = self.encode_with_bit_count(value, output)?;
        Ok(())
    }

    /// Encode `value` and return the number of significant bits in the last output byte (0 if byte-aligned).
    pub fn encode_with_bit_count(
        &self,
        value: &DfdlValue,
        output: &mut Vec<u8>,
    ) -> Result<u8> {
        let value = unwrap_root_for_encode(value, &self.ctx.program.root_element);
        let mut bit_count = 0u8;
        self.encode_node(self.ctx.program.root, value, output, &mut bit_count)?;
        Ok(bit_count)
    }

    /// Encode into a freshly allocated buffer.
    pub fn encode_to_vec(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(value, &mut out)?;
        Ok(out)
    }

    fn encode_node(
        &self,
        node_id: u32,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
    ) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, props } => {
                let map = value.as_sequence_fields()?;
                let effective = precompute_output_values(self, children, map)?;
                for (idx, &child) in children.iter().enumerate() {
                    if child_skips_encode(self, child)? {
                        continue;
                    }
                    self.write_separator(props, out, bit_count, idx, children.len())?;
                    self.encode_sequence_particle(child, &effective, props, out, bit_count)?;
                }
                Ok(())
            }
            IrNode::Choice { branches, .. } => {
                if let DfdlValue::Choice { discriminator, value } = value {
                    let branch = branches
                        .iter()
                        .find(|b| {
                            self.ctx
                                .strings()
                                .get(b.name)
                                .map(|name| name == discriminator)
                                .unwrap_or(false)
                        })
                        .ok_or(VmError::InvalidChoice)?;
                    return self.encode_node(branch.node, value, out, bit_count);
                }
                if let DfdlValue::Sequence(map) = value {
                    for branch in branches {
                        let key = self.ctx.strings().get(branch.name)?;
                        if let Some(branch_value) = map.get(key) {
                            return self.encode_node(branch.node, branch_value, out, bit_count);
                        }
                    }
                }
                Err(VmError::InvalidChoice.into())
            }
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                if let Some(child_id) = child {
                    let name_str = self.ctx.strings().get(*name)?;
                    let field = match value.field(name_str) {
                        Some(inner) => inner,
                        None => value,
                    };
                    if needs_length_frame(props) {
                        self.encode_framed_element(
                            *child_id,
                            props,
                            field,
                            out,
                            bit_count,
                            Some(name_str),
                        )
                    } else {
                        self.encode_element_occurrences(*child_id, props, field, out, bit_count, props)
                    }
                } else {
                    write_alignment(out, bit_count, props)?;
                    write_simple(
                        out,
                        bit_count,
                        value,
                        *kind,
                        props,
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        Some(self.ctx.strings().get(*name)?),
                    )
                    .map_err(Into::into)
                }
            }
        }
    }

    fn encode_framed_element(
        &self,
        child_id: u32,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        field_name: Option<&str>,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed = trailing_suppressed_count(items, props, self.ctx.strings())?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            self.write_occurrence_separator(props, out, bit_count, idx, encode_len)?;
            write_alignment(out, bit_count, props)?;
            if let Some(id) = props.initiator {
                write_byte_aligned(
                    out,
                    bit_count,
                    &encode_delimiter(self.ctx.strings().get(id)?),
                )
                .map_err(Error::from)?;
            }
            let mut payload = Vec::new();
            let mut payload_bit_count = 0u8;
            self.encode_node(child_id, item, &mut payload, &mut payload_bit_count)?;
            write_framed_payload(
                out,
                bit_count,
                &payload,
                payload_bit_count,
                props,
                self.ctx.strings(),
                field_name,
            )?;
        }
        Ok(())
    }

    fn encode_element_occurrences(
        &self,
        node_id: u32,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        sep_props: &IrProps,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed = trailing_suppressed_count(items, props, self.ctx.strings())?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if sep_props.separator_position != SeparatorPosition::Postfix {
                self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
            }
            write_alignment(out, bit_count, props)?;
            self.encode_node(node_id, item, out, bit_count)?;
            if sep_props.separator_position == SeparatorPosition::Postfix {
                self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
            }
        }
        Ok(())
    }

    fn encode_sequence_particle(
        &self,
        node_id: u32,
        map: &BTreeMap<String, DfdlValue>,
        parent_props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
    ) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                let key = self.ctx.strings().get(*name)?;
                let value = self.element_encode_value(props, key, map)?;
                let resolved = resolve_length_props_encode(props, map, self.ctx.strings())?;
                validate_explicit_decimal_before_encode(
                    *kind,
                    &resolved,
                    &self.ctx.program.tunables,
                )?;
                if let Some(child_id) = child {
                    let field = match value.field(key) {
                        Some(inner) => inner,
                        None => &value,
                    };
                    if needs_length_frame(&resolved) {
                        self.encode_framed_element(
                            *child_id,
                            &resolved,
                            field,
                            out,
                            bit_count,
                            Some(key),
                        )
                    } else {
                        self.encode_element_occurrences(
                            *child_id,
                            &resolved,
                            field,
                            out,
                            bit_count,
                            parent_props,
                        )
                    }
                } else {
                    self.encode_simple_occurrences(*kind, &resolved, &value, out, bit_count, Some(key))
                }
            }
            IrNode::Sequence { .. } => self.encode_node(
                node_id,
                &DfdlValue::Sequence(map.clone()),
                out,
                bit_count,
            ),
            IrNode::Choice { .. } => {
                for (discriminator, value) in map {
                    if branches_contain(self.ctx.program, node_id, discriminator) {
                        return self.encode_node(
                            node_id,
                            &DfdlValue::choice(discriminator.clone(), value.clone()),
                            out,
                            bit_count,
                        );
                    }
                }
                Err(VmError::InvalidChoice.into())
            }
        }
    }

    fn encode_simple_occurrences(
        &self,
        kind: crate::ir::ValueKind,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        field_name: Option<&str>,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed = trailing_suppressed_count(items, props, self.ctx.strings())?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            self.write_occurrence_separator(props, out, bit_count, idx, encode_len)?;
            write_alignment(out, bit_count, props)?;
            write_simple(
                out,
                bit_count,
                item,
                kind,
                props,
                self.ctx.strings(),
                &self.ctx.program.tunables,
                field_name,
            )
            .map_err(Error::from)?;
        }
        Ok(())
    }

    fn element_encode_value(
        &self,
        props: &IrProps,
        key: &str,
        map: &BTreeMap<String, DfdlValue>,
    ) -> Result<DfdlValue> {
        if props.output_value_calc.is_some() {
            eval_output_value_calc(props, map, self.ctx.strings())
        } else {
            map.get(key)
                .cloned()
                .ok_or_else(|| VmError::MissingField { name: key.into() }.into())
        }
    }

    fn write_separator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        index: usize,
        total: usize,
    ) -> Result<()> {
        self.write_separator_mode(props, out, bit_count, index, total, false)
    }

    fn write_occurrence_separator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        index: usize,
        total: usize,
    ) -> Result<()> {
        self.write_separator_mode(props, out, bit_count, index, total, true)
    }

    fn write_separator_mode(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        index: usize,
        total: usize,
        occurrences: bool,
    ) -> Result<()> {
        if !should_emit_separator(props.separator_position, index, total, occurrences) {
            return Ok(());
        }
        if let Some(id) = props.separator {
            write_byte_aligned(out, bit_count, &encode_delimiter(self.ctx.strings().get(id)?))
                .map_err(Error::from)?;
        }
        Ok(())
    }
}

fn child_skips_encode(enc: &Encoder<'_>, node_id: u32) -> Result<bool> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { props, .. } => Ok(props.input_value_calc.is_some()),
        _ => Ok(false),
    }
}

fn precompute_output_values<'a>(
    enc: &Encoder<'a>,
    children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
) -> Result<BTreeMap<String, DfdlValue>> {
    let mut effective = map.clone();
    for &child in children {
        let IrNode::Element { name, props, .. } = enc.ctx.program.node(child)? else {
            continue;
        };
        if props.output_value_calc.is_none() {
            continue;
        }
        let key = enc.ctx.strings().get(*name)?.to_string();
        let computed = eval_output_value_calc(props, &effective, enc.ctx.strings())?;
        effective.insert(key, computed);
    }
    Ok(effective)
}

fn eval_output_value_calc(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<DfdlValue> {
    let calc = props.output_value_calc.ok_or_else(|| VmError::InvalidValue {
        message: "missing outputValueCalc".into(),
    })?;
    let len = match calc {
        OutputValueCalc::Constant(v) => v,
        OutputValueCalc::ContentLengthSelf(units, addend) => {
            length_in_units(0, units)? as i64 + addend
        }
        OutputValueCalc::ValueLengthSelf(units, addend) => {
            length_in_units(0, units)? as i64 + addend
        }
        OutputValueCalc::ContentLengthSibling(_units, addend) => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            value_byte_length(sib)? as i64 + addend
        }
        OutputValueCalc::ValueLengthSibling(units, addend) => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            length_in_units(value_byte_length(sib)?, units)? as i64 + addend
        }
    };
    Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| VmError::InvalidValue {
        message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
    })?))
}

fn resolve_length_props_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<IrProps> {
    if props.length_kind != LengthKind::Explicit || props.length.is_some() {
        return Ok(props.clone());
    }
    let Some(sib_id) = props.length_sibling else {
        return Ok(props.clone());
    };
    let sib_name = strings.get(sib_id)?;
    let sib_val = map.get(sib_name).ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("length sibling `{sib_name}` not available"),
    })?;
    let mut resolved = props.clone();
    resolved.length = Some(length_from_value(sib_val, props.length_sibling_cast_long)?);
    Ok(resolved)
}

fn sibling_from_map<'a>(
    id: Option<crate::ir::StringId>,
    map: &'a BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<&'a DfdlValue> {
    let id = id.ok_or_else(|| VmError::InvalidValue {
        message: "outputValueCalc sibling missing".into(),
    })?;
    let name = strings.get(id)?;
    map.get(name).ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("outputValueCalc sibling `{name}` not available"),
    })
    .map_err(Into::into)
}

fn length_in_units(byte_len: usize, units: LengthUnits) -> Result<usize> {
    match units {
        LengthUnits::Bytes => Ok(byte_len),
        LengthUnits::Bits => Ok(byte_len.saturating_mul(8)),
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "outputValueCalc character units".into(),
        }
        .into()),
    }
}

fn value_byte_length(value: &DfdlValue) -> Result<usize> {
    match value {
        DfdlValue::String(s) => Ok(s.len()),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => Ok(s.len()),
        DfdlValue::HexBinary(v) => Ok(v.len()),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("valueLength on unsupported value `{other:?}`"),
        }
        .into()),
    }
}

fn negative_runtime_length_error(value: i64) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!("Runtime Schema Definition Error. dfdl:length {value}"),
    }
}

fn length_from_value(value: &DfdlValue, cast_long: bool) -> Result<u64> {
    match value {
        DfdlValue::Double(v) if cast_long => {
            if v.is_nan() {
                return Err(VmError::InvalidValue {
                    message: "Parse Error. Cannot convert NaN double value to xs:long".into(),
                }
                .into());
            }
            let truncated = *v as i64;
            u64::try_from(truncated).map_err(|_| negative_runtime_length_error(truncated).into())
        }
        DfdlValue::Byte(v) => {
            let v = *v as i64;
            u64::try_from(v).map_err(|_| negative_runtime_length_error(v).into())
        }
        DfdlValue::UnsignedByte(v) => Ok(*v as u64),
        DfdlValue::Short(v) => {
            let v = *v as i64;
            u64::try_from(v).map_err(|_| negative_runtime_length_error(v).into())
        }
        DfdlValue::UnsignedShort(v) => Ok(*v as u64),
        DfdlValue::Int(v) => {
            u64::try_from(*v).map_err(|_| negative_runtime_length_error(*v as i64).into())
        }
        DfdlValue::UnsignedInt(v) => Ok(*v as u64),
        DfdlValue::Long(v) => {
            u64::try_from(*v).map_err(|_| negative_runtime_length_error(*v).into())
        }
        other => Err(VmError::InvalidValue {
            message: alloc::format!("length sibling has unsupported type: {other:?}"),
        }
        .into()),
    }
}

fn should_emit_separator(
    position: SeparatorPosition,
    index: usize,
    total: usize,
    occurrences: bool,
) -> bool {
    match position {
        SeparatorPosition::Prefix => index < total,
        SeparatorPosition::Infix => index > 0,
        SeparatorPosition::Postfix if occurrences => index < total,
        SeparatorPosition::Postfix => index + 1 < total,
    }
}

fn needs_length_frame(props: &IrProps) -> bool {
    matches!(
        props.length_kind,
        LengthKind::Prefixed | LengthKind::Explicit | LengthKind::Fixed | LengthKind::Delimited
    )
}

fn unwrap_root_for_encode<'a>(value: &'a DfdlValue, root_element: &str) -> &'a DfdlValue {
    if let DfdlValue::Sequence(map) = value {
        if map.len() == 1 {
            if let Some((name, inner)) = map.iter().next() {
                if name == root_element {
                    return inner;
                }
            }
        }
    }
    value
}

fn branches_contain(program: &IrProgram, node_id: u32, name: &str) -> bool {
    match program.node(node_id) {
        Ok(IrNode::Choice { branches, .. }) => branches.iter().any(|b| {
            program
                .strings
                .get(b.name)
                .map(|branch_name| branch_name == name)
                .unwrap_or(false)
        }),
        _ => false,
    }
}

trait ValueView {
    fn as_sequence_fields(&self) -> Result<&BTreeMap<String, DfdlValue>>;
}

impl ValueView for DfdlValue {
    fn as_sequence_fields(&self) -> Result<&BTreeMap<String, DfdlValue>> {
        match self {
            DfdlValue::Sequence(map) => Ok(map),
            _ => Err(VmError::TypeMismatch {
                expected: "sequence".into(),
            }
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SeparatorPosition;

    #[test]
    fn occurrence_postfix_emits_after_last_item() {
        assert!(should_emit_separator(
            SeparatorPosition::Postfix,
            2,
            3,
            true,
        ));
        assert!(!should_emit_separator(
            SeparatorPosition::Postfix,
            2,
            3,
            false,
        ));
    }

    #[test]
    fn sibling_postfix_does_not_emit_after_last_child() {
        assert!(!should_emit_separator(
            SeparatorPosition::Postfix,
            0,
            1,
            false,
        ));
    }

    #[test]
    fn infix_separator_skips_first_item() {
        assert!(!should_emit_separator(
            SeparatorPosition::Infix,
            0,
            3,
            true,
        ));
        assert!(should_emit_separator(
            SeparatorPosition::Infix,
            1,
            3,
            true,
        ));
    }
}
