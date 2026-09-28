use super::helpers::*;
use super::ovc::{precompute_output_values, value_for_occurrence_encode};
use super::Encoder;
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProps};
use crate::schema::{
    encode_delimiter, encode_delimiter_by_alt, encode_property_delimiter,
    encode_sequence_separator, LengthKind, OccursCountKind, SeparatorPosition,
    SeparatorSuppressionPolicy, TextPadKind,
};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec::Vec;
use super::super::runtime::{
    encode_framing_delimiter_bytes, encoding_name, is_suppressible_empty_representation,
    resolve_encode_property_pattern, resolve_output_new_line_for_encode,
    validate_explicit_decimal_before_encode, write_alignment_with_config, write_byte_aligned,
};

impl<'a> Encoder<'a> {
    pub(crate) fn select_choice_branch<'b>(
        &self,
        branches: &'b [crate::ir::ChoiceBranch],
        map: &BTreeMap<String, DfdlValue>,
    ) -> Result<Option<&'b crate::ir::ChoiceBranch>> {
        for branch in branches {
            let Some(local) = choice_branch_infoset_local_key_enc(self, branch.node)? else {
                continue;
            };
            if map_has_local_key(map, &local).is_some() {
                return Ok(Some(branch));
            }
        }
        Ok(None)
    }

    pub(crate) fn encode_choice_matched_branch(
        &self,
        choice_props: &IrProps,
        branch_node: u32,
        value: &DfdlValue,
        parent_map: &BTreeMap<String, DfdlValue>,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
    ) -> Result<()> {
        let start = out.len();
        match self.ctx.program.node(branch_node)? {
            IrNode::Element { name, props, .. } => {
                let mut map = parent_map.clone();
                if let Some(fields) = value.sequence_fields() {
                    for (k, v) in fields {
                        map.insert(k.clone(), v.clone());
                    }
                } else if props.output_value_calc.is_none() && !props.hidden {
                    let key = self.ctx.strings().get(*name)?.to_string();
                    map.insert(key, value.clone());
                }
                let meta = crate::value::SequenceMeta::default();
                self.encode_sequence_particle(
                    branch_node,
                    &map,
                    choice_props,
                    out,
                    bit_count,
                    &meta,
                    Some(&map),
                    &[],
                )?;
            }
            _ => {
                let encode_value = if value.sequence_fields().is_some() {
                    value.clone()
                } else if matches!(
                    self.ctx.program.node(branch_node),
                    Ok(IrNode::Sequence { .. })
                ) {
                    DfdlValue::sequence(parent_map.clone())
                } else {
                    value.clone()
                };
                self.encode_node(branch_node, &encode_value, out, bit_count, Some(parent_map))?;
            }
        }
        pad_choice_explicit_frame(self, choice_props, out, bit_count, start)?;
        Ok(())
    }

    pub(crate) fn encode_sequence_particle(
        &self,
        node_id: u32,
        map: &BTreeMap<String, DfdlValue>,
        parent_props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        seq_meta: &crate::value::SequenceMeta,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        sequence_children: &[u32],
    ) -> Result<()> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                let key = self.ctx.strings().get(*name)?;
                let field_delim = seq_meta.field_delimiters.get(key);
                let value_map = merged_encode_lookup(encode_scope, map);
                let mut value = match self.element_encode_value(
                    *kind,
                    props,
                    key,
                    &value_map,
                    sequence_children,
                ) {
                    Ok(v) => v,
                    Err(Error::Vm(VmError::MissingField { .. })) if props.occurs_min == 0 => {
                        return Ok(());
                    }
                    Err(Error::Vm(VmError::MissingField { .. }))
                        if props.length == Some(0)
                            && matches!(
                                props.length_kind,
                                LengthKind::Explicit | LengthKind::Fixed
                            ) =>
                    {
                        DfdlValue::string("")
                    }
                    Err(e) => return Err(e),
                };
                if props.hidden
                    && child.is_none()
                    && matches!(
                        value,
                        DfdlValue::Sequence(ref s) if s.fields.is_empty()
                    )
                {
                    value = DfdlValue::string("");
                }
                value =
                    value_for_occurrence_encode(self, props, value, &value_map, sequence_children)?;
                if props.occurs_count_kind == OccursCountKind::Expression
                    && matches!(value, DfdlValue::Array(ref a) if a.is_empty())
                    && props.occurs_min == 0
                {
                    return Ok(());
                }
                let mut resolved = resolve_length_props_encode(props, &value_map, self.ctx.strings())?;
                resolved = resolve_encoding_for_encode(&resolved, &value_map, self.ctx.strings())?;
                resolved = resolve_byte_order_for_encode(&resolved, &value_map, self.ctx.strings())?;
                resolved =
                    resolve_text_standard_props_for_encode(&resolved, &value_map, self.ctx.strings())?;
                validate_fill_byte_for_encode(&resolved, self.ctx.strings())?;
                validate_explicit_decimal_before_encode(
                    *kind,
                    &resolved,
                    &self.ctx.program.tunables,
                    self.ctx.strings(),
                )?;
                if child.is_none()
                    && is_suppressible_empty_representation(&value, &resolved, self.ctx.strings())?
                {
                    let fixed_len = matches!(
                        resolved.length_kind,
                        LengthKind::Explicit | LengthKind::Fixed
                    ) && resolved.length.is_some_and(|l| l > 0);
                    let pad_empty_string = *kind == crate::ir::ValueKind::String
                        && resolved.text_pad_kind == TextPadKind::PadChar;
                    if fixed_len || pad_empty_string {
                        let schema_ctx = schema_context_field_name(key);
                        return self.encode_simple_occurrences(
                            *kind,
                            &resolved,
                            &value,
                            out,
                            bit_count,
                            Some(&schema_ctx),
                            field_delim,
                            parent_props,
                            Some(&value_map),
                            encode_scope,
                            true,
                        );
                    }
                    if resolved.trailing_skip == 0 {
                        return Ok(());
                    }
                    write_alignment_with_config(out, bit_count, &resolved, Some(&self.ctx.config))
                        .map_err(Error::from)?;
                    crate::vm::alignment::write_trailing_skip(out, bit_count, &resolved)
                        .map_err(Error::from)?;
                    return Ok(());
                }
                if matches!(&value, DfdlValue::Null) {
                    return self.encode_nil_element(*kind, props, out, bit_count);
                }
                if let Some(child_id) = child {
                    let field = element_payload_value(&value, key);
                    let sibling_lookup = merged_encode_lookup(encode_scope, map);
                    if needs_length_frame(&resolved) {
                        let schema_ctx = schema_context_field_name(key);
                        self.encode_framed_element(
                            *child_id,
                            &resolved,
                            field,
                            out,
                            bit_count,
                            Some(&schema_ctx),
                            field_delim,
                            encode_scope,
                            Some(&sibling_lookup),
                            true,
                        )
                    } else {
                        self.encode_element_occurrences(
                            *child_id,
                            &resolved,
                            field,
                            out,
                            bit_count,
                            parent_props,
                            encode_scope,
                            Some(&sibling_lookup),
                        )
                    }
                } else {
                    let schema_ctx = schema_context_field_name(key);
                    self.encode_simple_occurrences(
                        *kind,
                        &resolved,
                        &value,
                        out,
                        bit_count,
                        Some(&schema_ctx),
                        field_delim,
                        parent_props,
                        Some(&value_map),
                        encode_scope,
                        true,
                    )
                }
            }
            IrNode::Sequence {
                children,
                props: inner_props,
            } => {
                let scope_lookup = merged_encode_lookup(encode_scope, map);
                let effective = precompute_output_values(self, children, map, inner_props)?;
                let separator_lookup = merged_encode_lookup(Some(&scope_lookup), &effective);
                self.write_initiator(inner_props, out, bit_count, None, Some(&separator_lookup))?;
                let mut wrote_particle = false;
                for (idx, &child) in children.iter().enumerate() {
                    if child_skips_encode(self, child)? {
                        continue;
                    }
                    if optional_sequence_particle_absent(self, child, &effective)? {
                        if trailing_empty_absent_position_slot(
                            self,
                            inner_props,
                            child,
                            children,
                            idx,
                            &effective,
                        )? {
                            self.write_sequence_separator(
                                inner_props,
                                out,
                                bit_count,
                                idx,
                                children.len(),
                                seq_meta,
                                Some(&separator_lookup),
                            )?;
                            wrote_particle = true;
                        }
                        continue;
                    }
                    let defer_sep = sequence_separator_deferred_to_child_occurrences(self, child)?;
                    if !defer_sep {
                        if wrote_particle
                            || inner_props.separator_position == SeparatorPosition::Prefix
                        {
                            self.write_sequence_separator(
                                inner_props,
                                out,
                                bit_count,
                                idx,
                                children.len(),
                                seq_meta,
                                Some(&separator_lookup),
                            )?;
                        }
                    } else if idx > 0 && inner_props.separator_position == SeparatorPosition::Infix
                    {
                        self.write_sequence_separator(
                            inner_props,
                            out,
                            bit_count,
                            idx,
                            children.len(),
                            seq_meta,
                            Some(&separator_lookup),
                        )?;
                    }
                    self.encode_sequence_particle(
                        child,
                        &effective,
                        inner_props,
                        out,
                        bit_count,
                        seq_meta,
                        Some(&scope_lookup),
                        children,
                    )?;
                    wrote_particle = true;
                }
                self.write_terminator(inner_props, out, bit_count, None, Some(&separator_lookup))?;
                Ok(())
            }
            IrNode::Choice {
                branches,
                props: choice_props,
                ..
            } => {
                for branch in branches {
                    if map_has_local_key(map, "r2").is_some()
                        && matches!(
                            self.ctx.program.node(branch.node).ok(),
                            Some(IrNode::Sequence { children, .. }) if children.is_empty()
                        )
                    {
                        continue;
                    }
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
                            map,
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
                            map,
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
        }
    }

    pub(crate) fn encode_framing_property_pattern(
        &self,
        raw: &str,
        props: &IrProps,
        siblings: Option<&BTreeMap<String, DfdlValue>>,
        alt: Option<u8>,
    ) -> Result<Vec<u8>> {
        let pat = resolve_encode_property_pattern(raw, siblings);
        if let Some(a) = alt {
            return Ok(encode_delimiter_by_alt(&pat, a));
        }
        let encoding = encoding_name(props, self.ctx.strings())?;
        let output_nl = resolve_output_new_line_for_encode(props, siblings, self.ctx.strings())?;
        Ok(encode_framing_delimiter_bytes(
            &pat,
            output_nl.as_deref(),
            encoding,
            None,
        ))
    }

    pub(crate) fn write_initiator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        alt: Option<u8>,
        siblings: Option<&BTreeMap<String, DfdlValue>>,
    ) -> Result<()> {
        if let Some(id) = props.initiator {
            let raw = self.ctx.strings().get(id)?;
            if !raw.is_empty() {
                let bytes = self.encode_framing_property_pattern(raw, props, siblings, alt)?;
                write_byte_aligned(out, bit_count, &bytes).map_err(Error::from)?;
            }
        }
        Ok(())
    }

    pub(crate) fn write_terminator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        alt: Option<u8>,
        siblings: Option<&BTreeMap<String, DfdlValue>>,
    ) -> Result<()> {
        if let Some(id) = props.terminator {
            let raw = self.ctx.strings().get(id)?;
            if !raw.is_empty() {
                let bytes = self.encode_framing_property_pattern(raw, props, siblings, alt)?;
                write_byte_aligned(out, bit_count, &bytes).map_err(Error::from)?;
            }
        }
        Ok(())
    }

    pub(crate) fn write_sequence_separator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        index: usize,
        total: usize,
        meta: &crate::value::SequenceMeta,
        sibling_map: Option<&BTreeMap<String, DfdlValue>>,
    ) -> Result<()> {
        if !should_emit_separator(props.separator_position, index, total, false) {
            return Ok(());
        }
        let Some(id) = props.separator else {
            return Ok(());
        };
        let raw = self.ctx.strings().get(id)?;
        let pat = resolve_encode_property_pattern(raw, sibling_map);
        let pat = pat.as_str();
        let sep_alt = meta.separator_alts.get(index).and_then(|a| *a);
        if let Some(alt) = sep_alt {
            write_byte_aligned(out, bit_count, &encode_delimiter_by_alt(pat, alt))
                .map_err(Error::from)?;
            return Ok(());
        }
        let newline_prefix = meta
            .infix_sep_newline_prefix
            .get(index.saturating_sub(1))
            .copied()
            .unwrap_or(false);
        let output_new_line = props
            .output_new_line
            .map(|id| self.ctx.strings().get(id))
            .transpose()?
            .map(|s| s as &str);
        let bytes = if crate::schema::is_nl_comma_space_pattern(pat) {
            encode_sequence_separator(pat, output_new_line, newline_prefix)
        } else {
            encode_property_delimiter(pat, output_new_line)
        };
        write_byte_aligned(out, bit_count, &bytes).map_err(Error::from)
    }

    pub(crate) fn write_occurrence_separator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        index: usize,
        total: usize,
    ) -> Result<()> {
        self.write_separator_mode(props, out, bit_count, index, total, true)
    }

    pub(crate) fn write_separator_mode(
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
            if *bit_count != 0 {
                if !props.fill_byte_defined {
                    return Err(VmError::InvalidValue {
                        message: "Schema Definition Error: Property fillByte is not defined".into(),
                    }
                    .into());
                }
                while *bit_count != 0 {
                    super::super::runtime::write_stream_bit(
                        out,
                        bit_count,
                        props.fill_byte & 1,
                        props.bit_order,
                    );
                }
            }
            write_byte_aligned(
                out,
                bit_count,
                &encode_delimiter(self.ctx.strings().get(id)?),
            )
            .map_err(Error::from)?;
        }
        Ok(())
    }
}

