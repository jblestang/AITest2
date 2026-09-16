use super::runtime::{
    decode_hex_binary, encode_framing_delimiter_bytes, encoding_name, hex_binary_from_integer,
    int_bytes, is_suppressible_empty_representation, nil_unparse_bytes_for_encode,
    resolve_encode_property_pattern, resolve_output_new_line_for_encode, write_alignment,
    write_alignment_for_kind, write_alignment_with_config, write_byte_aligned,
    write_framed_payload, write_simple, validate_explicit_decimal_before_encode,
    trailing_suppressed_count, should_suppress_occurrence_separator, RuntimeConfig, VmContext,
};
use super::alignment::write_leading_skip;
use crate::error::{Error, Result, VmError};
use crate::length_validate::validate_fill_byte_schema;
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::{
    encode_delimiter, encode_delimiter_by_alt, encode_property_delimiter, encode_sequence_separator,
    ByteOrder, ChoiceLengthKind, LengthKind, LengthUnits, OccursCountKind,
    Representation, SeparatorSuppressionPolicy, TextPadKind, OutputValueCalc, SeparatorPosition,
};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

fn schema_context_field_name(local_name: &str) -> String {
    alloc::format!("ex:{local_name}")
}

/// DFDL encoder VM — executes compiled IR to serialize logical values.
pub struct Encoder<'a> {
    ctx: VmContext<'a>,
    /// 1-based index and total count for the nearest enclosing array occurrence (repeat indicators).
    array_occurrence_for_ovc: Cell<Option<(usize, usize)>>,
}

