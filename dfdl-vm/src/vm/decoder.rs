use super::runtime::{
    consume_element_framing, consume_element_trailing_framing, consume_enclosing_delimiter,
    default_value_for, encoding_name, has_non_empty_terminator,
    is_suppressible_empty_representation, prefixed_payload_byte_length, read_delimited_bytes,
    read_length_span, read_prefixed_payload, read_simple, read_until_separator,
    should_suppress_decode_infix_separator, validate_explicit_decimal_before_decode,
    validate_unbounded_wsp_star_terminator, would_read_empty_delimited_field, Cursor,
    RuntimeConfig, VmContext,
};
use crate::schema::boolean_reps::BooleanSiblingEnv;
use crate::length_validate::{binary_length_validation_applies, validate_data_length_vm};
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps, StringId, ValueKind};
use crate::schema::{
    match_length_pattern, InputValueCalc, LengthKind, LengthUnits, OccursCountKind,
    Representation, SeparatorPosition,
};
use crate::value::{DfdlValue, StringValue};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::cell::RefCell;
use crate::value::FieldDelimiterMeta;

#[derive(Debug, Clone)]
struct SiblingState {
    value: DfdlValue,
    content_bytes: usize,
}

fn sibling_boolean_env<'a>(
    siblings: Option<&'a BTreeMap<String, SiblingState>>,
    text_map: &'a BTreeMap<String, String>,
    bytes_map: &'a BTreeMap<String, usize>,
) -> Option<BooleanSiblingEnv<'a>> {
    if siblings.is_none() {
        return None;
    }
    Some(BooleanSiblingEnv {
        text: text_map,
        content_bytes: bytes_map,
    })
}

fn sibling_maps_for_boolean(
    siblings: Option<&BTreeMap<String, SiblingState>>,
) -> (BTreeMap<String, String>, BTreeMap<String, usize>) {
    let mut text = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    if let Some(sibs) = siblings {
        for (name, state) in sibs {
            bytes.insert(name.clone(), state.content_bytes);
            if let DfdlValue::String(s) = &state.value {
                text.insert(name.clone(), s.text.clone());
            }
        }
    }
    (text, bytes)
}

enum FramingExtraOccurrences {
    None,
    One,
    Double,
}

/// DFDL decoder VM — executes compiled IR against an input byte stream.
pub struct Decoder<'a> {
    ctx: VmContext<'a>,
    /// Enclosing complex elements (for terminator/separator conflict detection).
    enclosing: RefCell<Vec<IrProps>>,
    /// Element names aligned with [`Self::enclosing`] (for separator error context).
    enclosing_names: RefCell<Vec<String>>,
    /// Per-field delimiter alternative indices captured during the current sequence child.
    field_delimiters: RefCell<BTreeMap<String, FieldDelimiterMeta>>,
}

