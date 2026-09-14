use super::runtime::{
    encoding_name, is_suppressible_empty_representation, nil_unparse_bytes_for_encode,
    write_alignment, write_alignment_for_kind, write_byte_aligned, write_framed_payload,
    write_simple, validate_explicit_decimal_before_encode, trailing_suppressed_count,
    should_suppress_occurrence_separator, RuntimeConfig, VmContext,
};
use super::alignment::write_leading_skip;
use crate::error::{Error, Result, VmError};
use crate::length_validate::validate_fill_byte_schema;
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::{
    encode_delimiter, encode_delimiter_by_alt, encode_property_delimiter, encode_sequence_separator,
    LengthKind, LengthUnits,
    OutputValueCalc, SeparatorPosition,
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
                let seq = value
                    .sequence_value()
                    .ok_or_else(|| VmError::TypeMismatch { expected: "sequence".into() })?;
                let map = &seq.fields;
                let effective = precompute_output_values(self, children, map)?;
                self.write_initiator(props, out, bit_count, seq.meta.initiator_alt)?;
                for (idx, &child) in children.iter().enumerate() {
                    if child_skips_encode(self, child)? {
                        continue;
                    }
                    let defer_sep = sequence_separator_deferred_to_child_occurrences(self, child)?;
                    if !defer_sep {
                        self.write_sequence_separator(
                            props,
                            out,
                            bit_count,
                            idx,
                            children.len(),
                            &seq.meta,
                        )?;
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
                }
                self.write_terminator(props, out, bit_count, seq.meta.terminator_alt)?;
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
                if let Some(map) = value.sequence_fields() {
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
                if matches!(value, DfdlValue::Null) {
                    return self.encode_nil_element(*kind, props, out, bit_count);
                }
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
                    )
                    .map_err(Error::from)?;
                    write_simple(
                        out,
                        bit_count,
                        value,
                        *kind,
                        props,
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        Some(self.ctx.strings().get(*name)?),
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
            write_alignment(out, bit_count, props)?;
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
            write_alignment(out, bit_count, props)?;
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
                let value = match self.element_encode_value(props, key, map) {
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
                    if resolved.trailing_skip == 0 {
                        return Ok(());
                    }
                    write_alignment(out, bit_count, &resolved).map_err(Error::from)?;
                    crate::vm::alignment::write_trailing_skip(out, bit_count, &resolved)
                        .map_err(Error::from)?;
                    return Ok(());
                }
                if matches!(&value, DfdlValue::Null) {
                    return self.encode_nil_element(*kind, props, out, bit_count);
                }
                if let Some(child_id) = child {
                    let field = if matches!(&value, DfdlValue::Null) {
                        &value
                    } else {
                        match value.field(key) {
                            Some(inner) => inner,
                            None => &value,
                        }
                    };
                    if needs_length_frame(&resolved) {
                        self.encode_framed_element(
                            *child_id,
                            &resolved,
                            field,
                            out,
                            bit_count,
                            Some(key),
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
                    self.encode_simple_occurrences(
                        *kind,
                        &resolved,
                        &value,
                        out,
                        bit_count,
                        Some(key),
                        field_delim,
                        parent_props,
                    )
                }
            }
            IrNode::Sequence { .. } => self.encode_node(
                node_id,
                &DfdlValue::sequence(map.clone()),
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
        delim_meta: Option<&crate::value::FieldDelimiterMeta>,
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
                continue;
            }
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
                delim_meta,
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
            eval_output_value_calc(props, map, self.ctx.strings())
        } else {
            map.get(key)
                .cloned()
                .ok_or_else(|| VmError::MissingField { name: key.into() }.into())
        }
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
        write_alignment(out, bit_count, props).map_err(Error::from)?;
        self.write_initiator(props, out, bit_count, None)?;
        let payload = nil_unparse_bytes_for_encode(props, self.ctx.strings()).map_err(Error::from)?;
        write_byte_aligned(out, bit_count, &payload).map_err(Error::from)?;
        self.write_terminator(props, out, bit_count, None)?;
        Ok(())
    }
}

fn child_skips_encode(enc: &Encoder<'_>, node_id: u32) -> Result<bool> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { props, .. } => Ok(props.input_value_calc.is_some()),
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
        DfdlValue::String(s) => Ok(s.text.len()),
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

fn unwrap_root_for_encode<'a>(value: &'a DfdlValue, root_element: &str) -> &'a DfdlValue {
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