impl<'a> Encoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
            array_occurrence_for_ovc: Cell::new(None),
        }
    }

    fn with_array_occurrence<R>(&self, index_1: usize, total: usize, f: impl FnOnce() -> Result<R>) -> Result<R> {
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
    pub fn encode_with_bit_count(
        &self,
        value: &DfdlValue,
        output: &mut Vec<u8>,
    ) -> Result<u8> {
        let value = unwrap_root_for_encode(
            value,
            &self.ctx.program.root_element,
            self.ctx.program.root,
            &self.ctx.program,
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

    fn select_choice_branch<'b>(
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

    fn encode_choice_matched_branch(
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

    fn encode_node(
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
                        return Err(VmError::TypeMismatch {
                            expected: "sequence".into(),
                        }
                        .into())
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
                            self,
                            props,
                            child,
                            children,
                            idx,
                            &effective,
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
                        if wrote_particle
                            || props.separator_position == SeparatorPosition::Prefix
                        {
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
                    } else if idx > 0 && props.separator_position == SeparatorPosition::Infix {
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
                        Some(&scope_lookup),
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
            IrNode::Choice { branches, props: choice_props, .. } => {
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
                            let val = map.get(&key).ok_or(VmError::MissingField {
                                name: key.clone(),
                            })?;
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
                            && matches!(
                                props.length_kind,
                                LengthKind::Explicit | LengthKind::Fixed
                            )
                            && props.length_units == LengthUnits::Characters
                        {
                            let enc = encoding_name(props, self.ctx.strings())?;
                            if crate::vm::encoding::normalize_encoding_name(&enc)
                                .is_some_and(|e| matches!(e, "utf-8" | "utf-16be" | "utf-16le"))
                            {
                                return Err(VmError::InvalidValue {
                                    message: alloc::format!(
                                        "Runtime Schema Definition Error. Variable width character lengthKind '{}' with lengthUnits 'characters' not supported for complex types",
                                        match props.length_kind {
                                            LengthKind::Explicit => "explicit",
                                            LengthKind::Fixed => "fixed",
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
                    let schema_ctx =
                        schema_context_field_name(self.ctx.strings().get(*name)?);
                    let props = encode_scope
                        .map(|scope| {
                            resolve_text_standard_props_for_encode(
                                props,
                                scope,
                                self.ctx.strings(),
                            )
                        })
                        .transpose()?
                        .unwrap_or_else(|| props.clone());
                    write_simple(
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
        delim_meta: Option<&crate::value::FieldDelimiterMeta>,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        encode_siblings: Option<&BTreeMap<String, DfdlValue>>,
        sequence_particle: bool,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed = trailing_suppressed_count(items, props, self.ctx.strings(), None)?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if !sequence_particle || encode_len > 1 {
                self.write_occurrence_separator(props, out, bit_count, idx, encode_len)?;
            }
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            if let Some(id) = props.initiator {
                let raw = self.ctx.strings().get(id)?;
                if !raw.is_empty() {
                    let bytes = self.encode_framing_property_pattern(
                        raw,
                        props,
                        encode_siblings,
                        delim_meta.and_then(|m| m.initiator_alt),
                    )?;
                    write_byte_aligned(out, bit_count, &bytes).map_err(Error::from)?;
                }
            }
            let mut payload = Vec::new();
            let mut payload_bit_count = 0u8;
            if matches!(item, DfdlValue::Null) {
                let nil_bytes =
                    nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
                write_byte_aligned(&mut payload, &mut payload_bit_count, &nil_bytes)
                    .map_err(Error::from)?;
            } else {
                self.encode_node(
                    child_id,
                    item,
                    &mut payload,
                    &mut payload_bit_count,
                    encode_scope,
                )?;
            }
            write_framed_payload(
                out,
                bit_count,
                &payload,
                payload_bit_count,
                props,
                self.ctx.strings(),
                Some(&self.ctx.config),
                field_name,
                delim_meta,
                encode_siblings,
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
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        encode_siblings: Option<&BTreeMap<String, DfdlValue>>,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed =
            trailing_suppressed_count(items, props, self.ctx.strings(), Some(sep_props))?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if sep_props.separator_position != SeparatorPosition::Postfix {
                if !should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    true,
                    self.ctx.strings(),
                )? {
                    self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
                }
            }
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            self.write_initiator(props, out, bit_count, None, encode_siblings)?;
            if matches!(item, DfdlValue::Null) {
                let nil_bytes =
                    nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
                write_byte_aligned(out, bit_count, &nil_bytes).map_err(Error::from)?;
            } else {
                self.with_array_occurrence(idx + 1, encode_len, || {
                    self.encode_node(node_id, item, out, bit_count, encode_scope)
                })?;
            }
            self.write_terminator(props, out, bit_count, None, encode_siblings)?;
            if sep_props.separator_position == SeparatorPosition::Postfix {
                if !should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    false,
                    self.ctx.strings(),
                )? {
                    self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
                }
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
                value = value_for_occurrence_encode(
                    self,
                    props,
                    value,
                    &value_map,
                    sequence_children,
                )?;
                if props.occurs_count_kind == OccursCountKind::Expression
                    && matches!(value, DfdlValue::Array(ref a) if a.is_empty())
                    && props.occurs_min == 0
                {
                    return Ok(());
                }
                let mut resolved = resolve_length_props_encode(props, map, self.ctx.strings())?;
                resolved = resolve_encoding_for_encode(&resolved, map, self.ctx.strings())?;
                resolved = resolve_byte_order_for_encode(&resolved, map, self.ctx.strings())?;
                resolved =
                    resolve_text_standard_props_for_encode(&resolved, map, self.ctx.strings())?;
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
                            Some(map),
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
                        Some(map),
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
                let effective =
                    precompute_output_values(self, children, map, inner_props)?;
                let separator_lookup = merged_encode_lookup(Some(&scope_lookup), &effective);
                self.write_initiator(
                    inner_props,
                    out,
                    bit_count,
                    None,
                    Some(&separator_lookup),
                )?;
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
                    let defer_sep =
                        sequence_separator_deferred_to_child_occurrences(self, child)?;
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
                    } else if idx > 0
                        && inner_props.separator_position == SeparatorPosition::Infix
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
                self.write_terminator(
                    inner_props,
                    out,
                    bit_count,
                    None,
                    Some(&separator_lookup),
                )?;
                Ok(())
            }
            IrNode::Choice { branches, props: choice_props, .. } => {
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
                        let val = map.get(&key).ok_or(VmError::MissingField {
                            name: key.clone(),
                        })?;
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

    fn encode_simple_occurrences(
        &self,
        kind: crate::ir::ValueKind,
        props: &IrProps,
        value: &DfdlValue,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        field_name: Option<&str>,
        delim_meta: Option<&crate::value::FieldDelimiterMeta>,
        sep_props: &IrProps,
        encode_siblings: Option<&BTreeMap<String, DfdlValue>>,
        encode_scope: Option<&BTreeMap<String, DfdlValue>>,
        sequence_particle: bool,
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed =
            trailing_suppressed_count(items, props, self.ctx.strings(), Some(sep_props))?;
        let encode_len = items.len().saturating_sub(suppressed);
        let empty = BTreeMap::new();
        let sibling_lookup = merged_encode_lookup(
            encode_scope,
            encode_siblings.unwrap_or(&empty),
        );
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            if sep_props.separator_position != SeparatorPosition::Postfix {
                let suppress = should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    true,
                    self.ctx.strings(),
                )? || is_suppressible_empty_representation(item, props, self.ctx.strings())?;
                if !suppress && (!sequence_particle || encode_len > 1) {
                    self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
                }
            }
            if is_suppressible_empty_representation(item, props, self.ctx.strings())? {
                let pad_empty = kind == crate::ir::ValueKind::String
                    && props.text_pad_kind == TextPadKind::PadChar;
                if !pad_empty {
                    continue;
                }
            }
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            write_simple(
                out,
                bit_count,
                item,
                kind,
                props,
                self.ctx.strings(),
                &self.ctx.program.tunables,
                &self.ctx.config,
                field_name,
                delim_meta,
                Some(&sibling_lookup),
            )
            .map_err(Error::from)?;
            if sep_props.separator_position == SeparatorPosition::Postfix {
                if !should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    false,
                    self.ctx.strings(),
                )? && (!sequence_particle || encode_len > 1)
                {
                    self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
                }
            }
        }
        Ok(())
    }

    fn element_encode_value(
        &self,
        kind: crate::ir::ValueKind,
        props: &IrProps,
        key: &str,
        map: &BTreeMap<String, DfdlValue>,
        sequence_children: &[u32],
    ) -> Result<DfdlValue> {
        if props.output_value_calc.is_some() {
            let local = crate::xml_util::local_name_str(key);
            if !ovc_deferred_to_encode_occurrence(props) {
                if let Some(k) = map_has_local_key(map, local) {
                    return Ok(map.get(&k).cloned().unwrap());
                }
                if let Some(v) = map.get(key) {
                    return Ok(v.clone());
                }
            }
            let computed =
                eval_output_value_calc(self, props, map, sequence_children, props)?;
            return Ok(ovc_value_for_element_kind(kind, computed));
        }
        if props.hidden {
            if let Some(default_id) = props.default_value {
                let text = self.ctx.strings().get(default_id)?;
                return Ok(DfdlValue::string(text));
            }
            return Ok(DfdlValue::sequence(BTreeMap::new()));
        }
        if let Some(v) = map.get(key) {
            return Ok(v.clone());
        }
        Err(VmError::MissingField { name: key.into() }.into())
    }

    fn encode_framing_property_pattern(
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

    fn write_initiator(
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

    fn write_terminator(
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

    fn write_sequence_separator(
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
        let pat = super::runtime::resolve_encode_property_pattern(raw, sibling_map);
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
            if *bit_count != 0 {
                if !props.fill_byte_defined {
                    return Err(VmError::InvalidValue {
                        message: "Schema Definition Error: Property fillByte is not defined".into(),
                    }
                    .into());
                }
                while *bit_count != 0 {
                    super::runtime::write_stream_bit(
                        out,
                        bit_count,
                        props.fill_byte & 1,
                        props.bit_order,
                    );
                }
            }
            write_byte_aligned(out, bit_count, &encode_delimiter(self.ctx.strings().get(id)?))
                .map_err(Error::from)?;
        }
        Ok(())
    }

    fn encode_nil_element(
        &self,
        _kind: crate::ir::ValueKind,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
    ) -> Result<()> {
        write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config)).map_err(Error::from)?;
        self.write_initiator(props, out, bit_count, None, None)?;
        let payload = nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
        write_byte_aligned(out, bit_count, &payload).map_err(Error::from)?;
        self.write_terminator(props, out, bit_count, None, None)?;
        Ok(())
    }
}

fn later_sequence_particle_present(
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

/// Positional `trailingEmpty`: absent optional implicit slots before a later present sibling
/// still unparse one zero-length occurrence (infix separator only).
fn trailing_empty_absent_position_slot(
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

fn optional_sequence_particle_absent(
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

fn child_skips_encode(enc: &Encoder<'_>, node_id: u32) -> Result<bool> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { props, .. } => Ok(
            props.input_value_calc.is_some()
                || props.input_value_calc_sibling.is_some()
                || props.input_value_calc_segments.is_some()
                || props.input_value_calc_path.is_some()
                || props.input_value_calc_expression.is_some(),
        ),
        _ => Ok(false),
    }
}

/// When a sequence separator applies between occurrences of one child, the occurrence encoder emits it.
fn sequence_separator_deferred_to_child_occurrences(
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

fn ovc_length_cycle_error(
    enc: &Encoder<'_>,
    children: &[u32],
) -> Result<()> {
    let strings = enc.ctx.strings();
    for &a in children {
        let IrNode::Element {
            name: name_a,
            props: props_a,
            ..
        } = enc.ctx.program.node(a)?
        else {
            continue;
        };
        let Some(ovc_sib) = props_a.output_value_calc_sibling else {
            continue;
        };
        let ovc_sib_name = strings.get(ovc_sib)?;
        for &b in children {
            if a == b {
                continue;
            }
            let IrNode::Element {
                name: name_b,
                props: props_b,
                ..
            } = enc.ctx.program.node(b)?
            else {
                continue;
            };
            let elem_b = strings.get(*name_b)?;
            if elem_b != ovc_sib_name {
                continue;
            }
            if let Some(len_sib) = props_b.length_sibling {
                let len_sib_name = strings.get(len_sib)?;
                let elem_a = strings.get(*name_a)?;
                if len_sib_name == elem_a {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "Unparse Error: Element `{elem_a}` does not have a value, due to a circular dependency between `{elem_a}` and `{elem_b}`"
                        ),
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

struct OvcPrecomputeEntry {
    node_id: u32,
    name_key: alloc::string::String,
    local: alloc::string::String,
    kind: crate::ir::ValueKind,
    props: IrProps,
}

fn collect_ovc_elements_in_sequence_subtree(
    enc: &Encoder<'_>,
    children: &[u32],
    out: &mut Vec<OvcPrecomputeEntry>,
) -> Result<()> {
    for &cid in children {
        match enc.ctx.program.node(cid)? {
            IrNode::Element {
                name,
                kind,
                props,
                child,
                ..
            } => {
                if props.output_value_calc_conditional {
                    let elem = enc.ctx.strings().get(*name)?;
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "Unparse Error: Element `{elem}` does not have a value, due to a circular dependency"
                        ),
                    }
                    .into());
                }
                if props.output_value_calc.is_some() {
                    let key = enc.ctx.strings().get(*name)?.to_string();
                    out.push(OvcPrecomputeEntry {
                        node_id: cid,
                        local: crate::xml_util::local_name_str(&key).to_string(),
                        name_key: key,
                        kind: *kind,
                        props: props.clone(),
                    });
                }
                if let Some(inner) = child {
                    collect_ovc_elements_in_subtree(enc, *inner, out)?;
                }
            }
            IrNode::Sequence { children: nested, .. } => {
                collect_ovc_elements_in_sequence_subtree(enc, nested, out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_ovc_elements_in_subtree(
    enc: &Encoder<'_>,
    node_id: u32,
    out: &mut Vec<OvcPrecomputeEntry>,
) -> Result<()> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Sequence { children, .. } => collect_ovc_elements_in_sequence_subtree(enc, children, out),
        IrNode::Element { child: Some(c), .. } => collect_ovc_elements_in_subtree(enc, *c, out),
        _ => Ok(()),
    }
}

fn count_dfdl_value_for_fn_count(value: &DfdlValue) -> i64 {
    match value {
        DfdlValue::Array(items) => items.len() as i64,
        DfdlValue::Null => 0,
        DfdlValue::Int(n) => *n as i64,
        DfdlValue::Long(n) => *n,
        _ => 1,
    }
}

fn eval_occurs_count_for_encode(
    enc: &Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    map: &BTreeMap<String, DfdlValue>,
    sequence_children: &[u32],
) -> Result<u64> {
    let value = resolve_output_value_calc_path_value(enc, steps, sequence_children, map)?;
    Ok(count_dfdl_value_for_fn_count(&value).max(0) as u64)
}

fn value_for_occurrence_encode(
    enc: &Encoder<'_>,
    props: &IrProps,
    value: DfdlValue,
    map: &BTreeMap<String, DfdlValue>,
    sequence_children: &[u32],
) -> Result<DfdlValue> {
    if props.occurs_count_kind != OccursCountKind::Expression {
        return Ok(value);
    }
    let Some(steps) = props.occurs_count_fn_path.as_ref() else {
        return Ok(value);
    };
    let n = eval_occurs_count_for_encode(enc, steps, map, sequence_children)? as usize;
    if n == 0 {
        return Ok(DfdlValue::Array(alloc::vec::Vec::new()));
    }
    match value {
        DfdlValue::Array(items) if items.len() > n => {
            Ok(DfdlValue::Array(items.into_iter().take(n).collect()))
        }
        other => Ok(other),
    }
}

fn ovc_deferred_to_encode_occurrence(props: &IrProps) -> bool {
    matches!(
        props.output_value_calc,
        Some(OutputValueCalc::OccursIndexPath { .. })
    )
}

fn precompute_output_values<'a>(
    enc: &Encoder<'a>,
    children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
    parent_props: &IrProps,
) -> Result<BTreeMap<String, DfdlValue>> {
    let _ = ovc_length_cycle_error(enc, children);
    let mut ovc_entries = Vec::new();
    collect_ovc_elements_in_sequence_subtree(enc, children, &mut ovc_entries)?;
    let mut effective = map.clone();
    let max_passes = ovc_entries.len().saturating_mul(2).max(4);
    for _pass in 0..max_passes {
        let mut progress = false;
        for entry in &ovc_entries {
            if ovc_deferred_to_encode_occurrence(&entry.props) {
                continue;
            }
            let computed = match eval_output_value_calc(
                enc,
                &entry.props,
                &effective,
                children,
                parent_props,
            ) {
                Ok(v) => v,
                Err(e) => {
                    if matches!(
                        entry.props.output_value_calc,
                        Some(OutputValueCalc::FnError)
                    ) {
                        return Err(e);
                    }
                    continue;
                }
            };
            let computed = ovc_value_for_element_kind(entry.kind, computed);
            let prev = effective.get(&entry.name_key);
            if prev == Some(&computed) {
                continue;
            }
            effective.insert(entry.name_key.clone(), computed);
            progress = true;
        }
        if !progress {
            break;
        }
    }
    for entry in &ovc_entries {
        if ovc_deferred_to_encode_occurrence(&entry.props) {
            continue;
        }
        if effective.get(&entry.name_key).is_none() {
            let elem = &entry.local;
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Unparse Error: Element `{elem}` does not have a value, due to a circular dependency"
                ),
            }
            .into());
        }
    }
    Ok(effective)
}

fn ovc_value_for_element_kind(kind: crate::ir::ValueKind, value: DfdlValue) -> DfdlValue {
    use crate::ir::ValueKind;
    match (kind, value) {
        (ValueKind::String, DfdlValue::Int(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Long(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::Float, DfdlValue::Int(n)) => DfdlValue::Float(n as f32),
        (ValueKind::Float, DfdlValue::Long(n)) => DfdlValue::Float(n as f32),
        (ValueKind::Double, DfdlValue::Int(n)) => DfdlValue::Double(n as f64),
        (ValueKind::Double, DfdlValue::Long(n)) => DfdlValue::Double(n as f64),
        (ValueKind::DateTime | ValueKind::Time, DfdlValue::String(s)) => {
            let norm = crate::vm::calendar_binary::normalize_xs_date_lexical(&s.text)
                .unwrap_or_else(|_| s.text.clone());
            DfdlValue::DateTime(norm)
        }
        (_, v) => v,
    }
}

fn encoded_bit_length(byte_len: usize, bit_count: u8) -> usize {
    byte_len.saturating_mul(8) + bit_count as usize
}

/// Bit length of a value for `dfdl:valueLength(..., 'bits')` (excludes unused high bits in the last byte).
fn encoded_value_length_bits(byte_len: usize, bit_count: u8) -> usize {
    if bit_count == 0 {
        byte_len.saturating_mul(8)
    } else if byte_len == 0 {
        bit_count as usize
    } else {
        byte_len.saturating_sub(1).saturating_mul(8) + bit_count as usize
    }
}

/// `dfdl:valueLength` for delimited simple types: encoded field value only (no initiator/terminator).
fn delimited_value_length_bits_from_encode(
    enc: &Encoder<'_>,
    props: &IrProps,
    buf: &[u8],
    bit_count: u8,
    encode_scope: Option<&BTreeMap<String, DfdlValue>>,
) -> Result<usize> {
    let strings = enc.ctx.strings();
    let encoding = encoding_name(props, strings)?;
    let output_nl = resolve_output_new_line_for_encode(props, encode_scope, strings)?
        .map(|s| s as String);
    let output_nl_ref = output_nl.as_deref();
    let mut total_bits = encoded_value_length_bits(buf.len(), bit_count);
    if let Some(id) = props.initiator {
        let raw = strings.get(id)?;
        let pat = resolve_encode_property_pattern(raw, encode_scope);
        if !pat.is_empty() {
            let bytes =
                encode_framing_delimiter_bytes(&pat, output_nl_ref, &encoding, None);
            total_bits = total_bits.saturating_sub(encoded_bit_length(bytes.len(), 0));
        }
    }
    if let Some(id) = props.terminator {
        let raw = strings.get(id)?;
        let pat = resolve_encode_property_pattern(raw, encode_scope);
        if !pat.is_empty() {
            let bytes =
                encode_framing_delimiter_bytes(&pat, output_nl_ref, &encoding, None);
            total_bits = total_bits.saturating_sub(encoded_bit_length(bytes.len(), 0));
        }
    }
    Ok(total_bits)
}

fn find_particle_by_local_name(
    enc: &Encoder<'_>,
    node_id: u32,
    local: &str,
) -> Result<Option<u32>> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { name, child, .. } => {
            let ename = enc.ctx.strings().get(*name)?;
            if crate::xml_util::local_name_str(ename) == local {
                return Ok(Some(node_id));
            }
            if let Some(cid) = child {
                if let Some(found) = find_particle_by_local_name(enc, *cid, local)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        IrNode::Sequence { children, .. } => {
            for &cid in children {
                if let Some(found) = find_particle_by_local_name(enc, cid, local)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if let Some(found) = find_particle_by_local_name(enc, branch.node, local)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
    }
}

fn root_sequence_children(enc: &Encoder<'_>) -> Result<Vec<u32>> {
    let root = enc.ctx.program.root;
    let IrNode::Element { child: Some(seq_id), .. } = enc.ctx.program.node(root)? else {
        return Ok(Vec::new());
    };
    match enc.ctx.program.node(*seq_id)? {
        IrNode::Sequence { children, .. } => Ok(children.clone()),
        _ => Ok(Vec::new()),
    }
}

fn apply_ovc_path_step_index(
    enc: &Encoder<'_>,
    step: &crate::ir::IrInputPathStep,
    local: &str,
    value: DfdlValue,
) -> Result<DfdlValue> {
    match value {
        DfdlValue::Array(items) => {
            if step.index_from_occurs {
                let (idx, _) = enc.array_occurrence_for_ovc.get().ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!(
                        "outputValueCalc path `{local}[dfdl:occursIndex()]` missing occurrence context"
                    ),
                })?;
                return items
                    .get(idx.saturating_sub(1))
                    .cloned()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: alloc::format!(
                            "outputValueCalc path missing `{local}[dfdl:occursIndex()]`"
                        ),
                    }
                    .into());
            }
            if let Some(n) = step.index {
                return items
                    .get((n as usize).saturating_sub(1))
                    .cloned()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: alloc::format!("outputValueCalc path missing `{local}[{n}]`"),
                    }
                    .into());
            }
        }
        other if !step.index_from_occurs && step.index.is_none() => return Ok(other),
        _ => {}
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("outputValueCalc path invalid index on `{local}`"),
    }
    .into())
}

fn resolve_output_value_calc_path_value(
    enc: &Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
) -> Result<DfdlValue> {
    let strings = enc.ctx.strings();
    let first = steps.first().ok_or_else(|| VmError::InvalidValue {
        message: "empty outputValueCalc path".into(),
    })?;
    let first_local = strings.get(first.local)?;
    let scope = if sequence_children.is_empty() {
        root_sequence_children(enc)?
    } else {
        sequence_children.to_vec()
    };
    let scope = scope.as_slice();
    let mut value = if let Some(k) = map_has_local_key(map, first_local) {
        map.get(&k).cloned().unwrap()
    } else if let Some(cid) = find_particle_by_local_in_children(enc, scope, first_local)? {
        synthesize_element_subtree_value(enc, cid, map, scope)?
    } else if scope != sequence_children {
        if let Some(cid) = find_particle_by_local_in_children(enc, sequence_children, first_local)? {
            synthesize_element_subtree_value(enc, cid, map, sequence_children)?
        } else {
            resolve_path_step_value(enc, scope, map, first_local)?
        }
    } else {
        resolve_path_step_value(enc, scope, map, first_local)?
    };
    if first.index.is_some() || first.index_from_occurs {
        value = apply_ovc_path_step_index(enc, first, first_local, value)?;
    }
    if steps.len() == 1 {
        return Ok(value);
    }
    let root_scope = root_sequence_children(enc)?;
    let mut path_scope_vec = if let Some(b_id) =
        find_particle_by_local_in_children(enc, &root_scope, first_local)?
    {
        inner_sequence_children_of_element(enc, b_id).unwrap_or_else(|| scope.to_vec())
    } else {
        scope.to_vec()
    };
    let mut path_scope = path_scope_vec.as_slice();
    for step in steps.iter().skip(1) {
        let local = strings.get(step.local)?;
        value = match value {
            DfdlValue::Sequence(seq) => {
                let step_map = {
                    let mut merged = map.clone();
                    for (k, v) in &seq.fields {
                        merged.insert(k.clone(), v.clone());
                    }
                    merged
                };
                let child_val = if let Some(k) = map_has_local_key(&seq.fields, local) {
                    seq.fields.get(&k).cloned()
                } else if let Ok(v) = resolve_path_step_value(enc, path_scope, &step_map, local) {
                    Some(v)
                } else if let Ok(Some(cid)) = find_particle_by_local_in_children(enc, path_scope, local)
                {
                    synthesize_element_subtree_value(enc, cid, &step_map, path_scope).ok()
                } else {
                    None
                };
                let child = child_val.as_ref().ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc path missing `{local}`"),
                })?;
                if step.index.is_some() || step.index_from_occurs {
                    apply_ovc_path_step_index(enc, step, local, child.clone())?
                } else {
                    child.clone()
                }
            }
            _ => {
                return Err(VmError::InvalidValue {
                    message: "outputValueCalc path requires sequence".into(),
                }
                .into());
            }
        };
    }
    Ok(value)
}

fn eval_output_infoset_path(
    enc: &Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
) -> Result<i64> {
    let value = resolve_output_value_calc_path_value(enc, steps, sequence_children, map)?;
    numeric_from_dfdl_value(&value)
}

fn inner_sequence_children_of_element(enc: &Encoder<'_>, element_id: u32) -> Option<Vec<u32>> {
    let IrNode::Element { child: Some(c), .. } = enc.ctx.program.node(element_id).ok()? else {
        return None;
    };
    match enc.ctx.program.node(*c).ok()? {
        IrNode::Sequence { children, .. } => Some(children.clone()),
        _ => None,
    }
}

fn find_particle_by_local_in_children(
    enc: &Encoder<'_>,
    children: &[u32],
    local: &str,
) -> Result<Option<u32>> {
    for &cid in children {
        if let Some(found) = find_particle_by_local_name(enc, cid, local)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn find_particle_for_ovc_path(
    enc: &Encoder<'_>,
    scope_children: &[u32],
    steps: &[crate::ir::IrInputPathStep],
) -> Result<Option<u32>> {
    let strings = enc.ctx.strings();
    let first = steps.first().ok_or_else(|| VmError::InvalidValue {
        message: "empty outputValueCalc path".into(),
    })?;
    let first_local = strings.get(first.local)?;
    let mut current = match find_particle_by_local_in_children(enc, scope_children, first_local)? {
        Some(id) => id,
        None => return Ok(None),
    };
    for step in steps.iter().skip(1) {
        let local = strings.get(step.local)?;
        current = match find_particle_by_local_name(enc, current, local)? {
            Some(id) => id,
            None => return Ok(None),
        };
    }
    Ok(Some(current))
}

fn resolve_path_step_value(
    enc: &Encoder<'_>,
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
    local: &str,
) -> Result<DfdlValue> {
    if let Some(k) = map_has_local_key(map, local) {
        return Ok(map.get(&k).cloned().unwrap());
    }
    for &child_id in sequence_children {
        let IrNode::Element { name, props, .. } = enc.ctx.program.node(child_id)? else {
            continue;
        };
        let ename = enc.ctx.strings().get(*name)?;
        if crate::xml_util::local_name_str(ename) != crate::xml_util::local_name_str(local) {
            continue;
        }
        if props.output_value_calc.is_some() {
            return eval_output_value_calc(enc, props, map, sequence_children, props);
        }
        if let IrNode::Element { child: Some(cid), .. } = enc.ctx.program.node(child_id)? {
            return synthesize_element_subtree_value(enc, *cid, map, sequence_children);
        }
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("outputValueCalc path missing `{local}`"),
    }
    .into())
}

fn synthesize_element_subtree_value(
    enc: &Encoder<'_>,
    node_id: u32,
    map: &BTreeMap<String, DfdlValue>,
    sequence_children: &[u32],
) -> Result<DfdlValue> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Sequence {
            children,
            props: seq_props,
        } => {
            let mut effective = precompute_output_values(enc, children, map, seq_props)?;
            for &cid in children {
                let IrNode::Element { name, .. } = enc.ctx.program.node(cid)? else {
                    continue;
                };
                let key = enc.ctx.strings().get(*name)?;
                if map_has_local_key(&effective, crate::xml_util::local_name_str(key)).is_some() {
                    continue;
                }
                let val = synthesize_element_subtree_value(enc, cid, map, sequence_children)?;
                effective.insert(key.to_string(), val);
            }
            Ok(DfdlValue::Sequence(crate::value::SequenceValue::new(effective)))
        }
        IrNode::Element {
            name,
            kind: _,
            props,
            child,
        } => {
            let key = enc.ctx.strings().get(*name)?;
            if let Some(k) = map_has_local_key(map, crate::xml_util::local_name_str(key)) {
                return Ok(map.get(&k).cloned().unwrap());
            }
            if props.output_value_calc.is_some() {
                return eval_output_value_calc(enc, props, map, sequence_children, props);
            }
            if let Some(cid) = child {
                return synthesize_element_subtree_value(enc, *cid, map, sequence_children);
            }
            Err(VmError::InvalidValue {
                message: alloc::format!("outputValueCalc path missing `{key}`"),
            }
            .into())
        }
        _ => Err(VmError::InvalidValue {
            message: "outputValueCalc path unsupported particle".into(),
        }
        .into()),
    }
}

fn numeric_from_dfdl_value(value: &DfdlValue) -> Result<i64> {
    match value {
        DfdlValue::Int(v) => Ok(*v as i64),
        DfdlValue::Long(v) => Ok(*v),
        DfdlValue::Byte(v) => Ok(*v as i64),
        DfdlValue::Short(v) => Ok(*v as i64),
        DfdlValue::Integer(s) => s.parse::<i64>().map_err(|_| {
            VmError::InvalidValue {
                message: alloc::format!("invalid integer `{s}`"),
            }
            .into()
        }),
        _ => Err(VmError::InvalidValue {
            message: "outputValueCalc path numeric required".into(),
        }
        .into()),
    }
}

fn map_has_local_key(map: &BTreeMap<String, DfdlValue>, local: &str) -> Option<String> {
    for key in map.keys() {
        if crate::xml_util::local_name_str(key) == local {
            return Some(key.clone());
        }
    }
    None
}

fn ir_props_encodable_without_infoset(props: &IrProps) -> bool {
    if props.input_value_calc.is_some()
        || props.input_value_calc_sibling.is_some()
        || props.input_value_calc_segments.is_some()
        || props.input_value_calc_path.is_some()
    {
        return false;
    }
    props.output_value_calc.is_some()
        || props.output_value_calc_literal.is_some()
        || props.output_value_calc_sibling.is_some()
        || props.output_value_calc_conditional
        || props.occurs_min == 0
}

fn infoset_particle_can_absent_enc(enc: &Encoder<'_>, node_id: u32) -> Result<bool> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element {
            props,
            child,
            kind,
            ..
        } => {
            if ir_props_encodable_without_infoset(props) {
                return Ok(true);
            }
            if *kind == crate::ir::ValueKind::Complex {
                if let Some(child_id) = child {
                    return infoset_particle_can_absent_enc(enc, *child_id);
                }
            }
            Ok(false)
        }
        IrNode::Sequence { children, .. } => {
            for &cid in children {
                if !infoset_particle_can_absent_enc(enc, cid)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if infoset_particle_can_absent_enc(enc, branch.node)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn ir_branch_encodable_without_infoset(enc: &Encoder<'_>, branch_node: u32) -> Result<bool> {
    match enc.ctx.program.node(branch_node)? {
        IrNode::Element {
            props,
            child,
            kind,
            ..
        } => {
            if ir_props_encodable_without_infoset(props) {
                return Ok(true);
            }
            if *kind == crate::ir::ValueKind::Complex {
                if let Some(child_id) = child {
                    return ir_branch_encodable_without_infoset(enc, *child_id);
                }
            }
            Ok(false)
        }
        IrNode::Sequence { children, .. } => {
            if children.is_empty() {
                return Ok(true);
            }
            for &cid in children {
                if !ir_branch_encodable_without_infoset(enc, cid)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if ir_branch_encodable_without_infoset(enc, branch.node)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn choice_branches_are_all_hidden(
    enc: &Encoder<'_>,
    branches: &[crate::ir::ChoiceBranch],
) -> Result<bool> {
    for branch in branches {
        let hidden = match enc.ctx.program.node(branch.node)? {
            IrNode::Element { props, .. } => props.hidden,
            _ => false,
        };
        if !hidden {
            return Ok(false);
        }
    }
    Ok(!branches.is_empty())
}

fn choice_branch_infoset_local_key_enc(enc: &Encoder<'_>, branch_node: u32) -> Result<Option<String>> {
    match enc.ctx.program.node(branch_node)? {
        IrNode::Element { name, props, .. } => {
            if ir_props_encodable_without_infoset(props) {
                return Ok(None);
            }
            let ename = enc.ctx.strings().get(*name)?;
            Ok(Some(crate::xml_util::local_name_str(ename).to_string()))
        }
        IrNode::Sequence { children, .. } => {
            for &cid in children {
                if let Some(k) = choice_branch_infoset_local_key_enc(enc, cid)? {
                    return Ok(Some(k));
                }
            }
            Ok(None)
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if let Some(k) = choice_branch_infoset_local_key_enc(enc, branch.node)? {
                    return Ok(Some(k));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn choice_branch_data_key(
    enc: &Encoder<'_>,
    branch_node: u32,
    map: &BTreeMap<String, DfdlValue>,
) -> Option<String> {
    match enc.ctx.program.node(branch_node).ok()? {
        IrNode::Element { name, props, .. } => {
            if props.hidden {
                return None;
            }
            let ename = enc.ctx.strings().get(*name).ok()?;
            map_has_local_key(map, crate::xml_util::local_name_str(ename))
        }
        IrNode::Sequence { children, .. } => {
            for &cid in children {
                if let Some(k) = choice_branch_data_key(enc, cid, map) {
                    return Some(k);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_descendant_element_by_local(
    enc: &Encoder<'_>,
    node_id: u32,
    local_name: &str,
) -> Result<Option<u32>> {
    let sib_local = crate::xml_util::local_name_str(local_name);
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { name, child, .. } => {
            let n = enc.ctx.strings().get(*name)?;
            let n_local = crate::xml_util::local_name_str(n);
            if n == local_name || n_local == local_name || n_local == sib_local {
                return Ok(Some(node_id));
            }
            if let Some(c) = child {
                if let Some(found) = find_descendant_element_by_local(enc, *c, local_name)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        IrNode::Sequence { children, .. } => {
            for &c in children {
                if let Some(found) = find_descendant_element_by_local(enc, c, local_name)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn find_child_element_by_name(
    enc: &Encoder<'_>,
    children: &[u32],
    local_name: &str,
) -> Result<Option<u32>> {
    for &child in children {
        if let Some(found) = find_descendant_element_by_local(enc, child, local_name)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn value_length_encode_target(enc: &Encoder<'_>, node_id: u32) -> Result<u32> {
    let mut current = node_id;
    for _ in 0..4 {
        match enc.ctx.program.node(current)? {
            IrNode::Element {
                kind: crate::ir::ValueKind::Complex,
                child: Some(child_id),
                ..
            } => {
                current = *child_id;
            }
            IrNode::Sequence { .. } => return Ok(current),
            _ => return Ok(current),
        }
    }
    Ok(current)
}

fn measure_value_length(
    enc: &Encoder<'_>,
    node_id: u32,
    value: &DfdlValue,
    units: LengthUnits,
    encode_scope: Option<&BTreeMap<String, DfdlValue>>,
) -> Result<usize> {
    if let Ok(IrNode::Element { props, .. }) = enc.ctx.program.node(node_id) {
        if props.object_kind == crate::schema::ObjectKind::Bytes {
            let len = blob_value_byte_len(value)?;
            return length_in_units(len, units);
        }
    }
    if let Ok(IrNode::Element {
        kind,
        child,
        props,
        ..
    }) = enc.ctx.program.node(node_id)
    {
        if child.is_none()
            && props.length_kind == LengthKind::Delimited
            && props.escape_scheme.is_none()
            && matches!(
                kind,
                crate::ir::ValueKind::String
                    | crate::ir::ValueKind::Decimal
                    | crate::ir::ValueKind::Integer
            )
        {
            let bytes = value_byte_length(value)?;
            return match units {
                LengthUnits::Bits => Ok(bytes.saturating_mul(8)),
                LengthUnits::Bytes => Ok(bytes),
                LengthUnits::Characters => Err(VmError::UnsupportedOperation {
                    op: "outputValueCalc character units".into(),
                }
                .into()),
            };
        }
    }
    let mut buf = Vec::new();
    let mut bit_count = 0u8;
    let encode_id = if value.sequence_fields().is_some() {
        value_length_encode_target(enc, node_id)?
    } else {
        node_id
    };
    enc.encode_node(encode_id, value, &mut buf, &mut bit_count, encode_scope)?;
    let value_bits = if let Ok(IrNode::Element {
        props,
        child: None,
        ..
    }) = enc.ctx.program.node(encode_id)
    {
        if props.length_kind == LengthKind::Delimited {
            delimited_value_length_bits_from_encode(
                enc,
                props,
                &buf,
                bit_count,
                encode_scope,
            )?
        } else {
            encoded_value_length_bits(buf.len(), bit_count)
        }
    } else {
        encoded_value_length_bits(buf.len(), bit_count)
    };
    match units {
        LengthUnits::Bits => Ok(value_bits),
        LengthUnits::Bytes => Ok((value_bits + 7) / 8),
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "outputValueCalc character units".into(),
        }
        .into()),
    }
}

fn apply_output_value_calc_scale(props: &IrProps, len: i64) -> i64 {
    len.saturating_mul(props.output_value_calc_scale.unwrap_or(1))
}

fn split_top_level_commas_ovc(s: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut out = alloc::vec::Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim().to_string());
    out
}

fn ovc_error_unquote_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return Some(s[1..s.len() - 1].replace("''", "'"));
    }
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return Some(s[1..s.len() - 1].replace("\"\"", "\""));
    }
    None
}

fn eval_fn_error_ovc(
    expr: &str,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<DfdlValue> {
    let expr = expr.trim();
    let error_call = if let Some(rest) = expr.strip_prefix("fn:round-half-to-even(") {
        rest.strip_suffix(')').unwrap_or(expr).trim()
    } else {
        expr
    };
    if error_call == "fn:error()" {
        return Err(VmError::InvalidValue {
            message: "Unparse Error: xqt-errors#FOER0000".into(),
        }
        .into());
    }
    let args_body = error_call
        .strip_prefix("fn:error(")
        .and_then(|a| a.strip_suffix(')'))
        .ok_or_else(|| VmError::InvalidValue {
            message: "invalid fn:error in outputValueCalc".into(),
        })?;
    let mut parts = alloc::vec!["Unparse Error".to_string()];
    for arg in split_top_level_commas_ovc(args_body) {
        let arg = arg.trim();
        if let Some(lit) = ovc_error_unquote_literal(arg) {
            parts.push(lit);
        } else if let Some(path) = arg.strip_prefix("../") {
            let local = path.rsplit(':').next().unwrap_or(path).trim();
            let text = lookup_sibling_string_in_encode_map(map, local)?;
            parts.push(text);
        } else {
            return Err(VmError::InvalidValue {
                message: alloc::format!("unsupported fn:error argument `{arg}`"),
            }
            .into());
        }
    }
    Err(VmError::InvalidValue {
        message: parts.join(": "),
    }
    .into())
}

fn eval_ovc_concat_segments(
    enc: &Encoder<'_>,
    segments: &[crate::ir::IrInputValueCalcSegment],
    map: &BTreeMap<String, DfdlValue>,
    children: &[u32],
) -> Result<String> {
    use crate::ir::IrInputValueCalcSegment;
    let strings = enc.ctx.strings();
    let mut out = String::new();
    for seg in segments {
        match seg {
            IrInputValueCalcSegment::Sibling(id) => {
                let name = strings.get(*id)?;
                let local = crate::xml_util::local_name_str(name);
                let text = lookup_sibling_string_in_encode_map(map, local)?;
                out.push_str(&text);
            }
            IrInputValueCalcSegment::Literal(id) => out.push_str(strings.get(*id)?),
            IrInputValueCalcSegment::Substring {
                sibling,
                start,
                length,
            } => {
                let name = strings.get(*sibling)?;
                let local = crate::xml_util::local_name_str(name);
                let text = lookup_sibling_string_in_encode_map(map, local)?;
                let start0 = (*start as usize).saturating_sub(1);
                for ch in text.chars().skip(start0).take(*length as usize) {
                    out.push(ch);
                }
            }
            IrInputValueCalcSegment::InfosetPath(steps) => {
                let v = resolve_output_value_calc_path_value(enc, steps, children, map)?;
                out.push_str(&dfdl_value_to_string_fragment(&v));
            }
            IrInputValueCalcSegment::ValueLength { sibling, units } => {
                let sib_name = strings.get(*sibling)?;
                let sib = sibling_from_map(Some(*sibling), map, strings)?;
                let len = if let Some(child_id) = find_child_element_by_name(enc, children, sib_name)? {
                    measure_value_length(enc, child_id, sib, *units, Some(map))?
                } else {
                    length_in_units(value_byte_length(sib)?, *units)?
                };
                out.push_str(&len.to_string());
            }
        }
    }
    Ok(out)
}

fn dfdl_value_to_string_fragment(value: &DfdlValue) -> String {
    match value {
        DfdlValue::String(s) => s.text.clone(),
        DfdlValue::Int(n) => n.to_string(),
        DfdlValue::Long(n) => n.to_string(),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.clone(),
        other => alloc::format!("{other:?}"),
    }
}

fn eval_output_value_calc(
    enc: &Encoder<'_>,
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    children: &[u32],
    parent_props: &IrProps,
) -> Result<DfdlValue> {
    let strings = enc.ctx.strings();
    let calc = props.output_value_calc.ok_or_else(|| VmError::InvalidValue {
        message: "missing outputValueCalc".into(),
    })?;
    match calc {
        OutputValueCalc::FnError => {
            let lit_id = props.output_value_calc_literal.ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc fn:error".into(),
            })?;
            let expr = strings.get(lit_id)?;
            return eval_fn_error_ovc(expr, map, strings);
        }
        OutputValueCalc::FnConcat => {
            let segments = props.output_value_calc_segments.as_ref().ok_or_else(|| {
                VmError::InvalidValue {
                    message: "missing outputValueCalc fn:concat segments".into(),
                }
            })?;
            let text = eval_ovc_concat_segments(enc, segments, map, children)?;
            return Ok(DfdlValue::string(text));
        }
        OutputValueCalc::HexBinaryFromLexical => {
            let lit_id = props.output_value_calc_literal.ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc hex literal".into(),
            })?;
            let text = strings.get(lit_id)?;
            let bytes = decode_hex_binary(text)?;
            return Ok(DfdlValue::HexBinary(bytes));
        }
        OutputValueCalc::HexBinaryFromInteger(n) => {
            let width = props.length.map(|l| l as usize);
            let bytes = if n >= 0 {
                decode_hex_binary(&dfdl_hex_binary_lexical_from_integer(n))?
            } else if let Some(w) = width {
                let min_w = minimal_signed_byte_width(n);
                if w == min_w {
                    hex_binary_from_integer(n, Some(w))
                } else {
                    decode_hex_binary(&dfdl_hex_binary_lexical_from_integer(n))?
                }
            } else {
                decode_hex_binary(&dfdl_hex_binary_lexical_from_integer(n))?
            };
            return Ok(DfdlValue::HexBinary(bytes));
        }
        OutputValueCalc::HexBinaryFromShort(v) => {
            return Ok(DfdlValue::HexBinary(int_bytes(i64::from(v), 2, false)));
        }
        OutputValueCalc::InfosetPathAddend => {
            let steps = props.output_value_calc_path.as_ref().ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc path".into(),
            })?;
            let addend = props.output_value_calc_path_addend.unwrap_or(0);
            if addend == 0 {
                return Ok(resolve_output_value_calc_path_value(enc, steps, children, map)?);
            }
            let base = eval_output_infoset_path(enc, steps, children, map)?;
            let len = base.saturating_add(addend);
            return Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
            })?));
        }
        OutputValueCalc::FnCountPath => {
            let steps = props.output_value_calc_path.as_ref().ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc fn:count path".into(),
            })?;
            let value = resolve_output_value_calc_path_value(enc, steps, children, map)?;
            let n = count_dfdl_value_for_fn_count(&value);
            return Ok(DfdlValue::Int(i32::try_from(n).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("fn:count result `{n}` out of range for int"),
            })?));
        }
        OutputValueCalc::OccursIndexPath { multiply } => {
            let (idx, _) = enc.array_occurrence_for_ovc.get().ok_or_else(|| VmError::InvalidValue {
                message: "occursIndex outputValueCalc missing occurrence context".into(),
            })?;
            let idx = idx as i64;
            let addend = props.output_value_calc_path_addend.unwrap_or(0);
            let steps = props.output_value_calc_path.as_deref().unwrap_or(&[]);
            let v = if steps.is_empty() {
                if multiply {
                    idx
                } else {
                    idx.saturating_add(addend)
                }
            } else {
                let path_val = eval_output_infoset_path(enc, steps, children, map)?;
                if multiply {
                    idx.saturating_mul(path_val)
                } else {
                    idx.saturating_add(path_val).saturating_add(addend)
                }
            };
            return Ok(DfdlValue::Int(i32::try_from(v).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("outputValueCalc result `{v}` out of range for int"),
            })?));
        }
        OutputValueCalc::ValueLengthInfosetPath(units, addend) => {
            let steps = props.output_value_calc_path.as_ref().ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc path".into(),
            })?;
            let val = resolve_output_value_calc_path_value(enc, steps, children, map)?;
            let last = steps.last().ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc path".into(),
            })?;
            let child_id = find_particle_for_ovc_path(enc, children, steps)?.ok_or_else(|| {
                let last_local = strings.get(last.local).unwrap_or("?");
                VmError::InvalidValue {
                    message: alloc::format!(
                        "outputValueCalc sibling `{last_local}` not available"
                    ),
                }
            })?;
            let len = apply_output_value_calc_scale(
                props,
                measure_value_length(enc, child_id, &val, units, Some(map))? as i64 + addend,
            );
            return Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| VmError::InvalidValue {
                message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
            })?));
        }
        OutputValueCalc::HexBinaryFromByteSibling => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let byte = match sib {
                DfdlValue::Byte(v) => *v,
                DfdlValue::Int(v) => i8::try_from(*v).map_err(|_| VmError::InvalidValue {
                    message: alloc::format!("value `{v}` out of range for byte"),
                })?,
                DfdlValue::String(s) => s.text.parse::<i8>().map_err(|_| VmError::InvalidValue {
                    message: alloc::format!("invalid byte `{text}`", text = s.text),
                })?,
                other => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("dfdl:hexBinary(xs:byte(...)) on `{other:?}`"),
                    }
                    .into());
                }
            };
            return Ok(DfdlValue::HexBinary(alloc::vec![byte as u8]));
        }
        _ => {}
    }

    let len = match calc {
        OutputValueCalc::Constant(v) => {
            if let Some(lit_id) = props.output_value_calc_literal {
                let text = strings.get(lit_id)?;
                return Ok(DfdlValue::string(text));
            }
            v
        }
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
        OutputValueCalc::StringLengthSibling => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let len = match sib {
                DfdlValue::String(s) => s.text.chars().count(),
                DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.chars().count(),
                other => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("string-length on unsupported value `{other:?}`"),
                    }
                    .into());
                }
            } as i64;
            len
        }
        OutputValueCalc::ValueLengthSibling(units, addend) => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let sib_name = strings.get(
                props
                    .output_value_calc_sibling
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "outputValueCalc sibling missing".into(),
                    })?,
            )?;
            if let Some(child_id) = find_child_element_by_name(enc, children, sib_name)? {
                apply_output_value_calc_scale(
                    props,
                    measure_value_length(enc, child_id, sib, units, Some(map))? as i64 + addend,
                )
            } else {
                apply_output_value_calc_scale(
                    props,
                    length_in_units(value_byte_length(sib)?, units)? as i64 + addend,
                )
            }
        }
        OutputValueCalc::Substring { start, length } => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let text = match sib {
                DfdlValue::String(s) => s.text.as_str(),
                DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.as_str(),
                other => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("substring on unsupported value `{other:?}`"),
                    }
                    .into());
                }
            };
            let start0 = start.saturating_sub(1);
            let slice: String = text.chars().skip(start0).take(length).collect();
            return Ok(DfdlValue::string(slice));
        }
        OutputValueCalc::RepeatIndicatorFromParentCount => {
            let (idx, total) = enc.array_occurrence_for_ovc.get().ok_or_else(|| {
                VmError::InvalidValue {
                    message: "repeatIndicator outputValueCalc missing occurrence context".into(),
                }
            })?;
            let v = if idx < total { 1 } else { 0 };
            return Ok(DfdlValue::Int(v));
        }
        OutputValueCalc::HexBinaryFromLexical
        | OutputValueCalc::HexBinaryFromInteger(_)
        | OutputValueCalc::HexBinaryFromShort(_)
        | OutputValueCalc::HexBinaryFromByteSibling
        | OutputValueCalc::InfosetPathAddend
        |         OutputValueCalc::OccursIndexPath { .. }
        | OutputValueCalc::FnCountPath
        | OutputValueCalc::FnConcat
        | OutputValueCalc::FnError
        | OutputValueCalc::ValueLengthInfosetPath(_, _) => {
            unreachable!("handled above")
        }
    };
    Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| VmError::InvalidValue {
        message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
    })?))
}

