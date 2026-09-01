use super::runtime::{read_simple, Cursor, RuntimeConfig, VmContext};
use alloc::string::ToString;
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram, ValueKind};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::String;

/// DFDL decoder VM — executes compiled IR against an input byte stream.
pub struct Decoder<'a> {
    ctx: VmContext<'a>,
}

impl<'a> Decoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
        }
    }

    /// Decode one logical value from `input`.
    pub fn decode(&self, input: &[u8]) -> Result<DfdlValue> {
        let mut cursor = Cursor::new(input);
        let value = self.decode_node(self.ctx.program.root, &mut cursor)?;
        if self.ctx.config.strict_eos && !cursor.is_empty() {
            return Err(VmError::TrailingData {
                remaining: cursor.remaining(),
            }
            .into());
        }
        Ok(value)
    }

    fn decode_node(&self, node_id: u32, cursor: &mut Cursor<'_>) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id) {
            IrNode::Sequence { children, props: _ } => {
                let mut map = BTreeMap::new();
                for &child in children {
                    let child_value = self.decode_node(child, cursor)?;
                    insert_child(&mut map, child, child_value, self.ctx.program)?;
                }
                Ok(DfdlValue::Sequence(map))
            }
            IrNode::Choice { branches, .. } => {
                for branch in branches {
                    if let Some(init) = &branch.initiator {
                        if !cursor.match_prefix(init) {
                            continue;
                        }
                    }
                    let saved = cursor.clone();
                    if let Ok(value) = self.decode_node(branch.node, cursor) {
                        let name = self.ctx.strings().get(branch.name).to_string();
                        return Ok(DfdlValue::choice(name, value));
                    }
                    *cursor = saved;
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
                    let inner = self.decode_node(*child_id, cursor)?;
                    Ok(wrap_named(
                        self.ctx.strings().get(*name),
                        inner,
                        ValueKind::Complex,
                    ))
                } else {
                    read_simple(cursor, *kind, props).map_err(Into::into)
                }
            }
        }
    }
}

fn insert_child(
    map: &mut BTreeMap<String, DfdlValue>,
    node_id: u32,
    value: DfdlValue,
    program: &IrProgram,
) -> Result<()> {
    match program.node(node_id) {
        IrNode::Element { name, .. } => {
            map.insert(program.strings.get(*name).to_string(), value);
            Ok(())
        }
        IrNode::Sequence { .. } => {
            if let DfdlValue::Sequence(fields) = value {
                for (k, v) in fields {
                    map.insert(k, v);
                }
                Ok(())
            } else {
                Err(VmError::TypeMismatch {
                    expected: "sequence".into(),
                }
                .into())
            }
        }
        IrNode::Choice { .. } => {
            if let DfdlValue::Choice { discriminator, value } = value {
                map.insert(discriminator, *value);
                Ok(())
            } else {
                Err(VmError::TypeMismatch {
                    expected: "choice".into(),
                }
                .into())
            }
        }
    }
}

fn wrap_named(name: &str, inner: DfdlValue, kind: ValueKind) -> DfdlValue {
    if kind == ValueKind::Complex {
        match inner {
            DfdlValue::Sequence(map) => {
                if !map.contains_key(name) {
                    // Flatten anonymous complex wrapper.
                    return DfdlValue::Sequence(map);
                }
                DfdlValue::Sequence(map)
            }
            other => {
                let mut map = BTreeMap::new();
                map.insert(name.into(), other);
                DfdlValue::Sequence(map)
            }
        }
    } else {
        inner
    }
}
