use super::runtime::{
    decode_hex_binary, encoding_name, hex_binary_from_integer, int_bytes,
    is_suppressible_empty_representation, nil_unparse_bytes_for_encode, write_alignment,
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
    ChoiceLengthKind, LengthKind, LengthUnits, Representation, TextPadKind, OutputValueCalc,
    SeparatorPosition,
};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

fn schema_context_field_name(local_name: &str) -> String {
    alloc::format!("ex:{local_name}")
}

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
        let value = unwrap_root_for_encode(
            value,
            &self.ctx.program.root_element,
            self.ctx.program.root,
            &self.ctx.program,
        );
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
                self.encode_node(branch_node, &encode_value, out, bit_count)?;
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
                let effective = precompute_output_values(self, children, map, props)?;
                self.write_initiator(props, out, bit_count, seq.meta.initiator_alt)?;
                let mut wrote_particle = false;
                for (idx, &child) in children.iter().enumerate() {
                    if child_skips_encode(self, child)? {
                        continue;
                    }
                    if optional_sequence_particle_absent(self, child, &effective)? {
                        continue;
                    }
                    let defer_sep = sequence_separator_deferred_to_child_occurrences(self, child)?;
                    if !defer_sep {
                        if wrote_particle {
                            self.write_sequence_separator(
                                props,
                                out,
                                bit_count,
                                idx,
                                children.len(),
                                &seq.meta,
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
                        )?;
                    }
                    self.encode_sequence_particle(child, &effective, props, out, bit_count, &seq.meta)?;
                    wrote_particle = true;
                }
                self.write_terminator(props, out, bit_count, seq.meta.terminator_alt)?;
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
                            self.write_initiator(choice_props, out, bit_count, None)?;
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
                            self.write_initiator(choice_props, out, bit_count, None)?;
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
                self.write_initiator(choice_props, out, bit_count, None)?;
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
                        )
                    } else {
                        self.encode_element_occurrences(*child_id, props, field, out, bit_count, props)
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
                    write_simple(
                        out,
                        bit_count,
                        value,
                        *kind,
                        props,
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        &self.ctx.config,
                        Some(&schema_ctx),
                        None,
                        None,
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
    ) -> Result<()> {
        let items = match value {
            DfdlValue::Array(items) => items.as_slice(),
            single => core::slice::from_ref(single),
        };
        let suppressed = trailing_suppressed_count(items, props, self.ctx.strings(), None)?;
        let encode_len = items.len().saturating_sub(suppressed);
        for (idx, item) in items.iter().take(encode_len).enumerate() {
            self.write_occurrence_separator(props, out, bit_count, idx, encode_len)?;
            write_alignment_with_config(out, bit_count, props, Some(&self.ctx.config))?;
            if let Some(id) = props.initiator {
                let pat = self.ctx.strings().get(id)?;
                if !pat.is_empty() {
                    let bytes = match delim_meta.and_then(|m| m.initiator_alt) {
                        Some(a) => encode_delimiter_by_alt(pat, a),
                        None => encode_delimiter(pat),
                    };
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
                self.encode_node(child_id, item, &mut payload, &mut payload_bit_count)?;
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
            self.write_initiator(props, out, bit_count, None)?;
            if matches!(item, DfdlValue::Null) {
                let nil_bytes =
                    nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
                write_byte_aligned(out, bit_count, &nil_bytes).map_err(Error::from)?;
            } else {
                self.encode_node(node_id, item, out, bit_count)?;
            }
            self.write_terminator(props, out, bit_count, None)?;
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
                let mut value = match self.element_encode_value(props, key, map) {
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
                let mut resolved = resolve_length_props_encode(props, map, self.ctx.strings())?;
                resolved = resolve_encoding_for_encode(&resolved, map, self.ctx.strings())?;
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
                    )
                }
            }
            IrNode::Sequence {
                children,
                props: inner_props,
            } => {
                let effective =
                    precompute_output_values(self, children, map, inner_props)?;
                self.write_initiator(inner_props, out, bit_count, None)?;
                for &child in children.iter() {
                    if child_skips_encode(self, child)? {
                        continue;
                    }
                    self.encode_sequence_particle(
                        child,
                        &effective,
                        inner_props,
                        out,
                        bit_count,
                        seq_meta,
                    )?;
                }
                self.write_terminator(inner_props, out, bit_count, None)?;
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
                        self.write_initiator(choice_props, out, bit_count, None)?;
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
                        self.write_initiator(choice_props, out, bit_count, None)?;
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
                self.write_initiator(choice_props, out, bit_count, None)?;
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
                let suppress = should_suppress_occurrence_separator(
                    sep_props,
                    props,
                    items,
                    idx,
                    true,
                    self.ctx.strings(),
                )? || is_suppressible_empty_representation(item, props, self.ctx.strings())?;
                if !suppress {
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
                encode_siblings,
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
                )? {
                    self.write_occurrence_separator(sep_props, out, bit_count, idx, encode_len)?;
                }
            }
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
            return eval_output_value_calc(self, props, map, &[], props);
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

    fn write_initiator(
        &self,
        props: &IrProps,
        out: &mut Vec<u8>,
        bit_count: &mut u8,
        alt: Option<u8>,
    ) -> Result<()> {
        if let Some(id) = props.initiator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() {
                let output_nl = props
                    .output_new_line
                    .and_then(|id| self.ctx.strings().get(id).ok());
                let bytes = match alt {
                    Some(a) => encode_delimiter_by_alt(pat, a),
                    None => encode_property_delimiter(pat, output_nl),
                };
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
    ) -> Result<()> {
        if let Some(id) = props.terminator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() {
                let output_nl = props
                    .output_new_line
                    .and_then(|id| self.ctx.strings().get(id).ok());
                let bytes = match alt {
                    Some(a) => encode_delimiter_by_alt(pat, a),
                    None => encode_property_delimiter(pat, output_nl),
                };
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
    ) -> Result<()> {
        if !should_emit_separator(props.separator_position, index, total, false) {
            return Ok(());
        }
        let Some(id) = props.separator else {
            return Ok(());
        };
        let pat = self.ctx.strings().get(id)?;
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
        self.write_initiator(props, out, bit_count, None)?;
        let payload = nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
        write_byte_aligned(out, bit_count, &payload).map_err(Error::from)?;
        self.write_terminator(props, out, bit_count, None)?;
        Ok(())
    }
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
                || props.input_value_calc_path.is_some(),
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

fn precompute_output_values<'a>(
    enc: &Encoder<'a>,
    children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
    parent_props: &IrProps,
) -> Result<BTreeMap<String, DfdlValue>> {
    // Cycle detection is conservative; precompute multi-pass resolves OVC order when acyclic.
    let _ = ovc_length_cycle_error(enc, children);
    for &child in children {
        let IrNode::Element { name, props, .. } = enc.ctx.program.node(child)? else {
            continue;
        };
        if props.output_value_calc_conditional {
            let elem = enc.ctx.strings().get(*name)?;
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Unparse Error: Element `{elem}` does not have a value, due to a circular dependency"
                ),
            }
            .into());
        }
    }
    let mut effective = map.clone();
    for _pass in 0..3 {
        for &child in children {
            let IrNode::Element { name, props, .. } = enc.ctx.program.node(child)? else {
                continue;
            };
            if props.output_value_calc.is_none() {
                continue;
            }
            let key = enc.ctx.strings().get(*name)?.to_string();
            let computed =
                eval_output_value_calc(enc, props, &effective, children, parent_props)?;
            effective.insert(key, computed);
        }
    }
    Ok(effective)
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

fn eval_output_infoset_path(
    enc: &Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
) -> Result<i64> {
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
    for step in steps.iter().skip(1) {
        let local = strings.get(step.local)?;
        value = match value {
            DfdlValue::Sequence(seq) => {
                let child_val = if let Some(k) = map_has_local_key(&seq.fields, local) {
                    seq.fields.get(&k).cloned()
                } else if let Ok(Some(cid)) = find_particle_by_local_in_children(enc, scope, local) {
                    synthesize_element_subtree_value(enc, cid, map, scope).ok()
                } else {
                    None
                };
                let child = child_val.as_ref().ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc path missing `{local}`"),
                })?;
                match (step.index, child) {
                    (Some(n), DfdlValue::Array(items)) => items
                        .get((n as usize).saturating_sub(1))
                        .cloned()
                        .ok_or_else(|| VmError::InvalidValue {
                            message: alloc::format!("outputValueCalc path missing `{local}[{n}]`"),
                        })?,
                    (None, v) => v.clone(),
                    _ => {
                        return Err(VmError::InvalidValue {
                            message: alloc::format!("outputValueCalc path invalid index on `{local}`"),
                        }
                        .into())
                    }
                }
            }
            _ => {
                return Err(VmError::InvalidValue {
                    message: "outputValueCalc path requires sequence".into(),
                }
                .into())
            }
        };
    }
    numeric_from_dfdl_value(&value)
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

fn find_child_element_by_name(
    enc: &Encoder<'_>,
    children: &[u32],
    local_name: &str,
) -> Result<Option<u32>> {
    for &child in children {
        let IrNode::Element { name, .. } = enc.ctx.program.node(child)? else {
            continue;
        };
        let n = enc.ctx.strings().get(*name)?;
        let n_local = crate::xml_util::local_name_str(n);
        let sib_local = crate::xml_util::local_name_str(local_name);
        if n == local_name || n_local == local_name || n_local == sib_local {
            return Ok(Some(child));
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
    enc.encode_node(encode_id, value, &mut buf, &mut bit_count)?;
    match units {
        LengthUnits::Bits => Ok(encoded_value_length_bits(buf.len(), bit_count)),
        LengthUnits::Bytes => {
            Ok((encoded_value_length_bits(buf.len(), bit_count) + 7) / 8)
        }
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "outputValueCalc character units".into(),
        }
        .into()),
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
            let base = eval_output_infoset_path(enc, steps, children, map)?;
            let len = base.saturating_add(addend);
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
            let sib_name = strings.get(
                props
                    .output_value_calc_sibling
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "outputValueCalc sibling missing".into(),
                    })?,
            )?;
            let sib_val = map.get(sib_name).ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!("outputValueCalc sibling `{sib_name}` not available"),
            })?;
            if let Some(child_id) = find_child_element_by_name(enc, children, sib_name)? {
                measure_value_length(enc, child_id, sib_val, units)? as i64 + addend
            } else {
                length_in_units(value_byte_length(sib_val)?, units)? as i64 + addend
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
        OutputValueCalc::HexBinaryFromLexical
        | OutputValueCalc::HexBinaryFromInteger(_)
        | OutputValueCalc::HexBinaryFromShort(_)
        | OutputValueCalc::HexBinaryFromByteSibling
        | OutputValueCalc::InfosetPathAddend => {
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

fn resolve_encoding_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &crate::ir::StringPool,
) -> Result<IrProps> {
    let raw = strings.get(props.encoding)?;
    let Some(sibling) = parse_sibling_property_expr(raw) else {
        return Ok(props.clone());
    };
    let sib_val = map.get(&sibling).ok_or_else(|| VmError::InvalidValue {
        message: alloc::format!("encoding sibling `{sibling}` not available"),
    })?;
    let enc = match sib_val {
        DfdlValue::String(s) => s.text.clone(),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.clone(),
        other => {
            return Err(VmError::InvalidValue {
                message: alloc::format!("encoding sibling must be string, got `{other:?}`"),
            }
            .into())
        }
    };
    let mut resolved = props.clone();
    resolved.encoding = strings.lookup(&enc).ok_or_else(|| {
        VmError::InvalidValue {
            message: alloc::format!("resolved encoding `{enc}` not in string pool"),
        }
    })?;
    Ok(resolved)
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