fn minimal_signed_byte_width(n: i64) -> usize {
    for w in 1..=8usize {
        let mask = if w >= 8 {
            u64::MAX
        } else {
            (1u64 << (w * 8)) - 1
        };
        let raw = hex_binary_from_integer(n, Some(w));
        let mut v = 0i64;
        for &b in &raw {
            v = (v << 8) | i64::from(b);
        }
        if w < 8 {
            let sign = 1i64 << (w * 8 - 1);
            if v & sign != 0 {
                v |= !mask as i64;
            }
        }
        if v == n {
            return w;
        }
    }
    8
}

fn dfdl_hex_binary_lexical_from_integer(n: i64) -> String {
    if n >= 0 {
        let s = alloc::format!("{n:x}");
        if s.len() % 2 == 1 {
            return alloc::format!("0{s}");
        }
        return s;
    }
    for w in 1..=8usize {
        let raw = hex_binary_from_integer(n, Some(w));
        let s: String = raw
            .iter()
            .map(|b| alloc::format!("{:02x}", b))
            .collect();
        let trimmed = s.trim_start_matches('0');
        if trimmed.is_empty() {
            return "00".into();
        }
        if trimmed.len() % 2 == 1 {
            return alloc::format!("0{trimmed}");
        }
        return trimmed.to_string();
    }
    "00".into()
}

