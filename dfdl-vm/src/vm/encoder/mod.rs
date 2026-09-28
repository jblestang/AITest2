mod element;
mod helpers;
mod ovc;
mod sequence;

#[cfg(test)]
mod tests;

use super::alignment::write_leading_skip;
use super::runtime::{
    encoding_name, write_alignment_for_kind, RuntimeConfig, VmContext,
};
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::Cell;

use helpers::*;
use ovc::precompute_output_values;
use sequence::{
    child_skips_encode, optional_sequence_particle_absent,
    sequence_separator_deferred_to_child_occurrences, trailing_empty_absent_position_slot,
};

/// DFDL encoder VM — executes compiled IR to serialize logical values.
pub struct Encoder<'a> {
    pub(crate) ctx: VmContext<'a>,
    /// 1-based index and total count for the nearest enclosing array occurrence (repeat indicators).
    pub(crate) array_occurrence_for_ovc: Cell<Option<(usize, usize)>>,
    pub(crate) seq_bit_order: Cell<Option<crate::schema::BitOrder>>,
}

impl<'a> Encoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
            array_occurrence_for_ovc: Cell::new(None),
            seq_bit_order: Cell::new(None),
        }
    }

    pub(crate) fn check_bit_order_change(&self, props: &IrProps, bit_count: u8) -> Result<()> {
        if !props.bit_order_defined {
            return Ok(());
        }
        let order = props.bit_order;
        if let Some(prev) = self.seq_bit_order.get() {
            if prev != order && !bit_count.is_multiple_of(8) {
                return Err(VmError::InvalidValue {
                    message: "Schema Definition Error: bitOrder change requires byte boundary".into(),
                }
                .into());
            }
        }
        Ok(())
    }

    pub(crate) fn note_bit_order(&self, props: &IrProps) {
        if props.bit_order_defined {
            self.seq_bit_order.set(Some(props.bit_order));
        }
    }

    pub(crate) fn with_array_occurrence<R>(
        &self,
        index_1: usize,
        total: usize,
        f: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        self.array_occurrence_for_ovc.set(Some((index_1, total)));
        let out = f();
        self.array_occurrence_for_ovc.set(None);
        out
    }

    /// Encode `value` and append bytes to `output`. Returns trailing bit count in the last byte.
    pub fn encode(&self, value: &DfdlValue, output: &mut Vec<u8>) -> Result<()> {
        let _ = self.encode_with_bit_count(value, output)?;
        Ok(())
    }

    /// Encode `value` and return the number of significant bits in the last output byte (0 if byte-aligned).
    pub fn encode_with_bit_count(&self, value: &DfdlValue, output: &mut Vec<u8>) -> Result<u8> {
        let value = unwrap_root_for_encode(
            value,
            &self.ctx.program.root_element,
            self.ctx.program.root,
            self.ctx.program,
        );
        let mut bit_count = 0u8;
        self.encode_node(self.ctx.program.root, value, output, &mut bit_count, None)?;
        Ok(bit_count)
    }

    /// Encode into a freshly allocated buffer.
    pub fn encode_to_vec(&self, value: &DfdlValue) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(value, &mut out)?;
        Ok(out)
    }

    pub(crate) fn encode_node(
        &self,
        node_id: u32,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
    ) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, props } => {
                let empty_seq = crate::value::SequenceValue::new(BTreeMap::new());
                let owned_seq;
                let seq = match value {
                    DfdlValue::Sequence(s) => s,
                    DfdlValue::String(text) if text.text.is_empty() => &empty_seq,
                    DfdlValue::Choice { value: inner, .. } => {
                        if let Some(s) = inner.sequence_value() {
                            s
                        } else {
                            return Err(VmError::TypeMismatch {
                                expected: "sequence".into(),
                            }
                            .into());
                        }
                    }
                    DfdlValue::Array(items) if items.len() == 1 => {
                        if let Some(s) = items[0].sequence_value() {
                            s
                        } else {
                            return Err(VmError::TypeMismatch {
                                expected: "sequence".into(),
                            }
                            .into());
                        }
                    }
                    _ => {
                        if let Some(fields) = value.sequence_fields() {
                            owned_seq = crate::value::SequenceValue::new(fields.clone());
                            &owned_seq
                        } else {
                            return Err(VmError::TypeMismatch {
                                expected: "sequence value".into(),
                            }
                            .into());
                        }
                    }
                };
                let map = &seq.fields;
                let scope_lookup = merged_encode_lookup(encode_scope, map);
                let effective = precompute_output_values(self, children, map, props)?;
                let separator_lookup = merged_encode_lookup(Some(&scope_lookup), &effective);
                self.write_initiator(
                    props,
                    out,
                    bit_count,
                    seq.meta.initiator_alt,
                    Some(&separator_lookup),
                )?;
                let mut wrote_particle = false;
                for (idx, &child) in children.iter().enumerate() {
                    if child_skips_encode(self, child)? {
                        continue;
                    }
                    if optional_sequence_particle_absent(self, child, &effective)? {
                        if trailing_empty_absent_position_slot(
                            self, props, child, children, idx, &effective,
                        )? {
                            self.write_sequence_separator(
                                props,
                                out,
                                bit_count,
                                idx,
                                children.len(),
                                &seq.meta,
                                Some(&separator_lookup),
                            )?;
                            wrote_particle = true;
                        }
                        continue;
                    }
                    let defer_sep = sequence_separator_deferred_to_child_occurrences(self, child)?;
                    if !defer_sep {
                        if wrote_particle || props.separator_position == crate::schema::SeparatorPosition::Prefix {
                            self.write_sequence_separator(
                                props,
                                out,
                                bit_count,
                                idx,
                                children.len(),
                                &seq.meta,
                                Some(&separator_lookup),
                            )?;
                        }
                    } else if idx > 0 && props.separator_position == crate::schema::SeparatorPosition::Infix {
                        // Leading infix separator before an unbounded/repeated child (NS_13a).
                        self.write_sequence_separator(
                            props,
                            out,
                            bit_count,
                            idx,
                            children.len(),
                            &seq.meta,
                            Some(&separator_lookup),
                        )?;
                    }
                    self.encode_sequence_particle(
                        child,
                        &effective,
                        props,
                        out,
                        bit_count,
                        &seq.meta,
                        Some(&separator_lookup),
                        children,
                    )?;
                    wrote_particle = true;
                }
                self.write_terminator(
                    props,
                    out,
                    bit_count,
                    seq.meta.terminator_alt,
                    Some(&separator_lookup),
                )?;
                Ok(())
            }
            IrNode::Choice {
                branches,
                props: choice_props,
                ..
            } => {
                if let DfdlValue::Choice {
                    discriminator,
                    value,
                } = value
                {
                    let branch = branches
                        .iter()
                        .find(|b| {
                            self.ctx
                                .strings()
                                .get(b.name)
                                .map(|name| name == discriminator)
                                .unwrap_or(false)
                        })
                        .ok_or(VmError::InvalidChoice {
                            branch_errors: alloc::vec::Vec::new(),
                        })?;
                    return self.encode_choice_matched_branch(
                        choice_props,
                        branch.node,
                        value,
                        &BTreeMap::new(),
                        out,
                        bit_count,
                    );
                }
                if let Some(map) = value.sequence_fields() {
                    for branch in branches {
                        if let Some(key) = choice_branch_data_key(self, branch.node, map) {
                            let val = map
                                .get(&key)
                                .ok_or(VmError::MissingField { name: key.clone() })?;
                            self.write_initiator(choice_props, out, bit_count, None, None)?;
                            return self.encode_choice_matched_branch(
                                choice_props,
                                branch.node,
                                val,
                                map,
                                out,
                                bit_count,
                            );
                        }
                    }
                    if !choice_branches_are_all_hidden(self, branches)? {
                        if let Some(branch) = self.select_choice_branch(branches, map)? {
                            self.write_initiator(choice_props, out, bit_count, None, None)?;
                            let branch_name = self.ctx.strings().get(branch.name)?;
                            let local = crate::xml_util::local_name_str(branch_name);
                            let map_key = map_has_local_key(map, local)
                                .unwrap_or_else(|| branch_name.to_string());
                            let branch_value = map
                                .get(&map_key)
                                .cloned()
                                .unwrap_or_else(|| DfdlValue::sequence(BTreeMap::new()));
                            return self.encode_choice_matched_branch(
                                choice_props,
                                branch.node,
                                &branch_value,
                                map,
                                out,
                                bit_count,
                            );
                        }
                    }
                }
                self.write_initiator(choice_props, out, bit_count, None, None)?;
                for branch in branches {
                    if matches!(
                        self.ctx.program.node(branch.node).ok(),
                        Some(IrNode::Sequence { children, .. }) if children.is_empty()
                    ) {
                        return self.encode_node(
                            branch.node,
                            &DfdlValue::sequence(BTreeMap::new()),
                            out,
                            bit_count,
                            encode_scope,
                        );
                    }
                }
                for branch in branches {
                    if infoset_particle_can_absent_enc(self, branch.node)? {
                        return self.encode_choice_matched_branch(
                            choice_props,
                            branch.node,
                            &DfdlValue::sequence(BTreeMap::new()),
                            value
                                .sequence_fields()
                                .map(|m| m as &BTreeMap<_, _>)
                                .unwrap_or(&BTreeMap::new()),
                            out,
                            bit_count,
                        );
                    }
                }
                for branch in branches {
                    if ir_branch_encodable_without_infoset(self, branch.node)? {
                        return self.encode_choice_matched_branch(
                            choice_props,
                            branch.node,
                            &DfdlValue::sequence(BTreeMap::new()),
                            value
                                .sequence_fields()
                                .map(|m| m as &BTreeMap<_, _>)
                                .unwrap_or(&BTreeMap::new()),
                            out,
                            bit_count,
                        );
                    }
                }
                Err(VmError::InvalidChoice {
                    branch_errors: alloc::vec::Vec::new(),
                }
                .into())
            }
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                if matches!(value, DfdlValue::Null) {
                    return self.encode_nil_element(*kind, props, out, bit_count);
                }
                if let Some(child_id) = child {
                    let name_str = self.ctx.strings().get(*name)?;
                    let field = element_payload_value(value, name_str);
                    if needs_length_frame(props) {
                        if *kind == crate::ir::ValueKind::Complex
                            && matches!(props.length_kind, crate::schema::LengthKind::Explicit | crate::schema::LengthKind::Fixed)
                            && props.length_units == crate::schema::LengthUnits::Characters
                        {
                            let enc = encoding_name(props, self.ctx.strings())?;
                            if crate::vm::encoding::normalize_encoding_name(enc)
                                .is_some_and(|e| matches!(e, "utf-8" | "utf-16be" | "utf-16le"))
                            {
                                return Err(VmError::InvalidValue {
                                    message: alloc::format!(
                                        "Runtime Schema Definition Error. Variable width character lengthKind '{}' with lengthUnits 'characters' not supported for complex types",
                                        match props.length_kind {
                                            crate::schema::LengthKind::Explicit => "explicit",
                                            crate::schema::LengthKind::Fixed => "fixed",
                                            _ => "explicit",
                                        }
                                    ),
                                }
                                .into());
                            }
                        }
                        let schema_ctx = schema_context_field_name(name_str);
                        self.encode_framed_element(
                            *child_id,
                            props,
                            field,
                            out,
                            bit_count,
                            Some(&schema_ctx),
                            None,
                            encode_scope,
                            encode_scope,
                            false,
                        )
                    } else {
                        self.encode_element_occurrences(
                            *child_id,
                            props,
                            field,
                            out,
                            bit_count,
                            props,
                            encode_scope,
                            encode_scope,
                        )
                    }
                } else {
                    write_leading_skip(out, bit_count, props).map_err(Error::from)?;
                    write_alignment_for_kind(
                        out,
                        bit_count,
                        props,
                        *kind,
                        encoding_name(props, self.ctx.strings())?,
                        Some(&self.ctx.config),
                    )
                    .map_err(Error::from)?;
                    self.check_bit_order_change(props, *bit_count)?;
                    let schema_ctx = schema_context_field_name(self.ctx.strings().get(*name)?);
                    let props = encode_scope
                        .map(|scope| {
                            resolve_text_standard_props_for_encode(props, scope, self.ctx.strings())
                        })
                        .transpose()?
                        .unwrap_or_else(|| props.clone());
                    let res = super::runtime::write_simple(
                        out,
                        bit_count,
                        value,
                        *kind,
                        &props,
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        &self.ctx.config,
                        Some(&schema_ctx),
                        None,
                        encode_scope,
                        None,
                    );
                    self.note_bit_order(&props);
                    res.map_err(Into::into)
                }
            }
        }
    }
}
