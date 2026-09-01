use super::runtime::{write_framed_payload, write_simple, RuntimeConfig, VmContext};
use crate::schema::{encode_delimiter, LengthKind};
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::SeparatorPosition;
use crate::value::DfdlValue;
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
                for (idx, &child) in children.iter().enumerate() {
                    self.write_separator(props, out, idx, children.len())?;
                    self.encode_sequence_child(child, map, out)?;
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
            self.encode_node(node_id, item, out)?;
        }
        Ok(())
    }

    fn encode_sequence_child(
        &self,
        node_id: u32,
        map: &alloc::collections::BTreeMap<alloc::string::String, DfdlValue>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element { name, props, .. } => {
                let key = self.ctx.strings().get(*name)?;
                let value = map
                    .get(key)
                    .ok_or_else(|| VmError::MissingField { name: key.into() })?;
                self.encode_element_occurrences(node_id, props, value, out)
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
    fn as_sequence_fields(
        &self,
    ) -> Result<&alloc::collections::BTreeMap<alloc::string::String, DfdlValue>>;
    fn as_choice_fields(&self) -> Result<(&str, &DfdlValue)>;
}

impl ValueView for DfdlValue {
    fn as_sequence_fields(
        &self,
    ) -> Result<&alloc::collections::BTreeMap<alloc::string::String, DfdlValue>> {
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