fn parse_sibling_property_expr(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let path = inner.strip_prefix("../")?;
    Some(
        path.rsplit(':')
            .next()
            .unwrap_or(path)
            .trim()
            .to_string(),
    )
}

fn lookup_sibling_string_in_encode_map(
    map: &BTreeMap<String, DfdlValue>,
    sibling: &str,
) -> Result<String> {
    let sib_val = map
        .iter()
        .find(|(k, _)| crate::xml_util::local_name_str(k) == sibling)
        .map(|(_, v)| v)
        .or_else(|| map.get(sibling))
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("property sibling `{sibling}` not available"),
        })?;
    match sib_val {
        DfdlValue::String(s) => Ok(s.text.clone()),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => Ok(s.clone()),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("property sibling must be string, got `{other:?}`"),
        }
        .into()),
    }
}

fn resolve_text_standard_props_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<IrProps> {
    let mut resolved = props.clone();
    if let Some(sib_id) = props.text_standard_decimal_separator_sibling {
        let local = strings.get(sib_id)?;
        resolved.resolved_text_standard_decimal_separator =
            Some(lookup_sibling_string_in_encode_map(map, local)?);
    }
    if let Some(sib_id) = props.text_standard_grouping_separator_sibling {
        let local = strings.get(sib_id)?;
        resolved.resolved_text_standard_grouping_separator =
            Some(lookup_sibling_string_in_encode_map(map, local)?);
    }
    if let Some(sib_id) = props.text_standard_exponent_rep_sibling {
        let local = strings.get(sib_id)?;
        resolved.resolved_text_standard_exponent_rep =
            Some(lookup_sibling_string_in_encode_map(map, local)?);
    }
    Ok(resolved)
}