impl<'a> Decoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
            enclosing: RefCell::new(Vec::new()),
            enclosing_names: RefCell::new(Vec::new()),
            field_delimiters: RefCell::new(BTreeMap::new()),
        }
    }

    /// Decode one logical value from `input`.
    pub fn decode(&self, input: &[u8]) -> Result<DfdlValue> {
        self.decode_with_bit_limit(input, None, None)
    }

    /// Decode with an optional significant bit length (TDML `type="bits"` documents).
    pub fn decode_with_bit_limit(
        &self,
        input: &[u8],
        frame_bits: Option<usize>,
        transmission_bit_order: Option<crate::schema::BitOrder>,
    ) -> Result<DfdlValue> {
        self.decode_with_tdml_options(input, frame_bits, transmission_bit_order, None)
    }

    pub fn decode_with_tdml_options(
        &self,
        input: &[u8],
        frame_bits: Option<usize>,
        transmission_bit_order: Option<crate::schema::BitOrder>,
        tdml_bit_order_regions: Option<alloc::vec::Vec<(crate::schema::BitOrder, usize)>>,
    ) -> Result<DfdlValue> {
        use crate::schema::BitOrder;
        let transmission = transmission_bit_order.unwrap_or(BitOrder::MostSignificantBitFirst);
        let mut cursor = match frame_bits {
            Some(bits) => Cursor::with_frame_bits_and_transmission(input, bits, transmission),
            None => {
                let mut c = Cursor::new(input);
                c.transmission_bit_order = transmission;
                c
            }
        };
        cursor.tdml_bit_order_regions = tdml_bit_order_regions;
        let value = self.decode_node(
            self.ctx.program.root,
            &mut cursor,
            false,
            None,
            None,
            None,
            false,
            &[],
        )?;
        self.consume_root_delimited_suffix(&mut cursor)?;
        if self.ctx.config.strict_eos {
            if let Some(limit) = cursor.frame_bit_limit {
                let consumed = cursor.absolute_bit_index();
                if consumed < limit {
                    let remaining_bits = limit.saturating_sub(consumed);
                    return Err(VmError::TrailingData {
                        consumed_bits: consumed,
                        remaining_bits,
                    }
                    .into());
                }
            } else if cursor.bit_count == 0 && cursor.remaining() > 0 {
                let total_bits = cursor.data.len().saturating_mul(8);
                let remaining_bits = cursor.remaining().saturating_mul(8);
                return Err(VmError::TrailingData {
                    consumed_bits: total_bits.saturating_sub(remaining_bits),
                    remaining_bits,
                }
                .into());
            }
        }
        Ok(wrap_root(
            &self.ctx.program.root_element,
            value,
        ))
    }

    fn decode_node(
        &self,
        node_id: u32,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, props } => {
                let mut initiator_alt = None;
                if let Some(id) = props.initiator {
                    let pat = self.ctx.strings().get(id)?;
                    if !pat.is_empty() {
                        let alt = cursor
                            .consume_delimiter_with_alt(pat, props.ignore_case)
                            .ok_or(VmError::InvalidValue {
                                message: "initiator mismatch".into(),
                            })?;
                        initiator_alt = Some(alt.1);
                    }
                }
                let mut extended = stop_sequences.to_vec();
                extended.push(props);
                let child_stops = extended.as_slice();
                let mut map = BTreeMap::new();
                let mut seq_siblings = siblings.cloned().unwrap_or_default();
                let mut infix_sep_newline_prefix = Vec::new();
                let mut separator_alts = Vec::new();
                let mut field_delim_meta = BTreeMap::new();
                let mut prev_absent_or_empty = false;
                for (idx, &child) in children.iter().enumerate() {
                    let child_has_following = self.following_sibling_consumes_input(children, idx);
                    let child_element_props = match self.ctx.program.node(child) {
                        Ok(IrNode::Element { props: cp, .. }) => Some(cp),
                        _ => None,
                    };
                    if idx > 0 {
                        if let Some(sep_id) = props.separator {
                            let pat = self.ctx.strings().get(sep_id)?;
                            if let Some(err) = self.separator_enclosing_delimiter_conflict(
                                props,
                                pat,
                                cursor,
                                child_stops,
                            ) {
                                return Err(err);
                            }
                        }
                    }
                    self.field_delimiters.borrow_mut().clear();
                    let suppress_sep = child_element_props
                        .map(|cp| {
                            should_suppress_decode_infix_separator(props, cp, prev_absent_or_empty)
                        })
                        .unwrap_or(false);
                    let sep_alt = if suppress_sep || !self.particle_consumes_input(child) {
                        None
                    } else {
                        self.consume_separator(
                            props,
                            cursor,
                            idx,
                            children.len(),
                            &mut infix_sep_newline_prefix,
                            child_stops,
                        )?
                    };
                    separator_alts.push(sep_alt);
                    let saved = cursor.clone();
                    let start = cursor.pos;
                    match self.decode_particle(
                        child,
                        cursor,
                        child_has_following,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                    ) {
                        Ok(child_value) => {
                            prev_absent_or_empty = child_element_props
                                .map(|cp| {
                                    is_suppressible_empty_representation(
                                        &child_value,
                                        cp,
                                        self.ctx.strings(),
                                    )
                                })
                                .transpose()?
                                .unwrap_or(false);
                            let consumed = cursor.pos.saturating_sub(start);
                            if let IrNode::Element { name, props, .. } = self.ctx.program.node(child)? {
                                let key = self.ctx.strings().get(*name)?.to_string();
                                let content_bytes = if props.length_kind == LengthKind::Prefixed {
                                    prefixed_payload_byte_length(
                                        &cursor.data[start..cursor.pos],
                                        props,
                                        self.ctx.strings(),
                                    )?
                                } else {
                                    consumed
                                };
                                seq_siblings.insert(
                                    key,
                                    SiblingState {
                                        value: child_value.clone(),
                                        content_bytes,
                                    },
                                );
                            }
                            insert_child(&mut map, child, child_value, self.ctx.program)?;
                            if let IrNode::Element { name, .. } = self.ctx.program.node(child)? {
                                let key = self.ctx.strings().get(*name)?.to_string();
                                if let Some(meta) = self.field_delimiters.borrow_mut().remove(&key) {
                                    field_delim_meta.insert(key, meta);
                                }
                            }
                        }
                        Err(e) if is_element_absent(&e) => {
                            if let Ok(IrNode::Element { props, .. }) =
                                self.ctx.program.node(child)
                            {
                                let zero_len = props.length_kind == LengthKind::Explicit
                                    && props.length == Some(0);
                                if props.occurs_min > 0 && !zero_len {
                                    return Err(e);
                                }
                            }
                            prev_absent_or_empty = true;
                            *cursor = saved;
                        }
                        Err(e) => return Err(e),
                    }
                }
                let mut terminator_alt = None;
                if let Some(id) = props.terminator {
                    let pat = self.ctx.strings().get(id)?;
                    if !pat.is_empty() {
                        if let Some((n, alt)) =
                            cursor.consume_delimiter_with_alt(pat, props.ignore_case)
                        {
                            if n == 0 && !cursor.is_empty() {
                                return Err(VmError::InvalidValue {
                                    message: alloc::format!("terminator mismatch: expected `{pat}`"),
                                }
                                .into());
                            }
                            terminator_alt = Some(alt);
                        } else if !cursor.is_empty() {
                            return Err(VmError::InvalidValue {
                                message: alloc::format!("terminator mismatch: expected `{pat}`"),
                            }
                            .into());
                        }
                    }
                }
                consume_element_trailing_framing(cursor, props)?;
                Ok(DfdlValue::Sequence(crate::value::SequenceValue {
                    fields: map,
                    meta: crate::value::SequenceMeta {
                        infix_sep_newline_prefix,
                        initiator_alt,
                        terminator_alt,
                        separator_alts,
                        field_delimiters: field_delim_meta,
                    },
                }))
            }
            IrNode::Choice { branches, props: _ } => {
                for branch in branches {
                    let saved = cursor.clone();
                    if let Ok(value) = self.decode_node(
                        branch.node,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    ) {
                        let name = self.ctx.strings().get(branch.name)?.to_string();
                        return Ok(DfdlValue::choice(name, value));
                    }
                    *cursor = saved;
                }
                Err(VmError::InvalidChoice.into())
            }
            IrNode::Element { props, .. } => self.decode_element_occurrences(
                node_id,
                props,
                cursor,
                has_following_sibling,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            ),
        }
    }

    fn decode_particle(
        &self,
        node_id: u32,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element { props, .. } => self.decode_element_occurrences(
                node_id,
                props,
                cursor,
                has_following_sibling,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            ),
            _ => self.decode_node(
                node_id,
                cursor,
                has_following_sibling,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            ),
        }
    }

    fn decode_element_occurrences(
        &self,
        node_id: u32,
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<DfdlValue> {
        validate_unbounded_wsp_star_terminator(props, self.ctx.strings())?;

        let mut min = props.occurs_min;
        if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
            min = 0;
        }
        let mut max = props.occurs_max.unwrap_or(u64::MAX);
        if props.occurs_count_kind == OccursCountKind::Parsed {
            if max != u64::MAX && min == max {
                match self.framing_extra_occurrence(node_id, props, parent_sequence) {
                    FramingExtraOccurrences::One => max = max.saturating_add(1),
                    FramingExtraOccurrences::Double => max = max.saturating_mul(2),
                    FramingExtraOccurrences::None => {}
                }
            } else if max != u64::MAX {
                // DFDL-5-062R: parsed count kind ignores maxOccurs during parse (post-decode validation only).
                max = u64::MAX;
            }
        }
        let populate_path = element_prefixed_name(self.ctx.program, node_id).ok();
        let populate_errors = should_populate_array_errors(props);
        let mut items = Vec::new();

        while (items.len() as u64) < max {
            if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
                break;
            }
            if items.len() as u64 >= min && cursor.is_empty() {
                break;
            }
            let before_occurrence_sep = cursor.clone();
            if !items.is_empty() {
                // Always try: delimited fields may defer enclosing consume, leaving the
                // occurrence separator at the cursor; if already consumed, this is a no-op.
                self.consume_occurrence_separator(parent_sequence, cursor)?;
            }
            let require_delimiter = has_following_sibling;
            let saved = cursor.clone();
            match self.decode_single_element(
                node_id,
                cursor,
                require_delimiter,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            ) {
                Ok(v) => {
                    if max == u64::MAX
                        && cursor.absolute_bit_index() == saved.absolute_bit_index()
                        && !cursor.is_frame_consumed()
                    {
                        if matches!(v, DfdlValue::Null) {
                            items.push(v);
                            continue;
                        }
                        if (items.len() as u64) >= min {
                            *cursor = before_occurrence_sep;
                            break;
                        }
                        return Err(VmError::InvalidValue {
                            message: alloc::format!(
                                "repeating element made no progress at byte offset {}",
                                saved.pos
                            ),
                        }
                        .into());
                    }
                    items.push(v);
                }
                Err(e) => {
                    if (items.len() as u64) >= min {
                        *cursor = if items.is_empty() {
                            saved
                        } else {
                            before_occurrence_sep
                        };
                        break;
                    }
                    if min == 0 && items.is_empty() {
                        *cursor = saved;
                        return Err(VmError::ElementAbsent.into());
                    }
                    if let Some(default) = default_value_for(
                        element_kind(self.ctx.program, node_id)?,
                        props,
                        self.ctx.strings(),
                    ) {
                        items.push(default);
                        break;
                    }
                    if populate_errors {
                        if let Some(path) = populate_path.as_deref() {
                            let index = items.len() as u64 + 1;
                            return Err(populate_failed_error(path, index, &e.to_string()).into());
                        }
                    }
                    return Err(e);
                }
            }
        }

        if (items.len() as u64) < min {
            if populate_errors {
                if let Some(path) = populate_path.as_deref() {
                    let index = items.len() as u64 + 1;
                    return Err(
                        populate_failed_error(
                            path,
                            index,
                            &alloc::format!(
                                "expected at least {min} occurrences, got {}",
                                items.len()
                            ),
                        )
                        .into(),
                    );
                }
            }
            return Err(VmError::InvalidValue {
                message: alloc::format!("expected at least {min} occurrences, got {}", items.len()),
            }
            .into());
        }

        if items.is_empty() {
            return Err(VmError::ElementAbsent.into());
        }

        if items.len() == 1 {
            Ok(items.remove(0))
        } else {
            Ok(DfdlValue::Array(items))
        }
    }

    fn decode_single_element(
        &self,
        node_id: u32,
        cursor: &mut Cursor<'_>,
        require_delimiter: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                let props = resolve_length_props(
                    props,
                    siblings,
                    *kind,
                    self.ctx.strings(),
                    &self.ctx.program.tunables,
                )?;
                if pattern_text_frame
                    && props.representation == Representation::Binary
                    && !matches!(*kind, ValueKind::HexBinary)
                {
                    return Err(VmError::InvalidValue {
                        message:
                            "Parse Error. lengthKind pattern complex type requires text representation"
                                .into(),
                    }
                    .into());
                }
                consume_element_framing(
                    cursor,
                    &props,
                    *kind,
                    encoding_name(&props, self.ctx.strings())?,
                )?;
                if let Some(child_id) = child {
                    crate::vm::runtime::validate_nil_value_runtime(&props, self.ctx.strings())?;
                    self.consume_initiator(&props, cursor)?;
                    if props.nillable
                        && crate::vm::runtime::nil_value_includes_empty(&props, self.ctx.strings())?
                        && cursor.is_empty()
                    {
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            DfdlValue::Null,
                            *kind,
                        ));
                    }
                    if props.nillable
                        && crate::vm::runtime::try_consume_nillable_element_nil(
                            cursor,
                            &props,
                            parent_sequence,
                            self.ctx.strings(),
                        )?
                    {
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            DfdlValue::Null,
                            *kind,
                        ));
                    }
                    if props.length_kind == LengthKind::Pattern {
                        let id = props.length_pattern.ok_or(VmError::InvalidValue {
                            message: "pattern complex missing lengthPattern".into(),
                        })?;
                        let pat = self.ctx.strings().get(id)?;
                        let len = match_length_pattern(&cursor.data[cursor.pos..], pat).ok_or(
                            VmError::InvalidValue {
                                message: alloc::format!("pattern `{pat}` mismatch"),
                            },
                        )?;
                        let bytes = cursor
                            .read_bytes(len)
                            .ok_or(VmError::UnexpectedEof)?;
                        let mut sub = Cursor::new(&bytes);
                        let scope = bytes.len();
                        let inner = self.decode_node(
                            *child_id,
                            &mut sub,
                            false,
                            None,
                            None,
                            Some(scope),
                            true,
                            &[],
                        )?;
                        if !sub.is_empty() {
                            return Err(VmError::InvalidValue {
                                message: "unconsumed bytes in pattern-length complex element".into(),
                            }
                            .into());
                        }
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            inner,
                            ValueKind::Complex,
                        ));
                    }
                    if props.length_kind == LengthKind::Explicit {
                        let len = props.length.ok_or(VmError::InvalidValue {
                            message: "explicit complex missing length".into(),
                        })? as usize;
                        if props.length_units == LengthUnits::Characters
                            || (props.length_units == LengthUnits::Bytes
                                && props.representation == Representation::Text)
                        {
                            let bytes = read_length_span(
                                cursor,
                                len,
                                props.length_units,
                                encoding_name(&props, self.ctx.strings())?,
                                props.bit_order,
                                props.encoding_error_policy,
                                true,
                            )?;
                            let mut sub = Cursor::new(&bytes);
                            let scope = bytes.len();
                            let inner = self.decode_node(
                                *child_id,
                                &mut sub,
                                false,
                                None,
                                None,
                                Some(scope),
                                pattern_text_frame,
                                &[],
                            )?;
                            if props.truncate_specified_length_string && !sub.is_empty() {
                                return Err(VmError::InvalidValue {
                                    message:
                                        "unconsumed bytes in explicit-length complex element".into(),
                                }
                                .into());
                            }
                            self.consume_terminator(&props, cursor)?;
                            return Ok(wrap_named(
                                self.ctx.strings().get(*name)?,
                                inner,
                                ValueKind::Complex,
                            ));
                        }
                        let bit_len = match props.length_units {
                            LengthUnits::Bits => len,
                            LengthUnits::Bytes => len.saturating_mul(8),
                            LengthUnits::Characters => unreachable!("handled above"),
                        };
                        let frame_start = cursor.absolute_bit_index();
                        let prev_limit = cursor
                            .frame_bit_limit
                            .replace(frame_start.saturating_add(bit_len));
                        let inner = self.decode_node(
                            *child_id,
                            cursor,
                            false,
                            None,
                            None,
                            None,
                            pattern_text_frame,
                            &[],
                        )?;
                        if props.truncate_specified_length_string
                            && !cursor.is_frame_consumed()
                        {
                            cursor.frame_bit_limit = prev_limit;
                            return Err(VmError::InvalidValue {
                                message:
                                    "unconsumed bytes in explicit-length complex element".into(),
                            }
                            .into());
                        }
                        if let Some(limit) = cursor.frame_bit_limit {
                            cursor.skip_to_bit_index(limit, props.bit_order)?;
                        }
                        cursor.frame_bit_limit = prev_limit;
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            inner,
                            ValueKind::Complex,
                        ));
                    }
                    if props.length_kind == LengthKind::Delimited {
                        let inline_sequence = matches!(
                            self.ctx.program.node(*child_id),
                            Ok(IrNode::Sequence { .. })
                        );
                        if !inline_sequence {
                            let bytes = read_delimited_bytes(
                                cursor,
                                &props,
                                self.ctx.strings(),
                                require_delimiter,
                                stop_sequences,
                            )?;
                            consume_enclosing_delimiter(
                                cursor,
                                &props,
                                self.ctx.strings(),
                                stop_sequences,
                            )?;
                            let mut sub = Cursor::new(&bytes);
                            let scope = bytes.len();
                            let inner = self.decode_node(
                                *child_id,
                                &mut sub,
                                false,
                                None,
                                None,
                                Some(scope),
                                pattern_text_frame,
                                &[],
                            )?;
                            if !sub.is_empty() {
                                return Err(VmError::InvalidValue {
                                    message: "unconsumed bytes in delimited complex element".into(),
                                }
                                .into());
                            }
                            return Ok(wrap_named(
                                self.ctx.strings().get(*name)?,
                                inner,
                                ValueKind::Complex,
                            ));
                        }
                        // Sequence children decode in the outer stream (see implicit+terminator path).
                    }
                    if props.length_kind == LengthKind::Prefixed {
                        let bytes = read_prefixed_payload(
                            cursor,
                            &props,
                            self.ctx.strings(),
                            Some(self.ctx.strings().get(*name)?),
                        )?;
                        let mut sub = Cursor::new(&bytes);
                        let scope = bytes.len();
                        let inner = self.decode_node(*child_id, &mut sub, false, None, None, Some(scope), pattern_text_frame, &[])?;
                        if !sub.is_empty() {
                            return Err(VmError::InvalidValue {
                                message: "unconsumed bytes in prefixed complex element".into(),
                            }
                            .into());
                        }
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            inner,
                            ValueKind::Complex,
                        ));
                    }
                    if props.length_kind == LengthKind::Implicit {
                        if let Some(term_id) = props.terminator {
                            let term = self.ctx.strings().get(term_id)?;
                            if !term.is_empty() {
                                if matches!(
                                    self.ctx.program.node(*child_id),
                                    Ok(IrNode::Sequence { .. })
                                ) {
                                    // Decode sequence children in the outer stream so separators
                                    // can detect enclosing terminators (e.g. `$` vs `$$`).
                                } else {
                                let bytes = read_until_separator(cursor, term, false, props.ignore_case)?;
                                if crate::schema::match_delimiter_opts(
                                    &cursor.data[cursor.pos..],
                                    term,
                                    props.ignore_case,
                                )
                                .is_some()
                                {
                                    let _ = cursor.consume_delimiter(term, props.ignore_case);
                                }
                                let mut sub = Cursor::new(&bytes);
                                let scope = bytes.len();
                                let inner = self.decode_node(*child_id, &mut sub, false, None, None, Some(scope), pattern_text_frame, &[])?;
                                if !sub.is_empty() {
                                    return Err(VmError::InvalidValue {
                                        message:
                                            "unconsumed bytes in terminator-bounded complex element"
                                                .into(),
                                    }
                                    .into());
                                }
                                return Ok(wrap_named(
                                    self.ctx.strings().get(*name)?,
                                    inner,
                                    ValueKind::Complex,
                                ));
                                }
                            }
                        }
                        if let Some(parent) = parent_sequence {
                            if let Some(sep_id) = parent.separator {
                                let sep = self.ctx.strings().get(sep_id)?;
                                let child_is_sequence = matches!(
                                    self.ctx.program.node(*child_id),
                                    Ok(IrNode::Sequence { .. })
                                );
                                let parent_sep_scopes_child = parent_sequence.is_some_and(|p| {
                                    matches!(
                                        p.separator_position,
                                        SeparatorPosition::Postfix
                                    )
                                });
                                if (!child_is_sequence || parent_sep_scopes_child)
                                    && self.inner_sequence_separator(*child_id)?.as_deref()
                                        != Some(sep)
                                {
                                    let bytes =
                                        read_until_separator(cursor, sep, false, parent.ignore_case)?;
                                    if crate::schema::match_delimiter_opts(
                                        &cursor.data[cursor.pos..],
                                        sep,
                                        parent.ignore_case,
                                    )
                                    .is_some()
                                    {
                                        let _ = cursor.consume_delimiter(sep, parent.ignore_case);
                                    }
                                    let mut sub = Cursor::new(&bytes);
                                    let scope = bytes.len();
                                    let inner = self.decode_node(*child_id, &mut sub, false, None, None, Some(scope), pattern_text_frame, &[])?;
                                    if !sub.is_empty() {
                                        return Err(VmError::InvalidValue {
                                            message:
                                                "unconsumed bytes in separator-bounded complex element"
                                                    .into(),
                                        }
                                        .into());
                                    }
                                    return Ok(wrap_named(
                                        self.ctx.strings().get(*name)?,
                                        inner,
                                        ValueKind::Complex,
                                    ));
                                }
                            }
                        }
                    }
                    let mut element_stops = stop_sequences.to_vec();
                    if props.separator.is_some()
                        || has_non_empty_terminator(&props, self.ctx.strings())?
                    {
                        element_stops.push(&props);
                    }
                    self.enclosing.borrow_mut().push(props.clone());
                    self.enclosing_names
                        .borrow_mut()
                        .push(self.ctx.strings().get(*name)?.to_string());
                    let inner = self.decode_node(
                        *child_id,
                        cursor,
                        false,
                        parent_sequence,
                        None,
                        content_scope_bytes,
                        pattern_text_frame,
                        &element_stops,
                    )?;
                    self.enclosing_names.borrow_mut().pop();
                    self.enclosing.borrow_mut().pop();
                    self.consume_terminator(&props, cursor)?;
                    consume_element_trailing_framing(cursor, &props)?;
                    Ok(wrap_named(
                        self.ctx.strings().get(*name)?,
                        inner,
                        ValueKind::Complex,
                    ))
                } else if props.input_value_calc_segments.is_some() {
                    eval_input_value_calc_concat(&props, siblings, self.ctx.strings())
                        .map_err(Into::into)
                } else if props.input_value_calc.is_some() {
                    eval_input_value_calc(
                        &props,
                        *kind,
                        cursor,
                        siblings,
                        self.ctx.strings(),
                        content_scope_bytes,
                    )
                    .map_err(Into::into)
                } else {
                    if props.nillable
                        && crate::vm::runtime::try_consume_nillable_element_nil(
                            cursor,
                            &props,
                            parent_sequence,
                            self.ctx.strings(),
                        )?
                    {
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            DfdlValue::Null,
                            *kind,
                        ));
                    }
                    let field_name = self.ctx.strings().get(*name)?.to_string();
                    let mut delim_meta = FieldDelimiterMeta::default();
                    let (sibling_text_map, sibling_bytes_map) =
                        sibling_maps_for_boolean(siblings);
                    let sibling_env = sibling_boolean_env(
                        siblings,
                        &sibling_text_map,
                        &sibling_bytes_map,
                    );
                    let value = read_simple(
                        cursor,
                        *kind,
                        &props,
                        self.ctx.strings(),
                        require_delimiter,
                        stop_sequences,
                        Some(&field_name),
                        &self.ctx.program.tunables,
                        false,
                        Some(&mut delim_meta),
                        sibling_env.as_ref(),
                        self.ctx.config.defer_facet_validation,
                    )
                    .map_err(crate::error::Error::from)?;
                    if delim_meta.initiator_alt.is_some() || delim_meta.terminator_alt.is_some() {
                        self.field_delimiters
                            .borrow_mut()
                            .insert(field_name, delim_meta);
                    }
                    Ok(value)
                }
            }
            _ => self.decode_node(node_id, cursor, false, None, None, content_scope_bytes, pattern_text_frame, stop_sequences),
        }
    }

    /// Section 9 framing TDML: minOccurs=maxOccurs with extra physical occurrences.
    fn framing_extra_occurrence(
        &self,
        node_id: u32,
        props: &IrProps,
        parent_sequence: Option<&IrProps>,
    ) -> FramingExtraOccurrences {
        if props.nillable {
            return FramingExtraOccurrences::None;
        }
        if props.length_kind == LengthKind::Delimited
            && has_non_empty_terminator(props, self.ctx.strings()).unwrap_or(false)
        {
            let comma_sep = parent_sequence
                .and_then(|p| p.separator)
                .and_then(|id| self.ctx.strings().get(id).ok().map(|s| s == ","))
                .unwrap_or(false);
            if comma_sep
                && matches!(
                    self.ctx.program.node(node_id),
                    Ok(IrNode::Element { child: None, .. })
                )
            {
                return FramingExtraOccurrences::One;
            }
            return FramingExtraOccurrences::None;
        }
        let Ok(IrNode::Element {
            child: Some(child_id),
            ..
        }) = self.ctx.program.node(node_id)
        else {
            return FramingExtraOccurrences::None;
        };
        let Ok(IrNode::Choice { branches, .. }) = self.ctx.program.node(*child_id) else {
            return FramingExtraOccurrences::None;
        };
        let mut any_delimited = false;
        let mut all_non_delimited = true;
        for b in branches {
            if let Ok(IrNode::Element { props: p, .. }) = self.ctx.program.node(b.node) {
                if p.length_kind == LengthKind::Delimited {
                    any_delimited = true;
                    all_non_delimited = false;
                }
            } else {
                all_non_delimited = false;
            }
        }
        if all_non_delimited {
            FramingExtraOccurrences::One
        } else if any_delimited {
            FramingExtraOccurrences::Double
        } else {
            FramingExtraOccurrences::None
        }
    }

    fn is_delimited_element(&self, node_id: u32, props: &IrProps) -> bool {
        if props.length_kind == LengthKind::Delimited {
            return true;
        }
        matches!(
            self.ctx.program.node(node_id),
            Ok(IrNode::Element {
                child: Some(_),
                props: elem_props,
                ..
            }) if elem_props.length_kind == LengthKind::Delimited
        )
    }

    fn element_consumes_enclosing_delimiter(&self, node_id: u32, props: &IrProps) -> bool {
        if !self.is_delimited_element(node_id, props) {
            return false;
        }
        if matches!(
            self.ctx.program.node(node_id),
            Ok(IrNode::Element { child: Some(_), .. })
        ) {
            return true;
        }
        props.initiator.is_none() && props.terminator.is_none()
    }

    fn is_complex_delimited_element(&self, node_id: u32, props: &IrProps) -> Result<bool> {
        Ok(self.is_delimited_element(node_id, props)
            && matches!(
                self.ctx.program.node(node_id)?,
                IrNode::Element { child: Some(_), .. }
            ))
    }

    fn should_skip_empty_complex_delimited_occurrence(
        &self,
        node_id: u32,
        props: &IrProps,
        parent_sequence: Option<&IrProps>,
        cursor: &Cursor<'_>,
        stop_sequences: &[&IrProps],
    ) -> Result<bool> {
        if !self.is_complex_delimited_element(node_id, props)? {
            return Ok(false);
        }
        let Some(parent) = parent_sequence else {
            return Ok(false);
        };
        let Some(sep_id) = parent.separator else {
            return Ok(false);
        };
        let parent_sep = self.ctx.strings().get(sep_id)?;
        if crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            parent_sep,
            parent.ignore_case,
        )
        .is_none()
        {
            return Ok(false);
        }
        would_read_empty_delimited_field(cursor, props, self.ctx.strings(), stop_sequences)
            .map_err(Into::into)
    }

    fn consume_occurrence_separator(
        &self,
        parent_sequence: Option<&IrProps>,
        cursor: &mut Cursor<'_>,
    ) -> Result<()> {
        let Some(props) = parent_sequence else {
            return Ok(());
        };
        let Some(id) = props.separator else {
            return Ok(());
        };
        let pat = self.ctx.strings().get(id)?;
        if crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            pat,
            props.ignore_case,
        )
        .is_some()
        {
            if !cursor.consume_delimiter(pat, props.ignore_case) {
                return Err(VmError::InvalidValue {
                    message: "separator mismatch".into(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn consume_root_delimited_suffix(&self, cursor: &mut Cursor<'_>) -> Result<()> {
        let node = self.ctx.program.node(self.ctx.program.root)?;
        if let IrNode::Element { props, child, .. } = node {
            if child.is_none() && props.length_kind == LengthKind::Delimited && !cursor.is_empty() {
                consume_enclosing_delimiter(cursor, props, self.ctx.strings(), &[])?;
            }
        }
        Ok(())
    }

    fn consume_initiator(&self, props: &IrProps, cursor: &mut Cursor<'_>) -> Result<()> {
        if let Some(id) = props.initiator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() && !cursor.consume_delimiter(pat, props.ignore_case) {
                return Err(VmError::InvalidValue {
                    message: "initiator mismatch".into(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn consume_terminator(&self, props: &IrProps, cursor: &mut Cursor<'_>) -> Result<()> {
        if let Some(id) = props.terminator {
            let pat = self.ctx.strings().get(id)?;
            if pat.is_empty() {
                return Ok(());
            }
            if !cursor.consume_delimiter(pat, props.ignore_case) {
                if cursor.is_empty() {
                    return Ok(());
                }
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "terminator mismatch: expected `{pat}` at byte 0x{:02x}",
                        cursor.data.get(cursor.pos).copied().unwrap_or(0)
                    ),
                }
                .into());
            }
        }
        Ok(())
    }

    fn consume_separator(
        &self,
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        index: usize,
        total: usize,
        infix_sep_newline_prefix: &mut Vec<bool>,
        stop_sequences: &[&IrProps],
    ) -> Result<Option<u8>> {
        if !should_write_separator(props.separator_position, index, total) {
            return Ok(None);
        }
        if let Some(id) = props.separator {
            let pat = self.ctx.strings().get(id)?;
            if let Some(err) =
                self.separator_enclosing_delimiter_conflict(props, pat, cursor, stop_sequences)
            {
                return Err(err);
            }
            if crate::schema::is_nl_comma_space_pattern(pat)
                && props.separator_position == SeparatorPosition::Infix
                && index > 0
            {
                if let Some((n, had_nl)) =
                    crate::schema::match_nl_comma_space_separator_with_flag(&cursor.data[cursor.pos..])
                {
                    cursor.advance(n);
                    infix_sep_newline_prefix.push(had_nl);
                    return Ok(None);
                }
            }
            if let Some((_, alt)) = cursor.consume_delimiter_with_alt(pat, props.ignore_case) {
                return Ok(Some(alt));
            }
            return Err(VmError::InvalidValue {
                message: "separator mismatch".into(),
            }
            .into());
        }
        Ok(None)
    }

    fn separator_enclosing_delimiter_conflict(
        &self,
        sep_props: &IrProps,
        separator: &str,
        cursor: &Cursor<'_>,
        stop_sequences: &[&IrProps],
    ) -> Option<Error> {
        use crate::error::VmError;
        use crate::schema::SeparatorPosition;
        let position = match sep_props.separator_position {
            SeparatorPosition::Prefix => "prefix",
            SeparatorPosition::Infix => "infix",
            SeparatorPosition::Postfix => "postfix",
        };
        let enclosing = self.enclosing.borrow();
        let enclosing_names = self.enclosing_names.borrow();
        let source = enclosing_names
            .last()
            .map(|n| alloc::format!(" ex:{n}"))
            .unwrap_or_default();
        let mut scans: Vec<&IrProps> = stop_sequences.to_vec();
        scans.extend(enclosing.iter());
        for enc in scans.iter().copied() {
            let Some(term_id) = enc.terminator else {
                continue;
            };
            let term = self.ctx.strings().get(term_id).ok()?;
            if term.len() > separator.len()
                && term.starts_with(separator)
                && crate::schema::match_delimiter_opts(
                    &cursor.data[cursor.pos..],
                    term,
                    enc.ignore_case,
                )
                .is_some()
            {
                return Some(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. {position} separator. Found enclosing delimiter: '{term}'. during scan for local delimiter(s): '{separator}'. Separator '{separator}' from{source}"
                    ),
                }
                .into());
            }
        }
        let sep_n = crate::schema::match_delimiter_opts(
            &cursor.data[cursor.pos..],
            separator,
            sep_props.ignore_case,
        )?;
        if sep_n == 0 {
            return None;
        }
        for enc in scans.iter().copied() {
            let Some(term_id) = enc.terminator else {
                continue;
            };
            let term = self.ctx.strings().get(term_id).ok()?;
            let term_n = crate::schema::match_delimiter_opts(
                &cursor.data[cursor.pos..],
                term,
                enc.ignore_case,
            )?;
            if term_n > sep_n {
                return Some(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. {position} separator. Found enclosing delimiter: '{term}'. during scan for local delimiter(s): '{separator}'. Separator '{separator}' from{source}"
                    ),
                }
                .into());
            }
        }
        None
    }

    fn following_sibling_consumes_input(&self, children: &[u32], idx: usize) -> bool {
        children[idx + 1..]
            .iter()
            .any(|&child| self.particle_consumes_input(child))
    }

    fn particle_consumes_input(&self, node_id: u32) -> bool {
        match self.ctx.program.node(node_id) {
            Ok(IrNode::Element { props, .. }) => {
                props.input_value_calc.is_none() && props.input_value_calc_segments.is_none()
            }
            Ok(IrNode::Sequence { children, .. }) => {
                children.iter().any(|&child| self.particle_consumes_input(child))
            }
            Ok(IrNode::Choice { branches, .. }) => branches
                .iter()
                .any(|branch| self.particle_consumes_input(branch.node)),
            Err(_) => true,
        }
    }

    fn inner_sequence_separator(&self, child_id: u32) -> Result<Option<String>> {
        let node_id = match self.ctx.program.node(child_id)? {
            IrNode::Element { child: Some(id), .. } => *id,
            _ => child_id,
        };
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { props, .. } => Ok(props
                .separator
                .map(|id| self.ctx.strings().get(id).map(|s| s.to_string()))
                .transpose()?),
            _ => Ok(None),
        }
    }
}

fn is_element_absent(err: &Error) -> bool {
    matches!(err, Error::Vm(VmError::ElementAbsent))
}

fn should_populate_array_errors(props: &IrProps) -> bool {
    matches!(
        props.occurs_count_kind,
        OccursCountKind::Implicit | OccursCountKind::Fixed
    ) && (props.occurs_min != 1 || props.occurs_max != Some(1))
}

fn element_prefixed_name(program: &IrProgram, node_id: u32) -> Result<String> {
    match program.node(node_id)? {
        IrNode::Element { name, .. } => {
            let local = program.strings.get(*name)?;
            Ok(alloc::format!("ex:{local}"))
        }
        _ => Ok(alloc::string::String::from("ex:unknown")),
    }
}

fn populate_failed_error(qname: &str, index: u64, cause: &str) -> VmError {
    VmError::InvalidValue {
        message: if cause.is_empty() {
            alloc::format!("Parse Error: Failed to populate {qname}[{index}].")
        } else {
            alloc::format!("Parse Error: Failed to populate {qname}[{index}]. Cause: {cause}")
        },
    }
}

fn element_kind(program: &IrProgram, node_id: u32) -> core::result::Result<ValueKind, VmError> {
    match program.node(node_id)? {
        IrNode::Element { kind, .. } => Ok(*kind),
        _ => Ok(ValueKind::Complex),
    }
}

fn insert_child(
    map: &mut BTreeMap<String, DfdlValue>,
    node_id: u32,
    value: DfdlValue,
    program: &IrProgram,
) -> Result<()> {
    match program.node(node_id)? {
        IrNode::Element { name, props, .. } => {
            if props.hidden {
                return Ok(());
            }
            let key = program.strings.get(*name)?.to_string();
            insert_field(map, key, value);
            Ok(())
        }
        IrNode::Sequence { .. } => {
            if let DfdlValue::Sequence(seq) = value {
                for (k, v) in seq.fields {
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
                match *value {
                    DfdlValue::Sequence(seq) => {
                        for (k, v) in seq.fields {
                            insert_field(map, k, v);
                        }
                    }
                    other => {
                        map.insert(discriminator, other);
                    }
                }
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

fn should_write_separator(position: SeparatorPosition, index: usize, total: usize) -> bool {
    match position {
        SeparatorPosition::Prefix => index < total,
        SeparatorPosition::Infix => index > 0,
        SeparatorPosition::Postfix => index > 0 && index < total,
    }
}

fn insert_field(map: &mut BTreeMap<String, DfdlValue>, key: String, value: DfdlValue) {
    if let Some(existing) = map.remove(&key) {
        map.insert(key, append_value(existing, value));
    } else {
        map.insert(key, value);
    }
}

fn append_value(existing: DfdlValue, value: DfdlValue) -> DfdlValue {
    match existing {
        DfdlValue::Array(mut items) => {
            items.push(value);
            DfdlValue::Array(items)
        }
        other => DfdlValue::Array(alloc::vec![other, value]),
    }
}

fn wrap_root(name: &str, value: DfdlValue) -> DfdlValue {
    match value {
        DfdlValue::Sequence(seq) if seq.fields.contains_key(name) => DfdlValue::Sequence(seq),
        DfdlValue::Sequence(seq) => {
            let mut wrapped = BTreeMap::new();
            wrapped.insert(name.into(), DfdlValue::Sequence(seq));
            DfdlValue::sequence(wrapped)
        }
        DfdlValue::Choice { discriminator, value } => {
            let mut inner = BTreeMap::new();
            inner.insert(discriminator, *value);
            let mut wrapped = BTreeMap::new();
            wrapped.insert(name.into(), DfdlValue::sequence(inner));
            DfdlValue::sequence(wrapped)
        }
        other => {
            let mut map = BTreeMap::new();
            map.insert(name.into(), other);
            DfdlValue::sequence(map)
        }
    }
}

fn wrap_named(name: &str, inner: DfdlValue, kind: ValueKind) -> DfdlValue {
    if kind == ValueKind::Complex {
        if matches!(inner, DfdlValue::Null) {
            return DfdlValue::Null;
        }
        match inner {
            DfdlValue::Sequence(seq) => {
                if !seq.fields.contains_key(name) {
                    // Implicit complex element seq_01: keep inner fields under seq_01 for choice/infoset.
                    if name == "seq_01" {
                        let mut map = BTreeMap::new();
                        map.insert(name.into(), DfdlValue::Sequence(seq));
                        return DfdlValue::sequence(map);
                    }
                    return DfdlValue::Sequence(seq);
                }
                DfdlValue::Sequence(seq)
            }
            DfdlValue::Choice { discriminator, value } => {
                let mut map = BTreeMap::new();
                let key = discriminator.clone();
                if let DfdlValue::Sequence(inner) = *value {
                    if inner.fields.len() == 1 {
                        if let Some(v) = inner.fields.get(&key) {
                            map.insert(key, v.clone());
                        } else if let Some((only_key, only_val)) = inner.fields.iter().next() {
                            map.insert(only_key.clone(), only_val.clone());
                        } else {
                            map.insert(key, DfdlValue::Sequence(inner));
                        }
                    } else {
                        map.insert(key, DfdlValue::Sequence(inner));
                    }
                } else {
                    map.insert(key, *value);
                }
                DfdlValue::sequence(map)
            }
            other => {
                let mut map = BTreeMap::new();
                map.insert(name.into(), other);
                DfdlValue::sequence(map)
            }
        }
    } else {
        inner
    }
}

fn eval_input_value_calc_concat(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
) -> Result<DfdlValue> {
    use crate::ir::IrInputValueCalcSegment;
    let segments = props.input_value_calc_segments.as_ref().ok_or_else(|| VmError::InvalidValue {
        message: "missing inputValueCalc concat segments".into(),
    })?;
    let mut out = alloc::string::String::new();
    for seg in segments {
        match seg {
            IrInputValueCalcSegment::Sibling(id) => {
                let name = strings.get(*id)?;
                out.push_str(&sibling_string_value(siblings, name)?);
            }
            IrInputValueCalcSegment::Substring {
                sibling,
                start,
                length,
            } => {
                let name = strings.get(*sibling)?;
                let text = sibling_string_value(siblings, name)?;
                let start = (*start as usize).saturating_sub(1);
                for ch in text.chars().skip(start).take(*length as usize) {
                    out.push(ch);
                }
            }
        }
    }
    Ok(DfdlValue::String(StringValue::new(out)))
}

fn eval_input_value_calc(
    props: &IrProps,
    kind: ValueKind,
    cursor: &Cursor<'_>,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    content_scope_bytes: Option<usize>,
) -> Result<DfdlValue> {
    let calc = props.input_value_calc.ok_or_else(|| VmError::InvalidValue {
        message: "missing inputValueCalc".into(),
    })?;
    if calc == InputValueCalc::StringLiteral {
        let lit_id = props.input_value_calc_literal.ok_or_else(|| VmError::InvalidValue {
            message: "missing inputValueCalc string literal".into(),
        })?;
        let text = strings.get(lit_id)?;
        if kind == ValueKind::HexBinary {
            let bytes = super::runtime::decode_hex_binary(text)?;
            return Ok(crate::value::DfdlValue::HexBinary(bytes));
        }
        let parsed = super::calendar_binary::parse_xs_calendar_lexical(
            kind,
            props.calendar_date_only,
            text,
        )?;
        return Ok(crate::value::DfdlValue::DateTime(parsed));
    }
    if let InputValueCalc::Constant(v) = calc {
        if kind == ValueKind::Integer && props.non_negative_integer && v < 0 {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Error Cannot convert {v} to NonNegativeInteger"
                ),
            }
            .into());
        }
        return constant_input_value(kind, v);
    }
    if calc == InputValueCalc::BooleanFromSibling {
        if kind != ValueKind::Boolean {
            return Err(VmError::InvalidValue {
                message: "xs:boolean inputValueCalc requires xs:boolean element".into(),
            }
            .into());
        }
        let sib = sibling_state(props, siblings, strings)?;
        let text = dfdl_value_text(&sib.value);
        return super::runtime::parse_xs_boolean_lexical(text)
            .map(DfdlValue::Boolean)
            .map_err(Into::into);
    }
    let len = match calc {
        InputValueCalc::Constant(_) => unreachable!("handled above"),
        InputValueCalc::StringLiteral => unreachable!("handled above"),
        InputValueCalc::BooleanFromSibling => unreachable!("handled above"),
        InputValueCalc::ContentLengthSelf(units) | InputValueCalc::ValueLengthSelf(units) => {
            let byte_len = content_scope_bytes.unwrap_or_else(|| cursor.remaining());
            length_in_units(byte_len, units)?
        }
        InputValueCalc::ContentLengthSibling(units) => {
            let sib = sibling_state(props, siblings, strings)?;
            length_in_units(sib.content_bytes, units)?
        }
        InputValueCalc::ValueLengthSibling(_) => {
            let sib = sibling_state(props, siblings, strings)?;
            value_byte_length(&sib.value)?
        }
    };
    i32::try_from(len)
        .map(DfdlValue::Int)
        .map_err(|_| VmError::InvalidValue {
            message: alloc::format!("inputValueCalc result `{len}` out of range for int"),
        })
        .map_err(Into::into)
}

fn constant_input_value(kind: ValueKind, value: i64) -> Result<DfdlValue> {
    use ValueKind::*;
    match kind {
        Byte => i8::try_from(value)
            .map(DfdlValue::Byte)
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!("inputValueCalc constant `{value}` out of range for byte"),
            }),
        UnsignedByte => u8::try_from(value)
            .map(DfdlValue::UnsignedByte)
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!(
                    "inputValueCalc constant `{value}` out of range for unsignedByte"
                ),
            }),
        Short => i16::try_from(value)
            .map(DfdlValue::Short)
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!(
                    "inputValueCalc constant `{value}` out of range for short"
                ),
            }),
        UnsignedShort => u16::try_from(value)
            .map(DfdlValue::UnsignedShort)
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!(
                    "inputValueCalc constant `{value}` out of range for unsignedShort"
                ),
            }),
        Int => i32::try_from(value)
            .map(DfdlValue::Int)
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!("inputValueCalc constant `{value}` out of range for int"),
            }),
        UnsignedInt => u32::try_from(value)
            .map(DfdlValue::UnsignedInt)
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!(
                    "inputValueCalc constant `{value}` out of range for unsignedInt"
                ),
            }),
        Long => Ok(DfdlValue::Long(value)),
        Integer => Ok(DfdlValue::Integer(value.to_string())),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("inputValueCalc constant unsupported for `{other:?}`"),
        }),
    }
    .map_err(Into::into)
}