pub(crate) fn later_sequence_particle_present(
    enc: &Encoder<'_>,
    children: &[u32],
    from_idx: usize,
    map: &BTreeMap<String, DfdlValue>,
) -> Result<bool> {
    for &later in children.iter().skip(from_idx + 1) {
        if child_skips_encode(enc, later)? {
            continue;
        }
        if !optional_sequence_particle_absent(enc, later, map)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn trailing_empty_absent_position_slot(
    enc: &Encoder<'_>,
    seq_props: &IrProps,
    node_id: u32,
    children: &[u32],
    idx: usize,
    map: &BTreeMap<String, DfdlValue>,
) -> Result<bool> {
    if !matches!(
        seq_props.separator_suppression_policy,
        Some(SeparatorSuppressionPolicy::TrailingEmpty)
    ) || seq_props.separator_position != SeparatorPosition::Infix
    {
        return Ok(false);
    }
    let IrNode::Element { props, .. } = enc.ctx.program.node(node_id)? else {
        return Ok(false);
    };
    if props.occurs_min != 0
        || props.occurs_count_kind != OccursCountKind::Implicit
        || props.occurs_max.is_none()
        || props.occurs_max.is_some_and(|m| m > 1)
    {
        return Ok(false);
    }
    later_sequence_particle_present(enc, children, idx, map)
}

pub(crate) fn optional_sequence_particle_absent(
    enc: &Encoder<'_>,
    node_id: u32,
    map: &BTreeMap<String, DfdlValue>,
) -> Result<bool> {
    let IrNode::Element { name, props, .. } = enc.ctx.program.node(node_id)? else {
        return Ok(false);
    };
    if props.occurs_min > 0 || props.output_value_calc.is_some() || props.hidden {
        return Ok(false);
    }
    let key = enc.ctx.strings().get(*name)?;
    Ok(map_has_local_key(map, crate::xml_util::local_name_str(key)).is_none())
}

pub(crate) fn child_skips_encode(enc: &Encoder<'_>, node_id: u32) -> Result<bool> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { props, .. } => Ok(props.input_value_calc.is_some()
            || props.input_value_calc_sibling.is_some()
            || props.input_value_calc_segments.is_some()
            || props.input_value_calc_path.is_some()
            || props.input_value_calc_expression.is_some()),
        _ => Ok(false),
    }
}

pub(crate) fn sequence_separator_deferred_to_child_occurrences(
    enc: &Encoder<'_>,
    child_id: u32,
) -> Result<bool> {
    let IrNode::Element { props, .. } = enc.ctx.program.node(child_id)? else {
        return Ok(false);
    };
    Ok(props.occurs_max.is_none()
        || props.occurs_max.is_some_and(|max| max > 1)
        || props.occurs_min > 1)
}