fn resolve_byte_order_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<IrProps> {
    let Some(test_id) = props.byte_order_conditional_test else {
        return Ok(props.clone());
    };
    let raw = strings.get(test_id)?;
    let Some(sibling) = parse_sibling_property_expr(raw) else {
        return Ok(props.clone());
    };
    let text = lookup_sibling_string_in_encode_map(map, &sibling)?;
    let order = match text.as_str() {
        "bigEndian" => ByteOrder::BigEndian,
        "littleEndian" => ByteOrder::LittleEndian,
        other => {
            return Err(VmError::InvalidValue {
                message: alloc::format!("unknown byteOrder `{other}`"),
            }
            .into());
        }
    };
    let mut resolved = props.clone();
    resolved.byte_order = order;
    resolved.byte_order_defined = true;
    Ok(resolved)
}

fn resolve_encoding_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<IrProps> {
    let raw = strings.get(props.encoding)?;
    let Some(sibling) = parse_sibling_property_expr(raw) else {
        return Ok(props.clone());
    };
    let enc = lookup_sibling_string_in_encode_map(map, &sibling)?;
    let mut resolved = props.clone();
    resolved.encoding = lookup_encoding_string_id(strings, &enc).ok_or_else(|| {
        VmError::InvalidValue {
            message: alloc::format!("resolved encoding `{enc}` not in string pool"),
        }
    })?;
    Ok(resolved)
}