fn dfdl_value_text(value: &DfdlValue) -> &str {
    match value {
        DfdlValue::String(v) => &v.text,
        DfdlValue::Boolean(v) => {
            if *v {
                "true"
            } else {
                "false"
            }
        }
        _ => "",
    }
}

fn sibling_state<'a>(
    props: &IrProps,
    siblings: Option<&'a BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
) -> Result<&'a SiblingState> {
    let id = props.input_value_calc_sibling.ok_or_else(|| VmError::InvalidValue {
        message: "inputValueCalc sibling missing".into(),
    })?;
    let name = strings.get(id)?;
    siblings
        .and_then(|m| m.get(name))
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("inputValueCalc sibling `{name}` not available"),
        })
        .map_err(Into::into)
}

fn length_in_units(byte_len: usize, units: LengthUnits) -> Result<usize> {
    match units {
        LengthUnits::Bytes => Ok(byte_len),
        LengthUnits::Bits => Ok(byte_len.saturating_mul(8)),
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "inputValueCalc character units".into(),
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

fn sibling_string_value(
    siblings: Option<&BTreeMap<String, SiblingState>>,
    sib_name: &str,
) -> Result<alloc::string::String> {
    let sib_val = siblings
        .and_then(|m| m.get(sib_name))
        .map(|state| &state.value)
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Schema Definition Error: {sib_name} does not exist"),
        })?;
    match sib_val {
        DfdlValue::String(s) => Ok(s.text.clone()),
        other => Err(VmError::InvalidValue {
            message: alloc::format!(
                "runtime property sibling `{sib_name}` has unsupported type: {other:?}"
            ),
        }
        .into()),
    }
}

