use super::runtime::{write_alignment, write_framed_payload, write_simple, RuntimeConfig, VmContext};
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

    /// Encode `value` and append bytes to `output`.
    pub fn encode(&self, value: &DfdlValue, output: &mut Vec<u8>) -> Result<()> {
        let value = unwrap_root_for_encode(value, &self.ctx.program.root_element);
        self.encode_node(self.ctx.program.root, value, output)
    }

    /// Encode into a freshly allocated buffer.
    pub fn encode_to_vec(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(value, &mut out)?;
        Ok(out)
    }

    fn encode_node(&self, node_id: u32, value: &DfdlValue, out: &mut Vec<u8>) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, props } => {
                let map = value.as_sequence_fields()?;
                let effective = precompute_output_values(self, children, map)?;
                for (idx, &child) in children.iter().enumerate() {
                    self.write_separator(props, out, idx, children.len())?;
                    self.encode_sequence_particle(child, &effective, out)?;
                }
                Ok(())
            }
            IrNode::Choice { branches, .. } => {
                let (discriminator, branch_value) = value.as_choice_fields()?;
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
                self.encode_node(branch.node, branch_value, out)
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
                        self.encode_framed_element(*child_id, props, field, out)
                    } else {
                        self.encode_element_occurrences(*child_id, props, field, out)
                    }
                } else {
                    write_alignment(out, props)?;
                    write_simple(out, value, *kind, props, self.ctx.strings()).map_err(Into::into)
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
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        for (idx, item) in items.iter().enumerate() {
            self.write_separator(props, out, idx, items.len())?;
            write_alignment(out, props)?;
            if let Some(id) = props.initiator {
                out.extend(encode_delimiter(self.ctx.strings().get(id)?));
            }
            let mut payload = Vec::new();
            self.encode_node(child_id, item, &mut payload)?;
            write_framed_payload(out, &payload, props, self.ctx.strings())?;
        }
        Ok(())
    }

    fn encode_element_occurrences(
        &self,
        node_id: u32,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        for (idx, item) in items.iter().enumerate() {
            self.write_separator(props, out, idx, items.len())?;
            write_alignment(out, props)?;
            self.encode_node(node_id, item, out)?;
        }
        Ok(())
    }

    fn encode_sequence_particle(
        &self,
        node_id: u32,
        map: &BTreeMap<String, DfdlValue>,
        out: &mut Vec<u8>,
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
                if let Some(child_id) = child {
                    let field = match value.field(key) {
                        Some(inner) => inner,
                        None => &value,
                    };
                    if needs_length_frame(&resolved) {
                        self.encode_framed_element(*child_id, &resolved, field, out)
                    } else {
                        self.encode_element_occurrences(*child_id, &resolved, field, out)
                    }
                } else {
                    self.encode_simple_occurrences(*kind, &resolved, &value, out)
                }
            }
            IrNode::Sequence { .. } => {
                self.encode_node(node_id, &DfdlValue::Sequence(map.clone()), out)
            }
            IrNode::Choice { .. } => {
                for (discriminator, value) in map {
                    if branches_contain(self.ctx.program, node_id, discriminator) {
                        return self.encode_node(
                            node_id,
                            &DfdlValue::choice(discriminator.clone(), value.clone()),
                            out,
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
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        for (idx, item) in items.iter().enumerate() {
            self.write_separator(props, out, idx, items.len())?;
            write_alignment(out, props)?;
            write_simple(out, item, kind, props, self.ctx.strings()).map_err(Error::from)?;
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
        index: usize,
        total: usize,
    ) -> Result<()> {
        if !should_emit_separator(props.separator_position, index, total) {
            return Ok(());
        }
        if let Some(id) = props.separator {
            out.extend(encode_delimiter(self.ctx.strings().get(id)?));
        }
        Ok(())
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
    resolved.length = Some(length_from_value(sib_val)?);
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

fn length_from_value(value: &DfdlValue) -> Result<u64> {
    match value {
        DfdlValue::Byte(v) => Ok(*v as u64),
        DfdlValue::UnsignedByte(v) => Ok(*v as u64),
        DfdlValue::Short(v) => u64::try_from(*v).map_err(|_| VmError::InvalidValue {
            message: alloc::format!("negative length `{v}`"),
        })
        .map_err(Into::into),
        DfdlValue::UnsignedShort(v) => Ok(*v as u64),
        DfdlValue::Int(v) => u64::try_from(*v).map_err(|_| VmError::InvalidValue {
            message: alloc::format!("negative length `{v}`"),
        })
        .map_err(Into::into),
        DfdlValue::UnsignedInt(v) => Ok(*v as u64),
        DfdlValue::Long(v) => u64::try_from(*v).map_err(|_| VmError::InvalidValue {
            message: alloc::format!("negative length `{v}`"),
        })
        .map_err(Into::into),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("length sibling has unsupported type: {other:?}"),
        }
        .into()),
    }
}

fn should_emit_separator(position: SeparatorPosition, index: usize, total: usize) -> bool {
    match position {
        SeparatorPosition::Prefix => index < total,
        SeparatorPosition::Infix => index > 0,
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
    fn as_choice_fields(&self) -> Result<(&str, &DfdlValue)>;
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

    fn as_choice_fields(&self) -> Result<(&str, &DfdlValue)> {
        match self {
            DfdlValue::Choice { discriminator, value } => Ok((discriminator, value)),
            _ => Err(VmError::TypeMismatch {
                expected: "choice".into(),
            }
            .into()),
        }
    }
}