fn lookup_encoding_string_id(pool: &crate::ir::StringPool, enc: &str) -> Option<crate::ir::StringId> {
    if let Some(id) = pool.lookup(enc) {
        return Some(id);
    }
    if let Some(id) = pool
        .values
        .iter()
        .enumerate()
        .find(|(_, v)| v.eq_ignore_ascii_case(enc))
        .map(|(idx, _)| crate::ir::StringId(idx as u32))
    {
        return Some(id);
    }
    let upper = enc.to_ascii_uppercase();
    let canonical = match upper.as_str() {
        "US-ASCII" | "ASCII" | "ISO646-US" => "US-ASCII",
        "UTF8" => "UTF-8",
        "UTF16" | "UTF_16" => "UTF-16",
        "UTF32" | "UTF_32" => "UTF-32BE",
        "UTF-32" => "UTF-32BE",
        "ISO8859-1" | "ISO_8859-1" | "LATIN1" => "ISO-8859-1",
        _ => return None,
    };
    pool.lookup(canonical)
}

fn choice_explicit_frame_bytes_encode(
    props: &IrProps,
    strings: &crate::ir::StringPool,
) -> Result<Option<usize>> {
    if props.choice_length_kind != ChoiceLengthKind::Explicit {
        return Ok(None);
    }
    let Some(units) = props.choice_length else {
        return Err(VmError::InvalidValue {
            message: "choiceLengthKind explicit requires choiceLength".into(),
        }
        .into());
    };
    let n = units as usize;
    let span = match props.length_units {
        LengthUnits::Bytes => n,
        LengthUnits::Characters => {
            let enc = encoding_name(props, strings)?;
            super::encoding::character_span_byte_length(n, &enc)?
        }
        LengthUnits::Bits => n.saturating_add(7) / 8,
    };
    Ok(Some(span))
}

