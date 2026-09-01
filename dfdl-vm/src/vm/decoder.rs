use super::runtime::{default_value_for, read_simple, Cursor, RuntimeConfig, VmContext};
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps, ValueKind};
use crate::schema::{match_delimiter, SeparatorPosition};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::string::String;
use alloc::vec::Vec;

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
        Ok(wrap_root(
            &self.ctx.program.root_element,
            value,
        ))
    }

    fn decode_node(&self, node_id: u32, cursor: &mut Cursor<'_>) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id) {
            IrNode::Sequence { children, props } => {
                let mut map = BTreeMap::new();
                for (idx, &child) in children.iter().enumerate() {
                    if idx > 0 {
                        self.consume_separator(props, cursor)?;
                    }
                    let child_value = self.decode_particle(child, cursor)?;
                    insert_child(&mut map, child, child_value, self.ctx.program)?;
                }
                Ok(DfdlValue::Sequence(map))
            }
            IrNode::Choice { branches, .. } => {
                for branch in branches {
                    if let Some(init_id) = branch.initiator {
                        let pat = self.ctx.strings().get(init_id);
                        if match_delimiter(&cursor.data[cursor.pos..], pat).is_none() {
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
            IrNode::Element { props, .. } => self.decode_element_occurrences(node_id, props, cursor),
        }
    }

    fn decode_particle(&self, node_id: u32, cursor: &mut Cursor<'_>) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id) {
            IrNode::Element { props, .. } => self.decode_element_occurrences(node_id, props, cursor),
            _ => self.decode_node(node_id, cursor),
        }
    }

    fn decode_element_occurrences(
        &self,
        node_id: u32,
        props: &IrProps,
        cursor: &mut Cursor<'_>,
    ) -> Result<DfdlValue> {
        let min = props.occurs_min;
        let max = props.occurs_max.unwrap_or(u64::MAX);
        let mut items = Vec::new();

        while (items.len() as u64) < max {
            if items.len() as u64 >= min && cursor.is_empty() {
                break;
            }
            let saved = cursor.clone();
            match self.decode_single_element(node_id, cursor) {
                Ok(v) => items.push(v),
                Err(e) => {
                    if (items.len() as u64) >= min {
                        *cursor = saved;
                        break;
                    }
                    if let Some(default) = default_value_for(
                        element_kind(self.ctx.program, node_id),
                        props,
                        self.ctx.strings(),
                    ) {
                        items.push(default);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        if (items.len() as u64) < min {
            return Err(VmError::InvalidValue {
                message: alloc::format!("expected at least {min} occurrences, got {}", items.len()),
            }
            .into());
        }

        if items.len() == 1 {
            Ok(items.into_iter().next().unwrap())
        } else {
            Ok(DfdlValue::Array(items))
        }
    }

    fn decode_single_element(&self, node_id: u32, cursor: &mut Cursor<'_>) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id) {
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
                    read_simple(cursor, *kind, props, self.ctx.strings()).map_err(Into::into)
                }
            }
            _ => self.decode_node(node_id, cursor),
        }
    }

    fn consume_separator(&self, props: &IrProps, cursor: &mut Cursor<'_>) -> Result<()> {
        if props.separator_position != SeparatorPosition::Infix {
            return Ok(());
        }
        if let Some(id) = props.separator {
            let pat = self.ctx.strings().get(id);
            if !cursor.consume_delimiter(pat) {
                return Err(VmError::InvalidValue {
                    message: "separator mismatch".into(),
                }
                .into());
            }
        }
        Ok(())
    }
}

fn element_kind(program: &IrProgram, node_id: u32) -> ValueKind {
    match program.node(node_id) {
        IrNode::Element { kind, .. } => *kind,
        _ => ValueKind::Complex,
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

fn wrap_root(name: &str, value: DfdlValue) -> DfdlValue {
    match value {
        DfdlValue::Sequence(map) if map.contains_key(name) => DfdlValue::Sequence(map),
        DfdlValue::Sequence(map) => {
            let mut wrapped = BTreeMap::new();
            wrapped.insert(name.into(), DfdlValue::Sequence(map));
            DfdlValue::Sequence(wrapped)
        }
        other => {
            let mut map = BTreeMap::new();
            map.insert(name.into(), other);
            DfdlValue::Sequence(map)
        }
    }
}

fn wrap_named(name: &str, inner: DfdlValue, kind: ValueKind) -> DfdlValue {
    if kind == ValueKind::Complex {
        match inner {
            DfdlValue::Sequence(map) => {
                if !map.contains_key(name) {
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
