use super::runtime::{write_simple, RuntimeConfig, VmContext};
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram};
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
        self.encode_node(self.ctx.program.root, value, output)
    }

    /// Encode into a freshly allocated buffer.
    pub fn encode_to_vec(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(value, &mut out)?;
        Ok(out)
    }

    fn encode_node(&self, node_id: u32, value: &DfdlValue, out: &mut Vec<u8>) -> Result<()> {
        match self.ctx.program.node(node_id) {
            IrNode::Sequence { children, .. } => {
                let map = value.as_sequence_fields()?;
                for &child in children {
                    self.encode_sequence_child(child, map, out)?;
                }
                Ok(())
            }
            IrNode::Choice { branches, .. } => {
                let (discriminator, branch_value) = value.as_choice_fields()?;
                let branch = branches
                    .iter()
                    .find(|b| self.ctx.strings().get(b.name) == discriminator)
                    .ok_or_else(|| VmError::InvalidChoice)?;
                self.encode_node(branch.node, branch_value, out)
            }
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                if let Some(child_id) = child {
                    let field = value
                        .field(self.ctx.strings().get(*name))
                        .ok_or_else(|| VmError::MissingField {
                            name: self.ctx.strings().get(*name).into(),
                        })?;
                    self.encode_node(*child_id, field, out)
                } else {
                    write_simple(out, value, *kind, props).map_err(Into::into)
                }
            }
        }
    }

    fn encode_sequence_child(
        &self,
        node_id: u32,
        map: &alloc::collections::BTreeMap<alloc::string::String, DfdlValue>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        match self.ctx.program.node(node_id) {
            IrNode::Element { name, .. } => {
                let key = self.ctx.strings().get(*name);
                let value = map
                    .get(key)
                    .ok_or_else(|| VmError::MissingField { name: key.into() })?;
                self.encode_node(node_id, value, out)
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
}

fn branches_contain(program: &IrProgram, node_id: u32, name: &str) -> bool {
    if let IrNode::Choice { branches, .. } = program.node(node_id) {
        branches
            .iter()
            .any(|b| program.strings.get(b.name) == name)
    } else {
        false
    }
}

trait ValueView {
    fn as_sequence_fields(&self) -> Result<&alloc::collections::BTreeMap<alloc::string::String, DfdlValue>>;
    fn as_choice_fields(&self) -> Result<(&str, &DfdlValue)>;
}

impl ValueView for DfdlValue {
    fn as_sequence_fields(&self) -> Result<&alloc::collections::BTreeMap<alloc::string::String, DfdlValue>> {
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