fn choice_explicit_pad_byte(props: &IrProps) -> u8 {
    if props.representation == Representation::Text {
        return b' ';
    }
    props.fill_byte as u8
}

fn pad_choice_explicit_frame(
    enc: &Encoder<'_>,
    choice_props: &IrProps,
    out: &mut Vec<u8>,
    bit_count: &mut u8,
    start_byte_len: usize,
) -> Result<()> {
    let Some(frame) = choice_explicit_frame_bytes_encode(choice_props, enc.ctx.strings())? else {
        return Ok(());
    };
    if *bit_count != 0 {
        return Err(VmError::InvalidValue {
            message: "choice explicit length requires byte-aligned branch payload".into(),
        }
        .into());
    }
    let mut written = out.len().saturating_sub(start_byte_len);
    if written > frame {
        return Err(VmError::InvalidValue {
            message: "choice branch payload exceeds explicit choiceLength".into(),
        }
        .into());
    }
    let pad = choice_explicit_pad_byte(choice_props);
    while written < frame {
        write_byte_aligned(out, bit_count, core::slice::from_ref(&pad))?;
        written += 1;
    }
    Ok(())
}

fn validate_fill_byte_for_encode(props: &IrProps, strings: &crate::ir::StringPool) -> Result<()> {
    if !props.fill_byte_explicit {
        return Ok(());
    }
    let Some(ref bytes) = props.fill_byte_utf8 else {
        return Ok(());
    };
    let encoding = strings.get(props.encoding)?;
    validate_fill_byte_schema("fillByte", bytes, encoding).map_err(|e| {
        VmError::InvalidValue {
            message: e.to_string(),
        }
        .into()
    })
}