#[cfg(test)]
pub(crate) fn resolve_length_props_for_test(
    props: &IrProps,
    kind: ValueKind,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<IrProps> {
    resolve_length_props(props, None, kind, strings, tunables)
}

fn resolve_length_props(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    kind: ValueKind,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<IrProps> {
    let mut resolved = props.clone();

    if props.length_kind == LengthKind::Explicit && props.length.is_none() {
        if let Some(sib_id) = props.length_sibling {
            let sib_name = strings.get(sib_id)?;
            let sib_val = siblings
                .and_then(|m| m.get(sib_name))
                .map(|state| &state.value)
                .ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("length sibling `{sib_name}` not available"),
                })?;
            resolved.length = Some(length_from_value(sib_val, props.length_sibling_cast_long)?);
            if kind == ValueKind::Decimal {
                validate_explicit_decimal_before_decode(kind, &resolved, tunables, strings)?;
            } else if let Some(len) = resolved.length {
                if binary_length_validation_applies(kind, resolved.binary_number_rep) {
                    validate_data_length_vm(
                        kind,
                        len,
                        resolved.length_units,
                        resolved.binary_number_rep,
                    )?;
                }
            }
        }
    }

    if let Some(sib_id) = props.text_standard_decimal_separator_sibling {
        let sib_name = strings.get(sib_id)?;
        let raw = sibling_string_value(siblings, sib_name)?;
        crate::schema::validate_text_standard_separator_literal(
            "textStandardDecimalSeparator",
            &raw,
        )
        .map_err(|detail| VmError::InvalidValue {
            message: alloc::format!("Schema Definition Error: {detail}"),
        })?;
        resolved.resolved_text_standard_decimal_separator =
            Some(crate::schema::expand_entities_str(&raw));
        resolved.text_standard_decimal_separator_defined = true;
    }
    if let Some(sib_id) = props.text_standard_grouping_separator_sibling {
        let sib_name = strings.get(sib_id)?;
        let raw = sibling_string_value(siblings, sib_name)?;
        crate::schema::validate_text_standard_separator_literal(
            "textStandardGroupingSeparator",
            &raw,
        )
        .map_err(|detail| VmError::InvalidValue {
            message: alloc::format!("Schema Definition Error: {detail}"),
        })?;
        let expanded = crate::schema::expand_entities_str(&raw);
        if !raw.contains('%') && expanded.chars().count() != 1 {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Length of string must be exactly 1 character".into(),
            }
            .into());
        }
        resolved.resolved_text_standard_grouping_separator = Some(expanded);
        resolved.text_standard_grouping_separator_defined = true;
    }
    if let Some(sib_id) = props.text_standard_exponent_rep_sibling {
        let sib_name = strings.get(sib_id)?;
        let raw = sibling_string_value(siblings, sib_name)?;
        crate::schema::validate_text_standard_exponent_rep_literal(&raw).map_err(|detail| {
            VmError::InvalidValue {
                message: alloc::format!("Schema Definition Error: {detail}"),
            }
        })?;
        resolved.resolved_text_standard_exponent_rep = Some(raw);
        resolved.text_standard_exponent_rep_defined = true;
    }

    let mut distinct: alloc::vec::Vec<(&str, alloc::string::String)> = alloc::vec::Vec::new();
    if resolved.text_standard_decimal_separator_defined {
        let raw = resolved
            .resolved_text_standard_decimal_separator
            .clone()
            .or_else(|| {
                strings
                    .get(resolved.text_standard_decimal_separator)
                    .ok()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        distinct.push(("textStandardDecimalSeparator", raw));
    }
    if resolved.text_standard_grouping_separator_defined {
        let raw = resolved
            .resolved_text_standard_grouping_separator
            .clone()
            .or_else(|| {
                resolved
                    .text_standard_grouping_separator
                    .and_then(|id| strings.get(id).ok().map(str::to_string))
            })
            .unwrap_or_default();
        distinct.push(("textStandardGroupingSeparator", raw));
    }
    if resolved.text_standard_exponent_rep_defined {
        let raw = resolved
            .resolved_text_standard_exponent_rep
            .clone()
            .or_else(|| {
                strings
                    .get(resolved.text_standard_exponent_rep)
                    .ok()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        distinct.push(("textStandardExponentRep", raw));
    }
    if resolved.text_standard_infinity_rep != StringId(0) {
        let raw = strings
            .get(resolved.text_standard_infinity_rep)
            .unwrap_or("")
            .to_string();
        distinct.push(("textStandardInfinityRep", raw));
    }
    if resolved.text_standard_nan_rep != StringId(0) {
        let raw = strings
            .get(resolved.text_standard_nan_rep)
            .unwrap_or("")
            .to_string();
        distinct.push(("textStandardNaNRep", raw));
    }
    if resolved.text_standard_zero_rep_defined {
        let raw = strings
            .get(resolved.text_standard_zero_rep)
            .unwrap_or("")
            .to_string();
        distinct.push(("textStandardZeroRep", raw));
    }
    if distinct.len() >= 2 {
        let refs: alloc::vec::Vec<(&str, &str)> = distinct
            .iter()
            .map(|(n, s)| (*n, s.as_str()))
            .collect();
        crate::schema::validate_text_standard_distinct_values(&refs).map_err(|msg| {
            VmError::InvalidValue {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
        })?;
    }

    Ok(resolved)
}

fn negative_runtime_length_error(value: i64) -> crate::error::VmError {
    use crate::error::VmError;
    VmError::InvalidValue {
        message: alloc::format!(
            "Runtime Schema Definition Error. dfdl:length expression result must be non-negative, but was: {value}"
        ),
    }
}

fn length_from_value(value: &DfdlValue, cast_long: bool) -> Result<u64> {
    let err = |msg: alloc::string::String| -> Result<u64> {
        Err(VmError::InvalidValue { message: msg }.into())
    };
    match value {
        DfdlValue::Double(v) if cast_long => {
            if v.is_nan() {
                return err("Parse Error. Cannot convert NaN double value to xs:long".into());
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
        other => err(alloc::format!("length sibling has unsupported type: {other:?}")),
    }
}