fn resolve_length_props_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<IrProps> {
    if let Some(cap) = props.length_self_string_max_cap {
        let len = map
            .values()
            .find_map(|v| match v {
                DfdlValue::String(s) => Some(s.text.chars().count() as u64),
                _ => None,
            })
            .unwrap_or(0);
        let mut resolved = props.clone();
        resolved.length = Some(len.min(cap));
        return Ok(resolved);
    }
    if props.length_kind != LengthKind::Explicit || props.length.is_some() {
        return Ok(props.clone());
    }
    let Some(sib_id) = props.length_sibling else {
        return Ok(props.clone());
    };
    let sib_name = strings.get(sib_id)?;
    let sib_val = map
        .get(sib_name)
        .or_else(|| {
            map.iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == sib_name)
                .map(|(_, v)| v)
        })
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("length sibling `{sib_name}` not available"),
        })?;
    let mut resolved = props.clone();
    let base_len = length_from_value(sib_val, props.length_sibling_cast_long)?;
    resolved.length = Some(apply_length_sibling_adjust_encode(
        base_len,
        props.length_sibling_adjust,
    )?);
    Ok(resolved)
}

fn apply_length_sibling_adjust_encode(len: u64, adjust: i64) -> Result<u64> {
    if adjust == 0 {
        return Ok(len);
    }
    if adjust < 0 {
        let sub = u64::try_from(-adjust).map_err(|_| VmError::InvalidValue {
            message: "invalid length sibling adjustment".into(),
        })?;
        return len
            .checked_sub(sub)
            .ok_or_else(|| negative_runtime_length_error(-(sub as i64 - len as i64)).into());
    }
    Ok(len.saturating_add(adjust as u64))
}

fn merged_encode_lookup(
    outer: Option<&BTreeMap<String, DfdlValue>>,
    inner: &BTreeMap<String, DfdlValue>,
) -> BTreeMap<String, DfdlValue> {
    let mut merged = BTreeMap::new();
    if let Some(o) = outer {
        for (k, v) in o {
            merged.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in inner {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

fn lookup_sibling_value_in_map<'a>(
    map: &'a BTreeMap<String, DfdlValue>,
    local: &str,
) -> Option<&'a DfdlValue> {
    if let Some(k) = map_has_local_key(map, local) {
        return map.get(&k);
    }
    for v in map.values() {
        if let DfdlValue::Sequence(seq) = v {
            if let Some(found) = lookup_sibling_value_in_map(&seq.fields, local) {
                return Some(found);
            }
        }
    }
    None
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
    let local = crate::xml_util::local_name_str(name);
    lookup_sibling_value_in_map(map, local)
        .or_else(|| lookup_sibling_value_in_map(map, name))
        .ok_or_else(|| VmError::InvalidValue {
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

fn blob_payload_bytes(value: &DfdlValue) -> Result<alloc::vec::Vec<u8>> {
    match value {
        DfdlValue::Blob(b) => Ok(b.clone()),
        DfdlValue::String(s) => crate::tdml::resolve_blob_uri_to_bytes(&s.text).map_err(
            |m| VmError::InvalidValue { message: m }.into(),
        ),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => {
            crate::tdml::resolve_blob_uri_to_bytes(s).map_err(|m| {
                VmError::InvalidValue { message: m }.into()
            })
        }
        other => Err(VmError::InvalidValue {
            message: alloc::format!("valueLength on unsupported blob value `{other:?}`"),
        }
        .into()),
    }
}

fn blob_value_byte_len(value: &DfdlValue) -> Result<usize> {
    Ok(blob_payload_bytes(value)?.len())
}

fn value_byte_length(value: &DfdlValue) -> Result<usize> {
    match value {
        DfdlValue::Blob(b) => Ok(b.len()),
        DfdlValue::String(s) if looks_like_blob_uri(&s.text) => blob_value_byte_len(value),
        DfdlValue::String(s) => Ok(s.text.len()),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => {
            if s.contains('/') || s.starts_with("file:") {
                blob_value_byte_len(value)
            } else {
                Ok(s.len())
            }
        }
        DfdlValue::HexBinary(v) => Ok(v.len()),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("valueLength on unsupported value `{other:?}`"),
        }
        .into()),
    }
}

fn looks_like_blob_uri(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("file:") || t.contains("/blobs/") || t.ends_with(".bin")
}

fn negative_runtime_length_error(value: i64) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Runtime Schema Definition Error. dfdl:length expression result must be non-negative, but was: {value}"
        ),
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
        SeparatorPosition::Postfix => index > 0 && index < total,
    }
}

fn needs_length_frame(props: &IrProps) -> bool {
    matches!(
        props.length_kind,
        LengthKind::Prefixed | LengthKind::Explicit | LengthKind::Fixed | LengthKind::Delimited
    )
}

/// When `value` is already this element's payload (from a parent sequence map), do not use
/// transparent `DfdlValue::field` unwrapping — it can drill into a same-named descendant.
fn element_payload_value<'a>(value: &'a DfdlValue, local_name: &str) -> &'a DfdlValue {
    if let Some(fields) = value.sequence_fields() {
        if fields.contains_key(local_name) {
            return fields.get(local_name).expect("key just checked");
        }
    }
    value
}

fn unwrap_root_for_encode<'a>(
    value: &'a DfdlValue,
    root_element: &str,
    root_id: u32,
    program: &IrProgram,
) -> &'a DfdlValue {
    let root_is_complex = matches!(
        program.node(root_id),
        Ok(IrNode::Element {
            kind: crate::ir::ValueKind::Complex,
            ..
        })
    );
    if root_is_complex {
        return value;
    }
    if let Some(seq) = value.sequence_value() {
        if seq.fields.len() == 1 {
            if let Some((name, inner)) = seq.fields.iter().next() {
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
        assert!(should_emit_separator(
            SeparatorPosition::Postfix,
            2,
            3,
            false,
        ));
        assert!(!should_emit_separator(
            SeparatorPosition::Postfix,
            3,
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
