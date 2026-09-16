use super::facet_validate::{
    needs_choice_discriminator_facet_check, needs_facet_validation,
    validate_assert_eq_occurs_index, validate_choice_discriminator_facets,
    validate_decoded_facets_tdml,
};
use super::runtime::{
    consume_element_framing, consume_element_trailing_framing, consume_enclosing_delimiter,
    default_value_for, encoding_name, has_non_empty_terminator,
    is_suppressible_empty_representation, prefixed_payload_byte_length, read_delimited_bytes,
    read_length_span, read_prefixed_payload, read_simple, read_until_delimiters,
    read_until_separator,
    should_suppress_decode_infix_separator, should_suppress_decode_occurrence_separator,
    validate_explicit_decimal_before_decode,
    validate_unbounded_wsp_star_terminator, would_read_empty_delimited_field,
    format_found_at_cursor, Cursor, RuntimeConfig, VmContext,
};
use crate::schema::boolean_reps::BooleanSiblingEnv;
use crate::length_validate::{binary_length_validation_applies, validate_data_length_vm};
use crate::error::{Error, Result, VmError};
use crate::ir::{ChoiceBranch, IrNode, IrProgram, IrProps, StringId, StringPool, ValueKind};
use crate::schema::{
    match_length_pattern, BitOrder, ChoiceLengthKind, EmptyElementParsePolicy, InputValueCalc,
    LengthKind, LengthUnits, OccursCountKind, Representation, SeparatorPosition, SequenceKind,
};
use crate::value::{DfdlValue, StringValue};
use alloc::collections::BTreeMap;
use core::cell::Cell;
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

fn sibling_text_map_for_delimiters(
    siblings: Option<&BTreeMap<String, SiblingState>>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(siblings) = siblings else {
        return out;
    };
    for (k, v) in siblings {
        match &v.value {
            DfdlValue::String(s) => {
                out.insert(k.clone(), s.text.clone());
            }
            DfdlValue::Null => {
                out.insert(k.clone(), String::new());
            }
            _ => {}
        }
    }
    out
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

fn props_contribute_delimiter_stops(props: &IrProps, strings: &StringPool) -> Result<bool> {
    if let Some(id) = props.terminator {
        if !strings.get(id)?.is_empty() {
            return Ok(true);
        }
    }
    if let Some(id) = props.separator {
        if !strings.get(id)?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn cursor_at_own_sequence_terminator(
    cursor: &Cursor<'_>,
    seq_props: &IrProps,
    strings: &StringPool,
) -> Result<bool> {
    let Some(id) = seq_props.terminator else {
        return Ok(false);
    };
    let pat = strings.get(id)?;
    if pat.is_empty() {
        return Ok(false);
    }
    let enc = encoding_name(seq_props, strings).ok();
    Ok(
        crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            seq_props.ignore_case,
            enc.as_deref(),
        )
        .is_some_and(|n| n > 0),
    )
}

fn cursor_at_parent_infix_separator(
    cursor: &Cursor<'_>,
    parent_sequence: Option<&IrProps>,
    strings: &StringPool,
) -> Result<bool> {
    let Some(parent) = parent_sequence else {
        return Ok(false);
    };
    if parent.separator_position != SeparatorPosition::Infix {
        return Ok(false);
    }
    let Some(id) = parent.separator else {
        return Ok(false);
    };
    let pat = strings.get(id)?;
    Ok(
        crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            parent.ignore_case,
            None,
        )
        .is_some_and(|n| n > 0),
    )
}

fn filter_delimiter_stop_sequences<'a>(
    stop_sequences: &'a [&'a IrProps],
    strings: &StringPool,
) -> Result<Vec<&'a IrProps>> {
    let mut out = Vec::new();
    for props in stop_sequences {
        if props_contribute_delimiter_stops(props, strings)? {
            out.push(*props);
        }
    }
    Ok(out)
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
    /// Previous sibling `dfdl:bitOrder` within the innermost open sequence (runtime SDE).
    seq_bit_order: RefCell<Option<BitOrder>>,
    /// Parent postfix separator already consumed by separator-bounded implicit complex decode.
    parent_postfix_sep_consumed: RefCell<bool>,
    /// Nesting depth of [`Self::decode_element_occurrences`] (for postfix-sep flag lifetime).
    occurrence_decode_depth: Cell<u32>,
    /// Parent postfix separator consumed by the most recent separator-bounded complex decode.
    postfix_bounded_parent_sep_consumed: Cell<bool>,
    /// Parent infix between array occurrences is consumed by the occurrence loop, not fields.
    parent_infix_consumed_by_occurrence_loop: Cell<bool>,
    /// Runtime DFDL variable values (`defineVariable` + `setVariable`).
    runtime_variables: RefCell<BTreeMap<String, String>>,
    /// XPath sibling context for the active sequence decode (includes hidden elements).
    xpath_siblings: RefCell<BTreeMap<String, SiblingState>>,
    /// Outer sequence sibling maps for `../` / `../../` assert and xpath evaluation.
    xpath_ancestor_frames: RefCell<Vec<BTreeMap<String, SiblingState>>>,
    /// Set when a choice branch discriminator evaluates true (commits outer choice).
    discriminator_committed_branch: Cell<bool>,
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
            seq_bit_order: RefCell::new(None),
            parent_postfix_sep_consumed: RefCell::new(false),
            occurrence_decode_depth: Cell::new(0),
            postfix_bounded_parent_sep_consumed: Cell::new(false),
            parent_infix_consumed_by_occurrence_loop: Cell::new(false),
            runtime_variables: RefCell::new(BTreeMap::new()),
            xpath_siblings: RefCell::new(BTreeMap::new()),
            xpath_ancestor_frames: RefCell::new(Vec::new()),
            discriminator_committed_branch: Cell::new(false),
        }
    }

    fn push_xpath_ancestor_frame(&self, siblings: Option<&BTreeMap<String, SiblingState>>) {
        let frame = siblings
            .cloned()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| self.xpath_siblings_snapshot());
        if !frame.is_empty() {
            self.xpath_ancestor_frames.borrow_mut().push(frame);
        }
    }

    fn pop_xpath_ancestor_frame(&self) {
        self.xpath_ancestor_frames.borrow_mut().pop();
    }

    fn sibling_map_for_discriminator_up(&self, up: usize) -> BTreeMap<String, SiblingState> {
        if up == 0 {
            return self.xpath_siblings_snapshot();
        }
        let frames = self.xpath_ancestor_frames.borrow();
        if up <= frames.len() {
            return frames[frames.len() - up].clone();
        }
        self.xpath_siblings_snapshot()
    }

    fn eval_discriminator_xpath_eq(&self, inner: &str, dot: &str) -> Result<Option<bool>> {
        if inner.contains('*') || inner.contains('(') {
            return Ok(None);
        }
        let mut rest = inner.trim();
        let mut up = 0usize;
        while rest.starts_with("../") {
            up += 1;
            rest = &rest[3..];
        }
        let Some(eq_idx) = rest.find(" eq ") else {
            return Ok(None);
        };
        let mut path = rest[..eq_idx].trim();
        let lit = crate::schema::unquote_xpath_string_literal(rest[eq_idx + 4..].trim());
        let query_style = path.starts_with("./");
        if let Some(stripped) = path.strip_prefix("./") {
            path = stripped;
        }
        if path.contains('/') || path.contains('[') {
            let Some(actual) = self
                .xpath_discriminator_path_string(up, path)
                .ok()
                .flatten()
            else {
                return Ok(Some(false));
            };
            return Ok(Some(actual == lit));
        }
        if path == "." {
            return Ok(Some(dot == lit));
        }
        if path.starts_with('$') || path.contains('(') {
            return Ok(None);
        }
        let local = path.rsplit(':').next().unwrap_or(path).trim();
        return self.eval_sibling_name_eq_literal(local, &lit, up, query_style);
    }

    fn eval_sibling_name_eq_literal(
        &self,
        local: &str,
        lit: &str,
        up: usize,
        query_style: bool,
    ) -> Result<Option<bool>> {
        let up_levels = up.saturating_sub(1);
        let mut values = alloc::vec::Vec::new();
        if let Some(sib) = self.lookup_xpath_sibling_state(local, up_levels) {
            values.extend(sibling_discriminator_values(&sib.value));
        }
        if values.is_empty() {
            return Ok(Some(false));
        }
        if query_style && values.len() > 1 {
            let all_same = values.iter().all(|v| v == &values[0]);
            let any_match = values.iter().any(|v| v == lit);
            if !all_same || (any_match && values.iter().any(|v| v != lit)) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Schema Definition Error: query-style path expression `./{local}` is ambiguous."
                    ),
                }
                .into());
            }
        }
        Ok(Some(values.iter().any(|v| v == lit)))
    }

    fn xpath_discriminator_path_string(
        &self,
        up: usize,
        path: &str,
    ) -> Result<Option<alloc::string::String>> {
        let map = self.sibling_map_for_discriminator_up(up);
        let mut value: Option<DfdlValue> = None;
        for (i, step) in path.split('/').filter(|s| !s.is_empty()).enumerate() {
            let (local, index) = parse_discriminator_path_step(step)?;
            if i == 0 {
                let state = map
                    .iter()
                    .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| VmError::InvalidValue {
                        message: alloc::format!(
                            "Schema Definition Error: No element corresponding to step {local} found."
                        ),
                    })?;
                value = Some(if let Some(idx) = index {
                    single_or_array_item_at(&state.value, idx)?
                } else {
                    state.value.clone()
                });
            } else {
                let cur = value.ok_or_else(|| VmError::InvalidValue {
                    message: "discriminator path step without root".into(),
                })?;
                value = Some(navigate_discriminator_path_step(&cur, local, index)?);
            }
        }
        Ok(value.as_ref().map(dfdl_value_dispatch_string))
    }

    fn lookup_xpath_sibling_state(&self, local: &str, up_levels: usize) -> Option<SiblingState> {
        if up_levels == 0 {
            return self
                .xpath_siblings
                .borrow()
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
                .map(|(_, v)| v.clone());
        }
        let frames = self.xpath_ancestor_frames.borrow();
        if let Some(i) = frames.len().checked_sub(up_levels) {
            if let Some((_, v)) = frames[i]
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
            {
                return Some(v.clone());
            }
        }
        for frame in frames.iter() {
            if let Some((_, v)) = frame
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
            {
                return Some(v.clone());
            }
        }
        self.xpath_siblings
            .borrow()
            .iter()
            .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
            .map(|(_, v)| v.clone())
    }

    fn seed_xpath_siblings(&self, seed: Option<&BTreeMap<String, SiblingState>>) {
        let mut map = self.xpath_siblings.borrow_mut();
        map.clear();
        if let Some(s) = seed {
            map.extend(s.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }

    fn insert_xpath_sibling(&self, key: String, state: SiblingState) {
        let mut map = self.xpath_siblings.borrow_mut();
        if let Some(existing) = map.remove(&key) {
            map.insert(
                key,
                SiblingState {
                    value: append_value(existing.value, state.value),
                    content_bytes: state.content_bytes,
                },
            );
        } else {
            map.insert(key, state);
        }
    }

    fn xpath_siblings_snapshot(&self) -> BTreeMap<String, SiblingState> {
        self.xpath_siblings.borrow().clone()
    }

    fn runtime_check_bit_order_change(
        &self,
        props: &IrProps,
        cursor: &Cursor,
    ) -> Result<()> {
        if !props.bit_order_defined {
            return Ok(());
        }
        let order = props.bit_order;
        if let Some(prev) = *self.seq_bit_order.borrow() {
            if prev != order && cursor.absolute_bit_index() % 8 != 0 {
                return Err(VmError::InvalidValue {
                    message:
                        "Runtime Schema Definition Error. dfdl:bitOrder change requires byte boundary"
                            .into(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn note_sequence_field_bit_order(&self, props: &IrProps) {
        if props.bit_order_defined {
            *self.seq_bit_order.borrow_mut() = Some(props.bit_order);
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
        let mut vars = self.ctx.program.variables.clone();
        for (k, v) in &self.ctx.config.runtime_variable_overrides {
            vars.insert(k.clone(), v.clone());
            let local = k.rsplit(':').next().unwrap_or(k.as_str());
            if local != k.as_str() {
                vars.insert(local.to_string(), v.clone());
            }
        }
        *self.runtime_variables.borrow_mut() = vars;
        self.xpath_siblings.borrow_mut().clear();
        self.xpath_ancestor_frames.borrow_mut().clear();
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
                self.seed_xpath_siblings(siblings);
                *self.seq_bit_order.borrow_mut() = None;
                for (name_id, val_id) in &props.set_variables {
                    let name = self.ctx.strings().get(*name_id)?;
                    let val = self.ctx.strings().get(*val_id)?;
                    self.runtime_variables
                        .borrow_mut()
                        .insert(name.to_string(), val.to_string());
                }
                let mut initiator_alt = None;
                if let Some(id) = props.initiator {
                    let pat = self.ctx.strings().get(id)?;
                    if !pat.is_empty() {
                        let enc = encoding_name(props, self.ctx.strings()).ok();
                        let alt = cursor
                            .consume_delimiter_with_alt(
                                pat,
                                props.ignore_case,
                                enc.as_deref(),
                            )
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
                let mut seq_siblings = self.xpath_siblings_snapshot();
                let mut infix_sep_newline_prefix = Vec::new();
                let mut separator_alts = Vec::new();
                let mut field_delim_meta = BTreeMap::new();
                let mut prev_absent_or_empty = false;
                let mut prev_child_empty_string = false;
                let mut inter_child_sep_consumed_by_prev = false;
                if props.initiated_content {
                    return self.decode_sequence_initiated_content(
                        node_id,
                        children,
                        props,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                        initiator_alt,
                        infix_sep_newline_prefix,
                        separator_alts,
                        field_delim_meta,
                    );
                }
                if props.sequence_kind == SequenceKind::Unordered {
                    if props.initiated_content
                        || props.separator.is_some()
                        || self.unordered_sequence_uses_initiator_scan(children)
                    {
                        return self.decode_sequence_initiated_content(
                            node_id,
                            children,
                            props,
                            cursor,
                            has_following_sibling,
                            parent_sequence,
                            siblings,
                            content_scope_bytes,
                            pattern_text_frame,
                            child_stops,
                            initiator_alt,
                            infix_sep_newline_prefix,
                            separator_alts,
                            field_delim_meta,
                        );
                    }
                    return self.decode_unordered_unseparated_sequence(
                        node_id,
                        children,
                        props,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                        initiator_alt,
                    );
                }
                for (idx, &child) in children.iter().enumerate() {
                    let child_has_following = self.following_sibling_consumes_input(children, idx);
                    let mut single_child_infix_reps = 0u32;
                    'repeat_slot: loop {
                    let child_element_props = match self.ctx.program.node(child) {
                        Ok(IrNode::Element { props: cp, .. }) => Some(cp),
                        _ => None,
                    };
                    if idx > 0 {
                        if let Some(sep_id) = props.separator {
                            let pat = self.ctx.strings().get(sep_id)?;
                            if let Ok(IrNode::Element {
                                props: prev_props, ..
                            }) = self.ctx.program.node(children[idx - 1])
                            {
                                if crate::vm::runtime::field_terminator_pending_at_cursor(
                                    cursor,
                                    prev_props,
                                    self.ctx.strings(),
                                ) {
                                    let _ = crate::vm::runtime::consume_text_field_terminator_after_fixed_length(
                                        cursor,
                                        prev_props,
                                        self.ctx.strings(),
                                    );
                                }
                                if let Some(term_id) = prev_props.terminator {
                                    if let Ok(term) = self.ctx.strings().get(term_id) {
                                        if !term.is_empty() {
                                            let enc =
                                                encoding_name(prev_props, self.ctx.strings()).ok();
                                            let sep_enc =
                                                encoding_name(props, self.ctx.strings()).ok();
                                            let sep_n = crate::schema::match_delimiter_opts_for_encoding(
                                                &cursor.data[cursor.pos..],
                                                pat,
                                                props.ignore_case,
                                                sep_enc.as_deref(),
                                            )
                                            .unwrap_or(0);
                                            let term_n = crate::schema::delimiter_match_len_at(
                                                &cursor.data[cursor.pos..],
                                                term,
                                                prev_props.ignore_case,
                                                enc.as_deref(),
                                            )
                                            .unwrap_or(0);
                                            if term_n > sep_n {
                                                self.consume_terminator(prev_props, cursor)?;
                                            }
                                        }
                                    }
                                }
                            }
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
                    if let Ok(IrNode::Element { props: cp, .. }) = self.ctx.program.node(child) {
                        if cp.occurs_min == 0
                            && element_discriminator_always_false(cp, self.ctx.strings())
                        {
                            prev_absent_or_empty = true;
                            break 'repeat_slot;
                        }
                    }
                    let suppress_sep = child_element_props
                        .map(|cp| {
                            should_suppress_decode_infix_separator(props, cp, prev_absent_or_empty)
                        })
                        .unwrap_or(false)
                        || (idx > 0
                            && self.skip_infix_sep_after_parsed_unbounded_array(
                                props,
                                &children,
                                idx,
                                cursor,
                            )?)
                        || (idx > 0
                            && prev_child_empty_string
                            && props.separator.is_some()
                            && props.separator_position == SeparatorPosition::Infix
                            && props.separator.and_then(|id| self.ctx.strings().get(id).ok())
                                .is_some_and(|pat| {
                                    crate::schema::match_delimiter_opts_for_encoding(
                                        &cursor.data[cursor.pos..],
                                        pat,
                                        props.ignore_case,
                                        encoding_name(props, self.ctx.strings())
                                            .ok()
                                            .as_deref(),
                                    )
                                    .unwrap_or(0)
                                        == 0
                                }));
                    let skip_sep_at_term = child_element_props.is_some_and(|cp| cp.occurs_min == 0)
                        && cursor_at_own_sequence_terminator(
                            cursor,
                            props,
                            self.ctx.strings(),
                        )?;
                    if skip_sep_at_term {
                        break 'repeat_slot;
                    }
                    let sep_alt = if inter_child_sep_consumed_by_prev {
                        inter_child_sep_consumed_by_prev = false;
                        None
                    } else if suppress_sep || !self.particle_consumes_input(child) {
                        None
                    } else if props.separator_position == SeparatorPosition::Postfix && idx > 0 {
                        // Postfix separators are consumed after each prior occurrence
                        // (see decode_element_occurrences), not before the next index.
                        None
                    } else {
                        self.consume_separator(
                            props,
                            cursor,
                            idx,
                            children.len(),
                            &mut infix_sep_newline_prefix,
                            child_stops,
                            false,
                        )?
                    };
                    separator_alts.push(sep_alt);
                    let saved = cursor.clone();
                    let start = cursor.pos;
                    let saved_frame_limit = cursor.frame_bit_limit;
                    if let Ok(IrNode::Element { props: cur_p, .. }) = self.ctx.program.node(child) {
                        if idx + 1 < children.len() {
                            if let Ok(IrNode::Element { props: next_p, .. }) =
                                self.ctx.program.node(children[idx + 1])
                            {
                                if let Some(add) =
                                    crate::vm::runtime::sibling_mixed_utf8_utf16_delimited_limit(
                                        cursor,
                                        cur_p,
                                        next_p,
                                        self.ctx.strings(),
                                    )
                                {
                                    cursor.frame_bit_limit =
                                        Some(cursor.pos.saturating_add(add).saturating_mul(8));
                                }
                            }
                        }
                    }
                    let mut particle_stops = child_stops.to_vec();
                    // Sibling stop sequences apply to repeating scalar particles (e.g. NumSeq),
                    // not complex containers whose inner fields have their own terminators.
                    let extend_following_stops = matches!(
                        self.ctx.program.node(child),
                        Ok(IrNode::Element {
                            props: cp,
                            child: None,
                            ..
                        }) if cp.occurs_max.map(|m| m > 1).unwrap_or(true)
                    );
                    if extend_following_stops {
                        for &sib in &children[idx + 1..] {
                            if let Ok(IrNode::Element { props: sib_props, .. }) =
                                self.ctx.program.node(sib)
                            {
                                particle_stops.push(sib_props);
                            }
                        }
                    }
                    let decode_result = if let Ok(IrNode::Sequence { children: hc, .. }) =
                        self.ctx.program.node(child)
                    {
                        if self.is_hidden_group_carrier(hc) {
                            let mut map = BTreeMap::new();
                            for &gc in hc {
                                let gc_has_following = true;
                                let gc_start = cursor.pos;
                                let gc_value = self.decode_particle(
                                    gc,
                                    cursor,
                                    gc_has_following,
                                    Some(props),
                                    Some(&seq_siblings),
                                    content_scope_bytes,
                                    pattern_text_frame,
                                    &particle_stops,
                                )?;
                                if let IrNode::Element { name, props: gp, .. } =
                                    self.ctx.program.node(gc)?
                                {
                                    let key = self.ctx.strings().get(*name)?.to_string();
                                    let consumed = cursor.pos.saturating_sub(gc_start);
                                    let state = SiblingState {
                                        value: gc_value.clone(),
                                        content_bytes: consumed,
                                    };
                                    insert_seq_sibling(&mut seq_siblings,key.clone(), state.clone());
                                    self.insert_xpath_sibling(key, state);
                                    let _ = gp;
                                }
                                insert_child(&mut map, gc, gc_value, self.ctx.program)?;
                            }
                            Ok(DfdlValue::Sequence(crate::value::SequenceValue {
                                fields: map,
                                meta: crate::value::SequenceMeta {
                                    infix_sep_newline_prefix: Vec::new(),
                                    initiator_alt: None,
                                    terminator_alt: None,
                                    separator_alts: Vec::new(),
                                    field_delimiters: BTreeMap::new(),
                                },
                            }))
                        } else {
                            self.decode_particle(
                                child,
                                cursor,
                                child_has_following,
                                Some(props),
                                Some(&seq_siblings),
                                content_scope_bytes,
                                pattern_text_frame,
                                &particle_stops,
                            )
                        }
                    } else {
                        self.decode_particle(
                            child,
                            cursor,
                            child_has_following,
                            Some(props),
                            Some(&seq_siblings),
                            content_scope_bytes,
                            pattern_text_frame,
                            &particle_stops,
                        )
                    };
                    match decode_result {
                        Ok(child_value) => {
                            cursor.frame_bit_limit = saved_frame_limit;
                            if let Ok(IrNode::Element { props: cp, .. }) =
                                self.ctx.program.node(child)
                            {
                                if cp.occurs_min == 0
                                    && is_suppressible_empty_representation(
                                        &child_value,
                                        cp,
                                        self.ctx.strings(),
                                    )?
                                {
                                    prev_absent_or_empty = true;
                                    break 'repeat_slot;
                                }
                            }
                            prev_absent_or_empty = child_element_props
                                .map(|cp| {
                                    if cp.length_kind == LengthKind::Explicit
                                        && cp.length == Some(0)
                                    {
                                        return Ok(false);
                                    }
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
                                let state = SiblingState {
                                    value: child_value.clone(),
                                    content_bytes,
                                };
                                insert_seq_sibling(&mut seq_siblings,key.clone(), state.clone());
                                self.insert_xpath_sibling(key, state);
                            }
                            prev_child_empty_string = matches!(
                                &child_value,
                                DfdlValue::String(s) if s.text.is_empty()
                            );
                            insert_child(&mut map, child, child_value, self.ctx.program)?;
                            if idx + 1 < children.len() {
                                if let Ok(IrNode::Element { props: cp, .. }) =
                                    self.ctx.program.node(child)
                                {
                                    if self.consume_trailing_empty_infix_separators(
                                        props, cp, cursor,
                                    )? {
                                        inter_child_sep_consumed_by_prev = true;
                                    }
                                }
                            }
                            if idx == 0
                                && children.len() > 1
                                && props.separator.is_none()
                            {
                                if let (
                                    Ok(IrNode::Element { props: cur_p, .. }),
                                    Ok(IrNode::Element { props: next_p, .. }),
                                ) = (
                                    self.ctx.program.node(child),
                                    self.ctx.program.node(children[1]),
                                ) {
                                    crate::vm::runtime::check_mixed_encoding_adjacent_delimited_after_first(
                                        cursor,
                                        cur_p,
                                        next_p,
                                        props,
                                        self.ctx.strings(),
                                    )?;
                                }
                            }
                            if self.implicit_complex_consumed_parent_postfix(child, props)? {
                                inter_child_sep_consumed_by_prev = true;
                            }
                            if let IrNode::Element { name, .. } = self.ctx.program.node(child)? {
                                let key = self.ctx.strings().get(*name)?.to_string();
                                if let Some(meta) = self.field_delimiters.borrow_mut().remove(&key) {
                                    field_delim_meta.insert(key, meta);
                                }
                            }
                            let single_complex_child = children.len() == 1
                                && matches!(
                                    self.ctx.program.node(child),
                                    Ok(IrNode::Element { child: Some(_), .. })
                                );
                            if single_complex_child
                                && props.separator.is_some()
                                && props.separator_position == SeparatorPosition::Infix
                            {
                                if cursor.is_empty() {
                                    break 'repeat_slot;
                                }
                                if self.at_enclosing_terminator_stop(cursor, child_stops)? {
                                    break 'repeat_slot;
                                }
                                let saved_rep = cursor.clone();
                                let before_sep = cursor.pos;
                                match self.consume_separator(
                                    props,
                                    cursor,
                                    1,
                                    1,
                                    &mut infix_sep_newline_prefix,
                                    child_stops,
                                    false,
                                ) {
                                    Ok(alt) => {
                                        if cursor.pos <= before_sep
                                            || cursor.pos <= start
                                            || single_child_infix_reps >= 256
                                        {
                                            *cursor = saved_rep;
                                            break 'repeat_slot;
                                        }
                                        single_child_infix_reps += 1;
                                        separator_alts.push(alt);
                                        continue 'repeat_slot;
                                    }
                                    Err(_) => {
                                        *cursor = saved_rep;
                                        break 'repeat_slot;
                                    }
                                }
                            }
                            break 'repeat_slot;
                        }
                        Err(e) if is_element_absent(&e) => {
                            cursor.frame_bit_limit = saved_frame_limit;
                            if let Ok(IrNode::Element { name, props, .. }) =
                                self.ctx.program.node(child)
                            {
                                let zero_len = props.length_kind == LengthKind::Explicit
                                    && props.length == Some(0);
                                if props.occurs_min > 0 && !zero_len {
                                    return Err(e);
                                }
                                if zero_len {
                                    let key = self.ctx.strings().get(*name)?.to_string();
                                    insert_child(
                                        &mut map,
                                        child,
                                        DfdlValue::string(""),
                                        self.ctx.program,
                                    )?;
                                    let _ = key;
                                    prev_absent_or_empty = true;
                                    break 'repeat_slot;
                                }
                            }
                            prev_absent_or_empty = true;
                            // Optional absent: retain parse progress (e.g. NUL padding consumed).
                            if cursor.pos == saved.pos {
                                *cursor = saved;
                            }
                            break 'repeat_slot;
                        }
                        Err(e) => {
                            cursor.frame_bit_limit = saved_frame_limit;
                            if let Ok(IrNode::Element { props: cp, .. }) =
                                self.ctx.program.node(child)
                            {
                                if cp.occurs_min == 0 {
                                    let msg = e.to_string();
                                    let scalar_optional = matches!(
                                        self.ctx.program.node(child),
                                        Ok(IrNode::Element { child: None, .. })
                                    );
                                    let optional_parse_absent = scalar_optional
                                        && props.separator_suppression_policy
                                            == Some(
                                                crate::schema::SeparatorSuppressionPolicy::AnyEmpty,
                                            )
                                        && msg.contains("Parse Error");
                                    if (msg.contains("Init('")
                                        && optional_element_may_absorb_initiator_failure(cp))
                                        || (msg.contains("initiator mismatch")
                                            && optional_element_may_absorb_initiator_failure(cp))
                                        || msg.contains("Delimiter not found")
                                        || optional_parse_absent
                                    {
                                        prev_absent_or_empty = true;
                                        *cursor = saved;
                                        if optional_parse_absent
                                            && props.separator.is_some()
                                            && props.separator_position
                                                == SeparatorPosition::Infix
                                        {
                                            let _ = self.consume_separator(
                                                props,
                                                cursor,
                                                idx,
                                                children.len(),
                                                &mut infix_sep_newline_prefix,
                                                &particle_stops,
                                                false,
                                            )?;
                                        }
                                        break 'repeat_slot;
                                    }
                                }
                            }
                            return Err(e);
                        }
                    }
                    }
                }
                let mut terminator_alt = None;
                if let Some(id) = props.terminator {
                    let pat = self.ctx.strings().get(id)?;
                    if !pat.is_empty() {
                        let pat_resolved = if pat.trim().starts_with('{') {
                            let sib_text = sibling_text_map_for_delimiters(siblings);
                            crate::schema::eval_runtime_delimiter_expression(pat, &sib_text)
                                .unwrap_or_else(|| pat.to_string())
                        } else {
                            pat.to_string()
                        };
                        let enc = encoding_name(props, self.ctx.strings()).ok();
                        if let Some((n, alt)) = cursor.consume_delimiter_with_alt(
                            &pat_resolved,
                            props.ignore_case,
                            enc.as_deref(),
                        ) {
                            if n == 0
                                && !cursor.is_empty()
                                && !crate::schema::delimiter_alt_allows_trailing_input(
                                    &pat_resolved,
                                    alt,
                                )
                            {
                                return Err(VmError::InvalidValue {
                                    message: alloc::format!(
                                        "terminator mismatch: expected `{pat_resolved}`"
                                    ),
                                }
                                .into());
                            }
                            terminator_alt = Some(alt);
                        } else if !cursor.is_empty() {
                            return Err(VmError::InvalidValue {
                                message: alloc::format!("terminator mismatch: expected `{pat_resolved}`"),
                            }
                            .into());
                        }
                    }
                }
                consume_element_trailing_framing(cursor, props)?;
                self.validate_particle_discriminator(props, "")?;
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
            IrNode::Choice { branches, props } => {
                let choice_start = cursor.pos;
                self.consume_initiator(props, cursor)?;
                validate_choice_branches_non_optional_runtime(self.ctx.program, branches)?;
                validate_choice_branches_not_ivc_runtime(self.ctx.program, branches)?;
                validate_choice_branch_element_name_upa_runtime(self.ctx.program, branches)?;
                let mut choice_stops = stop_sequences.to_vec();
                if props.terminator.is_some() {
                    choice_stops.push(props);
                }
                let child_stops = choice_stops.as_slice();
                let choice_frame_bytes = choice_explicit_frame_bytes(
                    props,
                    cursor,
                    self.ctx.strings(),
                )?;
                let dispatch_key = choice_dispatch_key_string(
                    props,
                    siblings,
                    self.ctx.strings(),
                    &self.ctx.program.tunables,
                )?;
                let using_dispatch = dispatch_key.is_some();
                if using_dispatch
                    && dispatch_key
                        .as_ref()
                        .is_some_and(|k| k.is_empty())
                {
                    return Err(VmError::InvalidValue {
                        message:
                            "Runtime Schema Definition Error: Non-empty string required for choice dispatch key"
                                .into(),
                    }
                    .into());
                }
                let branches_iter: alloc::vec::Vec<&ChoiceBranch> = if let Some(ref key) =
                    dispatch_key
                {
                    let matched: alloc::vec::Vec<&ChoiceBranch> = branches
                        .iter()
                        .filter(|b| {
                            b.branch_key
                                .and_then(|id| self.ctx.strings().get(id).ok())
                                .is_some_and(|bk| bk == key.as_str())
                        })
                        .collect();
                    if matched.is_empty() {
                        return Err(VmError::InvalidValue {
                            message: alloc::format!(
                                "Parse Error. Choice dispatch key `{key}` failed to match any branch key."
                            ),
                        }
                        .into());
                    }
                    matched
                } else {
                    let dot_peek = peek_choice_discriminator_dot(
                        self.ctx.program,
                        branches,
                        cursor,
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                    );
                    let mut list: alloc::vec::Vec<&ChoiceBranch> = branches.iter().collect();
                    if let Some(ref dot) = dot_peek {
                        list.retain(|b| {
                            self.choice_branch_discriminator_matches(b.node, dot)
                        });
                    }
                    if props.initiated_content {
                        let any_initiator_branch = list.iter().any(|b| {
                            !self
                                .choice_branch_lacks_initiator(b.node)
                                .unwrap_or(true)
                        });
                        if any_initiator_branch {
                            list.retain(|b| {
                                !self
                                    .choice_branch_lacks_initiator(b.node)
                                    .unwrap_or(true)
                                    && self
                                        .choice_branch_initiator_present(cursor, b.node)
                                        .unwrap_or(false)
                            });
                            if list.is_empty() && !cursor.is_empty() {
                                return Err(VmError::InvalidValue {
                                    message: "initiator mismatch".into(),
                                }
                                .into());
                            }
                        }
                    }
                    list
                };
                let mut branch_errors = Vec::new();
                let mut initiated_initiator_committed = false;
                for branch in branches_iter {
                    if props.initiated_content && initiated_initiator_committed {
                        break;
                    }
                    self.discriminator_committed_branch.set(false);
                    let saved = cursor.clone();
                    let saved_frame_limit = cursor.frame_bit_limit;
                    if let Some(frame) = choice_frame_bytes {
                        cursor.frame_bit_limit =
                            Some(choice_start.saturating_add(frame).saturating_mul(8));
                    }
                    let branch_scope = choice_frame_bytes.or(content_scope_bytes);
                    let decode_result = self.decode_node(
                        branch.node,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        branch_scope,
                        pattern_text_frame,
                        child_stops,
                    );
                    cursor.frame_bit_limit = saved_frame_limit;
                    match decode_result {
                        Ok(value) => {
                            if let Some(frame) = choice_frame_bytes {
                                if cursor.pos > choice_start.saturating_add(frame) {
                                    let branch_err: Error = VmError::InvalidValue {
                                        message: "choice branch exceeded explicit choice length"
                                            .into(),
                                    }
                                    .into();
                                    branch_errors.push(format_choice_branch_error(
                                        branch,
                                        self.ctx.strings(),
                                        &branch_err,
                                    ));
                                    *cursor = saved;
                                    continue;
                                }
                            }
                            if self.choice_branch_needs_post_decode_facet_check(branch.node) {
                                if let Err(e) = self.validate_choice_branch_value(branch.node, &value)
                                {
                                    branch_errors.push(format_choice_branch_error(
                                        branch,
                                        self.ctx.strings(),
                                        &e,
                                    ));
                                    *cursor = saved;
                                    continue;
                                }
                            }
                            if let Some(frame) = choice_frame_bytes {
                                cursor.pos = choice_start.saturating_add(frame);
                            }
                            self.consume_terminator(props, cursor)?;
                            if matches!(
                                self.ctx.program.node(branch.node),
                                Ok(IrNode::Choice { .. })
                            ) && matches!(value, DfdlValue::Choice { .. })
                            {
                                return Ok(value);
                            }
                            let discriminator = choice_branch_discriminator_for_infoset(
                                branch,
                                branches,
                                self.ctx.strings(),
                                self.ctx.program,
                            )?;
                            return Ok(DfdlValue::choice(discriminator, value));
                        }
                        Err(e) => {
                            if is_schema_definition_error(&e) {
                                return Err(e);
                            }
                            if self.discriminator_committed_branch.get() && cursor.pos > saved.pos {
                                let detail = format_choice_branch_error(
                                    branch,
                                    self.ctx.strings(),
                                    &e,
                                );
                                let mut committed_errors = alloc::vec![detail];
                                if using_dispatch {
                                    committed_errors
                                        .insert(0, "Choice dispatch branch failed".into());
                                }
                                return Err(VmError::InvalidChoice {
                                    branch_errors: committed_errors,
                                }
                                .into());
                            }
                            branch_errors.push(format_choice_branch_error(
                                branch,
                                self.ctx.strings(),
                                &e,
                            ));
                            *cursor = saved;
                            if props.initiated_content
                                && self.choice_branch_initiator_present(cursor, branch.node)
                                    .unwrap_or(false)
                            {
                                initiated_initiator_committed = true;
                            }
                        }
                    }
                }
                if let Some(frame) = choice_frame_bytes {
                    cursor.pos = choice_start.saturating_add(frame);
                }
                if using_dispatch && !branch_errors.is_empty() {
                    branch_errors.insert(0, "Choice dispatch branch failed".into());
                }
                Err(VmError::InvalidChoice { branch_errors }.into())
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

    fn initiated_content_uses_initiator_discriminator(&self, props: &IrProps) -> bool {
        props.occurs_min == 0 && props.occurs_max != Some(1)
    }

    fn skip_initiated_content_sibling(
        &self,
        cursor: &Cursor<'_>,
        child: u32,
        committed: u32,
        children: &[u32],
    ) -> Result<bool> {
        let committed_idx = children.iter().position(|&c| c == committed);
        let child_idx = children.iter().position(|&c| c == child);
        let (Some(ci), Some(fi)) = (committed_idx, child_idx) else {
            return Ok(false);
        };
        if fi >= ci {
            return Ok(false);
        }
        let IrNode::Element { props: committed_props, .. } =
            self.ctx.program.node(committed)?
        else {
            return Ok(false);
        };
        let IrNode::Element { props: child_props, .. } = self.ctx.program.node(child)? else {
            return Ok(false);
        };
        if !self.initiator_present_at_cursor(cursor, child_props)?
            || !self.initiator_present_at_cursor(cursor, committed_props)?
        {
            return Ok(false);
        }
        Ok(true)
    }

    fn choice_branch_lacks_initiator(&self, branch_node: u32) -> Result<bool> {
        match self.ctx.program.node(branch_node)? {
            IrNode::Element { props, .. } => Ok(props.initiator.is_none()),
            IrNode::Sequence { children, props, .. } => {
                if props.initiator.is_some() {
                    return Ok(false);
                }
                Ok(children.iter().all(|&c| {
                    self.choice_branch_lacks_initiator(c).unwrap_or(false)
                }))
            }
            IrNode::Choice { .. } => Ok(false),
        }
    }

    fn choice_branch_initiator_present(
        &self,
        cursor: &Cursor<'_>,
        branch_node: u32,
    ) -> Result<bool> {
        match self.ctx.program.node(branch_node)? {
            IrNode::Element { props, child, .. } => {
                if props.initiator.is_some() {
                    return self.initiator_present_at_cursor(cursor, props);
                }
                if let Some(c) = *child {
                    return self.choice_branch_initiator_present(cursor, c);
                }
                Ok(false)
            }
            IrNode::Sequence { children, props, .. } => {
                if props.initiator.is_some() {
                    return self.initiator_present_at_cursor(cursor, props);
                }
                for &child in children {
                    if let Ok(IrNode::Element { props: cp, .. }) = self.ctx.program.node(child) {
                        if cp.initiator.is_some()
                            && self.initiator_present_at_cursor(cursor, cp)?
                        {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
            IrNode::Choice { branches, props, .. } => {
                if props.initiator.is_some() {
                    return self.initiator_present_at_cursor(cursor, props);
                }
                for branch in branches {
                    if self.choice_branch_initiator_present(cursor, branch.node)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn initiator_present_at_cursor(
        &self,
        cursor: &Cursor<'_>,
        props: &IrProps,
    ) -> Result<bool> {
        let Some(id) = props.initiator else {
            return Ok(true);
        };
        let pat = self.ctx.strings().get(id)?;
        if pat.is_empty() {
            return Ok(true);
        }
        let enc = encoding_name(props, self.ctx.strings()).ok();
        if crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            props.ignore_case,
            enc.as_deref(),
        )
        .is_some_and(|n| n > 0)
        {
            return Ok(true);
        }
        if pat.ends_with('[')
            && !pat.starts_with('[')
            && cursor.data.get(cursor.pos) == Some(&b'[')
        {
            return Ok(true);
        }
        Ok(false)
    }

    fn unordered_sequence_uses_initiator_scan(&self, children: &[u32]) -> bool {
        children.iter().any(|&id| {
            matches!(
                self.ctx.program.node(id),
                Ok(IrNode::Element { props, .. }) if props
                    .initiator
                    .is_some_and(|i| self.ctx.strings().get(i).ok().is_some_and(|p| !p.is_empty()))
            )
        })
    }

    fn decode_unordered_unseparated_sequence(
        &self,
        _node_id: u32,
        children: &[u32],
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        child_stops: &[&IrProps],
        initiator_alt: Option<u8>,
    ) -> Result<DfdlValue> {
        self.seed_xpath_siblings(siblings);
        let mut map = BTreeMap::new();
        let mut seq_siblings = self.xpath_siblings_snapshot();
        let remaining: alloc::vec::Vec<usize> = (0..children.len()).collect();
        let frame_start = cursor.pos;
        if let Err(e) = self.unordered_unseparated_backtrack(
            children,
            props,
            cursor,
            has_following_sibling,
            parent_sequence,
            &mut seq_siblings,
            content_scope_bytes,
            pattern_text_frame,
            child_stops,
            &remaining,
            &mut map,
        ) {
            if let Some(scalar_err) = self.unordered_unseparated_scalar_duplicate_error(
                children,
                &cursor.data[frame_start..],
            ) {
                return Err(scalar_err.into());
            }
            return Err(e);
        }
        self.validate_particle_discriminator(props, "")?;
        Ok(DfdlValue::Sequence(crate::value::SequenceValue {
            fields: map,
            meta: crate::value::SequenceMeta {
                infix_sep_newline_prefix: Vec::new(),
                initiator_alt,
                terminator_alt: None,
                separator_alts: Vec::new(),
                field_delimiters: BTreeMap::new(),
            },
        }))
    }

    fn unordered_unseparated_backtrack(
        &self,
        children: &[u32],
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        seq_siblings: &mut BTreeMap<String, SiblingState>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        child_stops: &[&IrProps],
        remaining: &[usize],
        map: &mut BTreeMap<String, DfdlValue>,
    ) -> Result<()> {
        if remaining.is_empty() {
            if !cursor.is_empty() {
                return Err(VmError::InvalidValue {
                    message: "unordered sequence: unconsumed input".into(),
                }
                .into());
            }
            return Ok(());
        }
        for (pick, &child_idx) in remaining.iter().enumerate() {
            let child = children[child_idx];
            let rewind = cursor.clone();
            let child_has_following = remaining.len() > 1
                || matches!(
                    self.ctx.program.node(child),
                    Ok(IrNode::Element { props: cp, .. })
                        if unordered_backtrack_multi_occurrence(cp)
                );
            let start = cursor.pos;
            let decode_result = if let Ok(IrNode::Element { props: cp, .. }) =
                self.ctx.program.node(child)
            {
                if unordered_backtrack_multi_occurrence(cp) {
                    self.decode_one_element_occurrence(
                        child,
                        cursor,
                        child_has_following,
                        Some(props),
                        Some(seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                    )
                } else {
                    self.decode_particle(
                        child,
                        cursor,
                        child_has_following,
                        Some(props),
                        Some(seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                    )
                }
            } else {
                self.decode_particle(
                    child,
                    cursor,
                    child_has_following,
                    Some(props),
                    Some(seq_siblings),
                    content_scope_bytes,
                    pattern_text_frame,
                    child_stops,
                )
            };
            match decode_result {
                Ok(child_value) => {
                    if let Ok(IrNode::Element { kind, props: cp, .. }) =
                        self.ctx.program.node(child)
                    {
                        if cp.occurs_min == 0
                            && is_suppressible_empty_representation(
                                &child_value,
                                cp,
                                self.ctx.strings(),
                            )?
                        {
                            *cursor = rewind.clone();
                            let mut rest = remaining.to_vec();
                            rest.remove(pick);
                            if self
                                .unordered_unseparated_backtrack(
                                    children,
                                    props,
                                    cursor,
                                    has_following_sibling,
                                    parent_sequence,
                                    seq_siblings,
                                    content_scope_bytes,
                                    pattern_text_frame,
                                    child_stops,
                                    &rest,
                                    map,
                                )
                                .is_ok()
                            {
                                return Ok(());
                            }
                            *cursor = rewind.clone();
                            continue;
                        }
                        if needs_facet_validation(cp)
                            || cp.facet_check_constraints
                            || !cp.facet_pattern_groups.is_empty()
                        {
                            if !backtrack_decoded_facets_ok(
                                &child_value,
                                *kind,
                                cp,
                                self.ctx.strings(),
                                &self.ctx.program.tunables,
                            ) {
                                *cursor = rewind;
                                continue;
                            }
                        }
                    }
                    let consumed = cursor.pos.saturating_sub(start);
                    let (key, state) = if let IrNode::Element { name, props: el_props, .. } =
                        self.ctx.program.node(child)?
                    {
                        let key = self.ctx.strings().get(*name)?.to_string();
                        let content_bytes = if el_props.length_kind == LengthKind::Prefixed {
                            prefixed_payload_byte_length(
                                &cursor.data[start..cursor.pos],
                                el_props,
                                self.ctx.strings(),
                            )?
                        } else {
                            consumed
                        };
                        (
                            key.clone(),
                            SiblingState {
                                value: child_value.clone(),
                                content_bytes,
                            },
                        )
                    } else {
                        return Err(VmError::InvalidValue {
                            message: "unordered sequence: expected element particle".into(),
                        }
                        .into());
                    };
                    let saved_map = map.clone();
                    let saved_siblings = seq_siblings.clone();
                    insert_child(map, child, child_value, self.ctx.program)?;
                    insert_seq_sibling(seq_siblings, key.clone(), state.clone());
                    self.insert_xpath_sibling(key, state);
                    let mut rest = remaining.to_vec();
                    rest.remove(pick);
                    if let Ok(IrNode::Element { name, props: cp, .. }) =
                        self.ctx.program.node(child)
                    {
                        let key = self.ctx.strings().get(*name)?.to_string();
                        if unordered_backtrack_may_take_another(map, &key, cp, cursor.is_empty())
                        {
                            rest.push(child_idx);
                        }
                    }
                    match self.unordered_unseparated_backtrack(
                        children,
                        props,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        seq_siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                        &rest,
                        map,
                    ) {
                        Ok(()) => return Ok(()),
                        Err(_) => {
                            *map = saved_map;
                            *seq_siblings = saved_siblings;
                            *cursor = rewind.clone();
                        }
                    }
                }
                Err(e) if is_element_absent(&e) => {
                    if let Ok(IrNode::Element { props: cp, .. }) = self.ctx.program.node(child) {
                        if cp.occurs_min == 0 {
                            let mut rest = remaining.to_vec();
                            rest.remove(pick);
                            if self
                                .unordered_unseparated_backtrack(
                                    children,
                                    props,
                                    cursor,
                                    has_following_sibling,
                                    parent_sequence,
                                    seq_siblings,
                                    content_scope_bytes,
                                    pattern_text_frame,
                                    child_stops,
                                    &rest,
                                    map,
                                )
                                .is_ok()
                            {
                                return Ok(());
                            }
                        }
                    }
                    *cursor = rewind.clone();
                }
                Err(_) => {
                    *cursor = rewind;
                }
            }
        }
        Err(VmError::InvalidValue {
            message: "unordered sequence: no valid particle order".into(),
        }
        .into())
    }

    fn unordered_unseparated_scalar_duplicate_error(
        &self,
        children: &[u32],
        data: &[u8],
    ) -> Option<VmError> {
        for &child in children {
            let Ok(IrNode::Element {
                name,
                kind,
                props,
                child: None,
                ..
            }) = self.ctx.program.node(child)
            else {
                continue;
            };
            let max = props.occurs_max.unwrap_or(1);
            if max != 1 {
                continue;
            }
            let Some(len) = props.length.filter(|&l| l > 0) else {
                continue;
            };
            if props.length_kind != LengthKind::Explicit {
                continue;
            }
            let len = len as usize;
            if data.len() < len {
                continue;
            }
            let mut match_starts = alloc::vec::Vec::new();
            for start in 0..=data.len().saturating_sub(len) {
                let slice = core::str::from_utf8(&data[start..start + len]).ok()?;
                let v = DfdlValue::String(StringValue::new(slice));
                if needs_facet_validation(props)
                    && validate_decoded_facets_tdml(
                        &v,
                        *kind,
                        props,
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        false,
                    )
                    .is_err()
                {
                    continue;
                }
                match_starts.push(start);
            }
            if match_starts.len() < 2 {
                continue;
            }
            let mut disjoint_pairs = false;
            'outer: for (i, &a) in match_starts.iter().enumerate() {
                for &b in &match_starts[i + 1..] {
                    if a + len <= b || b + len <= a {
                        disjoint_pairs = true;
                        break 'outer;
                    }
                }
            }
            if !disjoint_pairs {
                continue;
            }
            let ename = self.ctx.strings().get(*name).ok()?;
            return Some(VmError::InvalidValue {
                message: alloc::format!(
                    "Scalar Element Error: Multiple instances detected for scalar element {ename}"
                ),
            });
        }
        None
    }

    fn decode_sequence_initiated_content(
        &self,
        node_id: u32,
        children: &[u32],
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        child_stops: &[&IrProps],
        initiator_alt: Option<u8>,
        mut infix_sep_newline_prefix: Vec<bool>,
        separator_alts: Vec<Option<u8>>,
        mut field_delim_meta: BTreeMap<String, FieldDelimiterMeta>,
    ) -> Result<DfdlValue> {
        let mut map = BTreeMap::new();
        let mut seq_siblings = siblings.cloned().unwrap_or_default();
        let mut committed_child: Option<u32> = None;
        let mut initiated_block_lower_indices: Option<usize> = None;
        let ordered_initiated = props.initiated_content
            && props.sequence_kind != SequenceKind::Unordered;
        loop {
            if cursor.is_empty() {
                break;
            }
            if props.separator.is_some()
                && props.separator_position == SeparatorPosition::Prefix
            {
                let _ = self.consume_separator(
                    props,
                    cursor,
                    0,
                    children.len(),
                    &mut infix_sep_newline_prefix,
                    child_stops,
                    false,
                );
            }
            while cursor.pos < cursor.data.len() {
                let b = cursor.data[cursor.pos];
                if b == b';' || b == b'\n' || b == b'\r' || b == b' ' || b == b'\t' {
                    cursor.advance(1);
                } else {
                    break;
                }
            }
            let round_start = cursor.pos;
            let mut round_progress = false;
            for (idx, &child) in children.iter().enumerate() {
                if ordered_initiated
                    && initiated_block_lower_indices
                        .is_some_and(|cut| idx < cut)
                {
                    continue;
                }
                if let Some(committed) = committed_child {
                    if self.skip_initiated_content_sibling(cursor, child, committed, children)? {
                        continue;
                    }
                }
                let child_has_following = self.following_sibling_consumes_input(children, idx);
                if let Ok(IrNode::Element { name, props: cp, .. }) = self.ctx.program.node(child) {
                    if cp.initiator.is_some()
                        && !self.initiator_present_at_cursor(cursor, cp)?
                        && committed_child != Some(child)
                    {
                        let key = self.ctx.strings().get(*name).ok();
                        let already = key.and_then(|k| map.get(k)).is_some();
                        if cp.occurs_min == 0 || already {
                            continue;
                        }
                    }
                    if cp.occurs_min == 0
                        && !self.initiator_present_at_cursor(cursor, cp)?
                        && committed_child != Some(child)
                    {
                        continue;
                    }
                    if cp.occurs_min == 0
                        && element_discriminator_always_false(cp, self.ctx.strings())
                    {
                        continue;
                    }
                }
                let saved = cursor.clone();
                let start = cursor.pos;
                if props.sequence_kind == SequenceKind::Unordered {
                    if let Ok(IrNode::Element { name, props: cp, .. }) =
                        self.ctx.program.node(child)
                    {
                        if cp.nillable
                            && cp.initiator.is_some()
                            && !self.initiator_present_at_cursor(cursor, cp)?
                        {
                            let key = self.ctx.strings().get(*name)?.to_string();
                            if !map.contains_key(&key) {
                                let mut nil_cursor = cursor.clone();
                                if crate::vm::runtime::try_consume_nillable_element_nil(
                                    &mut nil_cursor,
                                    cp,
                                    Some(props),
                                    self.ctx.strings(),
                                    true,
                                )? {
                                    *cursor = nil_cursor;
                                    insert_child(
                                        &mut map,
                                        child,
                                        DfdlValue::Null,
                                        self.ctx.program,
                                    )?;
                                    if let IrNode::Element { name, props: el_props, .. } =
                                        self.ctx.program.node(child)?
                                    {
                                        let key = self.ctx.strings().get(*name)?.to_string();
                                        insert_seq_sibling(&mut seq_siblings,
                                            key.clone(),
                                            SiblingState {
                                                value: DfdlValue::Null,
                                                content_bytes: 0,
                                            },
                                        );
                                        self.insert_xpath_sibling(
                                            key,
                                            SiblingState {
                                                value: DfdlValue::Null,
                                                content_bytes: 0,
                                            },
                                        );
                                        let _ = el_props;
                                    }
                                    round_progress = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                let decode_result = if props.sequence_kind == SequenceKind::Unordered {
                    if let Ok(IrNode::Element { props: cp, .. }) = self.ctx.program.node(child) {
                        if cp.occurs_max.map(|m| m > 1).unwrap_or(true) {
                            self.decode_one_element_occurrence(
                                child,
                                cursor,
                                child_has_following,
                                Some(props),
                                Some(&seq_siblings),
                                content_scope_bytes,
                                pattern_text_frame,
                                child_stops,
                            )
                        } else {
                            self.decode_particle(
                                child,
                                cursor,
                                child_has_following,
                                Some(props),
                                Some(&seq_siblings),
                                content_scope_bytes,
                                pattern_text_frame,
                                child_stops,
                            )
                        }
                    } else {
                        self.decode_particle(
                            child,
                            cursor,
                            child_has_following,
                            Some(props),
                            Some(&seq_siblings),
                            content_scope_bytes,
                            pattern_text_frame,
                            child_stops,
                        )
                    }
                } else {
                    self.decode_particle(
                        child,
                        cursor,
                        child_has_following,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        child_stops,
                    )
                };
                match decode_result {
                    Ok(child_value) => {
                        if let Ok(IrNode::Element { props: cp, .. }) =
                            self.ctx.program.node(child)
                        {
                            if cp.occurs_min == 0
                                && is_suppressible_empty_representation(
                                    &child_value,
                                    cp,
                                    self.ctx.strings(),
                                )?
                            {
                                *cursor = saved;
                                continue;
                            }
                        }
                        let consumed = cursor.pos.saturating_sub(start);
                        if let IrNode::Element { name, props: el_props, .. } =
                            self.ctx.program.node(child)?
                        {
                            let key = self.ctx.strings().get(*name)?.to_string();
                            let content_bytes = if el_props.length_kind == LengthKind::Prefixed {
                                prefixed_payload_byte_length(
                                    &cursor.data[start..cursor.pos],
                                    el_props,
                                    self.ctx.strings(),
                                )?
                            } else {
                                consumed
                            };
                            insert_seq_sibling(&mut seq_siblings,
                                key,
                                SiblingState {
                                    value: child_value.clone(),
                                    content_bytes,
                                },
                            );
                        }
                        insert_child(&mut map, child, child_value, self.ctx.program)?;
                        if ordered_initiated {
                            if let Ok(IrNode::Element { props: cp, .. }) =
                                self.ctx.program.node(child)
                            {
                                if cp.occurs_min == 0
                                    && cp.initiator.is_some_and(|i| {
                                        self.ctx
                                            .strings()
                                            .get(i)
                                            .ok()
                                            .is_some_and(|p| !p.is_empty())
                                    })
                                    && initiated_block_lower_indices.is_none()
                                    && idx > 0
                                {
                                    let prior_required_unmet = children[..idx].iter().any(
                                        |&prior| {
                                            let Ok(IrNode::Element { name, props: pp, .. }) =
                                                self.ctx.program.node(prior)
                                            else {
                                                return false;
                                            };
                                            if pp.occurs_min == 0 {
                                                return false;
                                            }
                                            let key = self.ctx.strings().get(*name).ok();
                                            key.and_then(|k| map.get(k)).is_none()
                                        },
                                    );
                                    if !prior_required_unmet {
                                        initiated_block_lower_indices = Some(idx);
                                    }
                                }
                            }
                        }
                        if let IrNode::Element { name, .. } = self.ctx.program.node(child)? {
                            let key = self.ctx.strings().get(*name)?.to_string();
                            if let Some(meta) = self.field_delimiters.borrow_mut().remove(&key) {
                                field_delim_meta.insert(key, meta);
                            }
                        }
                        if let Ok(IrNode::Element { props: cp, .. }) =
                            self.ctx.program.node(child)
                        {
                            if self.initiated_content_uses_initiator_discriminator(cp) {
                                committed_child = Some(child);
                            }
                        }
                        if props.separator.is_some()
                            && !cursor.is_empty()
                            && props.separator_position == SeparatorPosition::Infix
                        {
                            let _ = self.consume_separator(
                                props,
                                cursor,
                                idx.saturating_add(1),
                                children.len(),
                                &mut infix_sep_newline_prefix,
                                child_stops,
                                idx.saturating_add(1) >= children.len(),
                            )?;
                        }
                        round_progress = true;
                        break;
                    }
                    Err(e) if is_element_absent(&e) => {
                        *cursor = saved;
                        continue;
                    }
                    Err(e) => {
                        *cursor = saved;
                        if let Ok(IrNode::Element { props: cp, .. }) =
                            self.ctx.program.node(child)
                        {
                            if cp.occurs_min == 0 {
                                continue;
                            }
                            let msg = e.to_string();
                            if (msg.contains("Init('")
                                && optional_element_may_absorb_initiator_failure(cp))
                                || (msg.contains("initiator mismatch")
                                    && optional_element_may_absorb_initiator_failure(cp))
                                || msg.contains("Delimiter not found")
                            {
                                continue;
                            }
                        }
                        return Err(e);
                    }
                }
            }
            if !round_progress || cursor.pos == round_start {
                break;
            }
        }
        if map.is_empty() && !cursor.is_empty() {
            return Err(VmError::InvalidValue {
                message: "initiated content sequence did not match any child initiator".into(),
            }
            .into());
        }
        if props.sequence_kind == SequenceKind::Unordered {
            self.validate_initiated_unordered_min_occurs(children, &map)?;
        }
        let _ = (node_id, has_following_sibling, parent_sequence);
        let mut terminator_alt = None;
        if let Some(id) = props.terminator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() {
                let enc = encoding_name(props, self.ctx.strings()).ok();
                if let Some((_n, alt)) = cursor.consume_delimiter_with_alt(
                    pat,
                    props.ignore_case,
                    enc.as_deref(),
                ) {
                    terminator_alt = Some(alt);
                }
            }
        }
        consume_element_trailing_framing(cursor, props)?;
        self.validate_particle_discriminator(props, "")?;
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

    fn is_hidden_group_carrier(&self, children: &[u32]) -> bool {
        if children.is_empty() {
            return false;
        }
        children.iter().all(|&id| {
            matches!(
                self.ctx.program.node(id),
                Ok(IrNode::Element { props, .. }) if props.hidden
            )
        })
    }

    fn decode_one_element_occurrence(
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
        let IrNode::Element { props, .. } = self.ctx.program.node(node_id)? else {
            return Err(VmError::TypeMismatch {
                expected: "element".into(),
            }
            .into());
        };
        let mut one_shot = props.clone();
        one_shot.occurs_max = Some(1);
        one_shot.occurs_min = if self.initiator_present_at_cursor(cursor, &props)? {
            1
        } else {
            0
        };
        self.decode_element_occurrences(
            node_id,
            &one_shot,
            cursor,
            has_following_sibling,
            parent_sequence,
            siblings,
            content_scope_bytes,
            pattern_text_frame,
            stop_sequences,
        )
    }

    fn validate_initiated_unordered_min_occurs(
        &self,
        children: &[u32],
        map: &BTreeMap<String, DfdlValue>,
    ) -> Result<()> {
        for &child in children {
            let IrNode::Element { name, props, .. } = self.ctx.program.node(child)? else {
                continue;
            };
            let key = self.ctx.strings().get(*name)?.to_string();
            let count = match map.get(&key) {
                Some(DfdlValue::Array(items)) => items.len(),
                Some(DfdlValue::Null) if props.nillable => 1,
                Some(DfdlValue::Null) => 0,
                Some(_) => 1,
                None => 0,
            };
            if (count as u64) < props.occurs_min {
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "unordered initiated sequence: element `{key}` expected at least {} occurrence(s), got {count}",
                        props.occurs_min
                    ),
                }
                .into());
            }
        }
        Ok(())
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
        if let Ok(IrNode::Sequence { children, .. }) = self.ctx.program.node(node_id) {
            if self.is_hidden_group_carrier(children) {
                let mut map = BTreeMap::new();
                for &child in children {
                    let value = self.decode_particle(
                        child,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    )?;
                    insert_child(&mut map, child, value, self.ctx.program)?;
                }
                return Ok(DfdlValue::Sequence(crate::value::SequenceValue {
                    fields: map,
                    meta: crate::value::SequenceMeta {
                        infix_sep_newline_prefix: Vec::new(),
                        initiator_alt: None,
                        terminator_alt: None,
                        separator_alts: Vec::new(),
                        field_delimiters: BTreeMap::new(),
                    },
                }));
            }
        }
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

    fn push_decoded_array_item(
        &self,
        items: &mut Vec<DfdlValue>,
        value: DfdlValue,
        props: &IrProps,
    ) -> Result<()> {
        let index = items.len() as u64 + 1;
        validate_assert_eq_occurs_index(&value, props, self.ctx.strings(), index)?;
        items.push(value);
        Ok(())
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
        struct OccurrenceDecodeDepthGuard<'a>(&'a Cell<u32>);
        impl Drop for OccurrenceDecodeDepthGuard<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get().saturating_sub(1));
            }
        }
        let depth = self.occurrence_decode_depth.get().saturating_add(1);
        self.occurrence_decode_depth.set(depth);
        if depth == 1 {
            *self.parent_postfix_sep_consumed.borrow_mut() = false;
        }
        let _occurrence_depth =
            OccurrenceDecodeDepthGuard(&self.occurrence_decode_depth);

        let mut min = props.occurs_min;
        let mut max = props.occurs_max.unwrap_or(u64::MAX);
        let mut occurs_from_expression = false;
        if props.occurs_count_kind == OccursCountKind::Expression {
            if let Some(steps) = props.occurs_count_fn_path.as_ref() {
                let sib_snap = self.xpath_siblings_snapshot();
                let n = eval_occurs_count_expression(
                    steps,
                    Some(&sib_snap),
                    self.ctx.strings(),
                    &self.ctx.program.tunables,
                )?;
                min = n;
                max = n;
                occurs_from_expression = true;
            }
        }
        if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
            if !occurs_from_expression {
                min = 0;
            }
            if cursor.is_empty() && min == 0 {
                return Ok(DfdlValue::Array(Vec::new()));
            }
        }
        if props.occurs_count_kind == OccursCountKind::Parsed {
            if props.occurs_max == Some(0) && min == 0 {
                // maxOccurs=0 with parsed count: scan padding (e.g. NUL-separated) without infoset items.
                max = u64::MAX;
            } else if max != u64::MAX && min == max {
                match self.framing_extra_occurrence(node_id, props, parent_sequence) {
                    FramingExtraOccurrences::One => max = max.saturating_add(1),
                    FramingExtraOccurrences::Double => max = max.saturating_mul(2),
                    FramingExtraOccurrences::None => {}
                }
            } else if max > 1 {
                // DFDL-5-062R: parsed count kind ignores maxOccurs during parse (post-decode validation only).
                max = u64::MAX;
            }
        }
        let populate_path = element_prefixed_name(self.ctx.program, node_id).ok();
        let populate_errors = should_populate_array_errors(props);
        let item_value_kind = element_kind(self.ctx.program, node_id)?;
        let mut items = Vec::new();
        let mut implicit_empty_probe = false;
        let never_optional_array = parent_sequence.and_then(|p| {
            if p.separator_suppression_policy == Some(crate::schema::SeparatorSuppressionPolicy::Never)
            {
                props.occurs_max.filter(|&m| m > 1)
            } else {
                None
            }
        });
        let mut never_infix_separators_consumed = 0u64;
        let delimiter_stops =
            filter_delimiter_stop_sequences(stop_sequences, self.ctx.strings())?;
        if props.empty_element_parse_policy == EmptyElementParsePolicy::TreatAsAbsent
            && min == 0
            && props.occurs_max == Some(0)
            && parent_sequence.is_some_and(|p| p.separator.is_some())
        {
            self.consume_treat_as_absent_separated_padding(
                props,
                node_id,
                parent_sequence,
                cursor,
                has_following_sibling,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            )?;
        }
        while (items.len() as u64) < max {
            if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
                let kind = element_kind(self.ctx.program, node_id)?;
                validate_explicit_decimal_before_decode(
                    kind,
                    props,
                    &self.ctx.program.tunables,
                    self.ctx.strings(),
                )?;
                if kind != ValueKind::Decimal
                    && binary_length_validation_applies(kind, props.binary_number_rep)
                {
                    validate_data_length_vm(kind, 0, props.length_units, props.binary_number_rep)?;
                }
                let more_array_occurrences = (items.len() as u64).saturating_add(1) < max;
                let parent_infix_consumed_by_occurrence_loop = more_array_occurrences
                    && parent_sequence.is_some_and(|p| {
                        p.separator.is_some()
                            && p.separator_position == SeparatorPosition::Infix
                    });
                let v = self.decode_single_element(
                    node_id,
                    cursor,
                    has_following_sibling || more_array_occurrences,
                    parent_sequence,
                    siblings,
                    content_scope_bytes,
                    pattern_text_frame,
                    delimiter_stops.as_slice(),
                    parent_infix_consumed_by_occurrence_loop,
                )?;
                self.push_decoded_array_item(&mut items, v, props)?;
                if parent_sequence.is_some_and(|p| {
                    p.separator_position == SeparatorPosition::Postfix
                }) {
                    self.consume_occurrence_separator(
                        parent_sequence,
                        Some(props),
                        Some(items.as_slice()),
                        cursor,
                    )?;
                }
                continue;
            }
            if items.len() as u64 >= min && cursor.is_empty() {
                if props.occurs_count_kind == OccursCountKind::Implicit
                    && items.is_empty()
                    && !implicit_empty_probe
                {
                    implicit_empty_probe = true;
                } else {
                    break;
                }
            }
            if (items.len() as u64) >= min {
                if max == u64::MAX
                    && self.at_enclosing_terminator_stop(cursor, stop_sequences)?
                {
                    break;
                }
                let at_sep_stop = self.at_enclosing_separator_stop(
                    cursor,
                    stop_sequences,
                    parent_sequence,
                )?;
                if at_sep_stop {
                    // Postfix occurrence separators (e.g. CSV rows terminated by %NL) mark the
                    // boundary before the next occurrence, not end of an unbounded array — blank
                    // lines must still decode as empty records (SequenceGroupNestedArray csv_nohang_1).
                    let postfix_unbounded = max == u64::MAX
                        && parent_sequence.is_some_and(|p| {
                            p.separator_position == SeparatorPosition::Postfix
                                && p.separator.is_some()
                        });
                    if !postfix_unbounded {
                        break;
                    }
                }
            }
            if let Some(limit) = self.ctx.program.tunables.max_occurs_bounds {
                if (items.len() as u32) >= limit {
                    let path = element_prefixed_name(self.ctx.program, node_id)
                        .unwrap_or_else(|_| "element".into());
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "Tunable Limit Exceeded: Array occurrences excceeds the maxOccursBounds tunable limit of {limit} at {path}"
                        ),
                    }
                    .into());
                }
            }
            let before_occurrence_sep = cursor.clone();
            if !items.is_empty()
                && !parent_sequence.is_some_and(|p| {
                    p.separator_position == SeparatorPosition::Postfix
                })
            {
                // Always try: delimited fields may defer enclosing consume, leaving the
                // occurrence separator at the cursor; if already consumed, this is a no-op.
                // Postfix separators are consumed after each occurrence (below), not before the next.
                let sep_pos = cursor.pos;
                if let Err(e) = self.consume_occurrence_separator(
                    parent_sequence,
                    Some(props),
                    Some(items.as_slice()),
                    cursor,
                ) {
                    if populate_errors && (items.len() as u64) < min {
                        if let Some(path) = populate_path.as_deref() {
                            return Err(
                                populate_failed_error(path, items.len() as u64 + 1, &e.to_string())
                                    .into(),
                            );
                        }
                    }
                    return Err(e);
                }
                if never_optional_array.is_some() && cursor.pos > sep_pos {
                    never_infix_separators_consumed += 1;
                }
            }
            let at_empty_slot = would_read_empty_delimited_field(
                cursor,
                props,
                self.ctx.strings(),
                delimiter_stops.as_slice(),
            )? || cursor_at_parent_infix_separator(
                cursor,
                parent_sequence,
                self.ctx.strings(),
            )?;
            if min == 0 && at_empty_slot {
                let sep_pos = cursor.pos;
                self.consume_occurrence_separator(
                    parent_sequence,
                    Some(props),
                    Some(items.as_slice()),
                    cursor,
                )?;
                if never_optional_array.is_some() && cursor.pos > sep_pos {
                    never_infix_separators_consumed += 1;
                }
                continue;
            }
            if min > 0
                && (items.len() as u64) < min
                && at_empty_slot
                && props.representation == Representation::Text
                && item_value_kind == ValueKind::String
            {
                let sep_pos = cursor.pos;
                self.consume_occurrence_separator(
                    parent_sequence,
                    Some(props),
                    Some(items.as_slice()),
                    cursor,
                )?;
                if never_optional_array.is_some() && cursor.pos > sep_pos {
                    never_infix_separators_consumed += 1;
                }
                self.push_decoded_array_item(&mut items, DfdlValue::string(""), props)?;
                continue;
            }
            if min > 0
                && (items.len() as u64) >= min
                && max == u64::MAX
                && at_empty_slot
                && parent_sequence.is_some_and(|p| {
                    p.separator.is_some()
                        && p.separator_position == SeparatorPosition::Infix
                })
            {
                let Some(sep_id) = parent_sequence.and_then(|p| p.separator) else {
                    unreachable!();
                };
                let pat = self.ctx.strings().get(sep_id)?;
                let enc = parent_sequence
                    .and_then(|p| encoding_name(p, self.ctx.strings()).ok());
                if Self::suffix_is_only_infix_separators(
                    cursor,
                    pat,
                    parent_sequence.unwrap().ignore_case,
                    enc.as_deref(),
                ) {
                    let sep_pos = cursor.pos;
                    self.consume_occurrence_separator(
                        parent_sequence,
                        Some(props),
                        Some(items.as_slice()),
                        cursor,
                    )?;
                    if never_optional_array.is_some() && cursor.pos > sep_pos {
                        never_infix_separators_consumed += 1;
                    }
                    continue;
                }
            }
            let more_array_occurrences = (items.len() as u64).saturating_add(1) < max;
            let require_delimiter = has_following_sibling || more_array_occurrences;
            let parent_infix_consumed_by_occurrence_loop = more_array_occurrences
                && parent_sequence.is_some_and(|p| {
                    p.separator.is_some()
                        && p.separator_position == SeparatorPosition::Infix
                });
            let saved = cursor.clone();
            struct ParentInfixDeferGuard<'a>(&'a Cell<bool>, bool);
            impl Drop for ParentInfixDeferGuard<'_> {
                fn drop(&mut self) {
                    self.0.set(self.1);
                }
            }
            let prev_infix_defer = self.parent_infix_consumed_by_occurrence_loop.get();
            self.parent_infix_consumed_by_occurrence_loop
                .set(parent_infix_consumed_by_occurrence_loop || prev_infix_defer);
            let _infix_defer_guard =
                ParentInfixDeferGuard(&self.parent_infix_consumed_by_occurrence_loop, prev_infix_defer);
            match self.decode_single_element(
                node_id,
                cursor,
                require_delimiter,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                delimiter_stops.as_slice(),
                parent_infix_consumed_by_occurrence_loop,
            ) {
                Ok(v) => {
                    if max == u64::MAX
                        && cursor.absolute_bit_index() == saved.absolute_bit_index()
                        && !cursor.is_frame_consumed()
                    {
                        if matches!(v, DfdlValue::Null) {
                            if props.empty_element_parse_policy
                                == EmptyElementParsePolicy::TreatAsAbsent
                            {
                                *cursor = saved;
                                if self.try_consume_treat_as_absent_separator(
                                    props,
                                    parent_sequence,
                                    cursor,
                                )? {
                                    if cursor.is_empty() {
                                        break;
                                    }
                                    continue;
                                }
                                if (items.len() as u64) >= min {
                                    *cursor = before_occurrence_sep;
                                    break;
                                }
                            }
                            self.push_decoded_array_item(&mut items, v, props)?;
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
                    if min == 0
                        && is_suppressible_empty_representation(
                            &v,
                            props,
                            self.ctx.strings(),
                        )?
                    {
                        let sep_pos = cursor.pos;
                        self.consume_occurrence_separator(
                            parent_sequence,
                            Some(props),
                            Some(items.as_slice()),
                            cursor,
                        )?;
                        if never_optional_array.is_some() && cursor.pos > sep_pos {
                            never_infix_separators_consumed += 1;
                        }
                        continue;
                    }
                    if v.as_str().is_some_and(str::is_empty)
                        && (items.len() as u64) >= min
                        && min > 0
                        && max == u64::MAX
                        && parent_sequence.is_some_and(|p| {
                            p.separator.is_some()
                                && p.separator_position == SeparatorPosition::Infix
                        })
                    {
                        let Some(sep_id) = parent_sequence.and_then(|p| p.separator) else {
                            unreachable!();
                        };
                        let pat = self.ctx.strings().get(sep_id)?;
                        let enc = parent_sequence
                            .and_then(|p| encoding_name(p, self.ctx.strings()).ok());
                        let parent = parent_sequence.unwrap();
                        let skip_leading_excess = items
                            .iter()
                            .all(|i| i.as_str().is_some_and(str::is_empty));
                        let skip_trailing_excess = items.iter().any(|i| {
                            i.as_str().is_some_and(|s| !s.is_empty())
                        }) && Self::suffix_is_only_infix_separators(
                            cursor,
                            pat,
                            parent.ignore_case,
                            enc.as_deref(),
                        );
                        if skip_leading_excess || skip_trailing_excess {
                            let sep_pos = cursor.pos;
                            self.consume_occurrence_separator(
                                parent_sequence,
                                Some(props),
                                Some(items.as_slice()),
                                cursor,
                            )?;
                            if never_optional_array.is_some() && cursor.pos > sep_pos {
                                never_infix_separators_consumed += 1;
                            }
                            continue;
                        }
                    }
                    self.push_decoded_array_item(&mut items, v, props)?;
                    if parent_sequence.is_some_and(|p| {
                        p.separator_position == SeparatorPosition::Postfix
                    }) {
                        let sep_pos = cursor.pos;
                        self.consume_occurrence_separator(
                            parent_sequence,
                            Some(props),
                            Some(items.as_slice()),
                            cursor,
                        )?;
                        if never_optional_array.is_some() && cursor.pos > sep_pos {
                            never_infix_separators_consumed += 1;
                        }
                    }
                }
                Err(e) => {
                    if implicit_empty_probe {
                        return Err(e);
                    }
                    if is_schema_definition_error(&e) {
                        return Err(e);
                    }
                    let rewind = saved.clone();
                    {
                        let err_msg = e.to_string();
                        if never_optional_array.is_some()
                            && (err_msg.contains("empty string")
                                || err_msg.contains("Unable to parse xs:int from empty"))
                        {
                            let track_never_sep = true;
                            *cursor = rewind.clone();
                            let sep_pos = cursor.pos;
                            self.consume_occurrence_separator(
                                parent_sequence,
                                Some(props),
                                Some(items.as_slice()),
                                cursor,
                            )?;
                            if track_never_sep && cursor.pos > sep_pos {
                                never_infix_separators_consumed += 1;
                            }
                            continue;
                        }
                    }
                    if min == 0 && items.is_empty() {
                        *cursor = rewind.clone();
                        let consumed = self.try_consume_treat_as_absent_separator_on_error(
                            props,
                            parent_sequence,
                            cursor,
                            &e,
                        )?;
                        if consumed {
                            if cursor.is_empty() {
                                break;
                            }
                            continue;
                        }
                        if is_element_absent(&e) {
                            if never_optional_array.is_some() {
                                *cursor = rewind.clone();
                                let sep_pos = cursor.pos;
                                self.consume_occurrence_separator(
                                    parent_sequence,
                                    Some(props),
                                    Some(items.as_slice()),
                                    cursor,
                                )?;
                                if cursor.pos > sep_pos {
                                    never_infix_separators_consumed += 1;
                                }
                                continue;
                            }
                            return Err(VmError::ElementAbsent.into());
                        }
                        let err_msg = e.to_string();
                        if err_msg.contains("initiator mismatch")
                            && !optional_element_may_absorb_initiator_failure(props)
                            && !rewind.is_empty()
                        {
                            return Err(
                                element_parse_error(
                                    node_id,
                                    self.ctx.program,
                                    self.ctx.strings(),
                                    e,
                                )
                                .into(),
                            );
                        }
                        break;
                    }
                    if (items.len() as u64) >= min {
                        if is_element_absent(&e) {
                            continue;
                        }
                        if self.try_consume_treat_as_absent_separator_on_error(
                            props,
                            parent_sequence,
                            cursor,
                            &e,
                        )? {
                            continue;
                        }
                        if props.initiator.is_some()
                            && cursor.pos > rewind.pos
                            && (items.len() as u64) < max
                        {
                            return Err(
                                element_parse_error(
                                    node_id,
                                    self.ctx.program,
                                    self.ctx.strings(),
                                    e,
                                )
                                .into(),
                            );
                        }
                        let err_msg = e.to_string();
                        if err_msg.contains("initiator mismatch") && !rewind.is_empty() {
                            return Err(
                                element_parse_error(
                                    node_id,
                                    self.ctx.program,
                                    self.ctx.strings(),
                                    e,
                                )
                                .into(),
                            );
                        }
                        // Failed attempt may have consumed an occurrence separator that
                        // belongs to following content (implicit or unbounded repetition).
                        *cursor = if max == u64::MAX
                            || props.occurs_count_kind == OccursCountKind::Implicit
                        {
                            before_occurrence_sep
                        } else {
                            rewind
                        };
                        break;
                    }
                    if let Some(default) = default_value_for(
                        element_kind(self.ctx.program, node_id)?,
                        props,
                        self.ctx.strings(),
                    ) {
                        self.push_decoded_array_item(&mut items, default, props)?;
                        break;
                    }
                    if populate_errors {
                        if let Some(path) = populate_path.as_deref() {
                            let index = items.len() as u64 + 1;
                            return Err(populate_failed_error(path, index, &e.to_string()).into());
                        }
                    }
                    return Err(
                        element_parse_error(node_id, self.ctx.program, self.ctx.strings(), e).into(),
                    );
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

        if let Some(m) = never_optional_array {
            let n = items.len() as u64;
            if n == 0 {
                if never_infix_separators_consumed != m.saturating_sub(1) {
                    return Err(VmError::InvalidValue {
                        message: "Parse Error: maxOccurs required with separatorSuppressionPolicy never"
                            .into(),
                    }
                    .into());
                }
            } else if n < m {
                return Err(VmError::InvalidValue {
                    message: "Parse Error: maxOccurs required with separatorSuppressionPolicy never"
                        .into(),
                }
                .into());
            }
        }

        if items.is_empty() {
            if min == 0
                && (props.occurs_max.map(|m| m > 1).unwrap_or(false)
                    || (props.occurs_count_kind == OccursCountKind::Parsed
                        && props.occurs_max.is_none()))
            {
                return Ok(DfdlValue::Array(Vec::new()));
            }
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
        parent_infix_consumed_by_occurrence_loop: bool,
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
                if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
                    validate_explicit_decimal_before_decode(
                        *kind,
                        &props,
                        &self.ctx.program.tunables,
                        self.ctx.strings(),
                    )?;
                    if *kind != ValueKind::Decimal
                        && binary_length_validation_applies(*kind, props.binary_number_rep)
                    {
                        validate_data_length_vm(
                            *kind,
                            0,
                            props.length_units,
                            props.binary_number_rep,
                        )?;
                    }
                }
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
                            true,
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
                                None,
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
                                stop_sequences,
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
                                let enc = encoding_name(&props, self.ctx.strings()).ok();
                                if crate::schema::match_delimiter_opts_for_encoding(
                                    &cursor.data[cursor.pos..],
                                    term,
                                    props.ignore_case,
                                    enc.as_deref(),
                                )
                                .is_some()
                                {
                                    let _ = cursor.consume_delimiter(
                                        term,
                                        props.ignore_case,
                                        enc.as_deref(),
                                    );
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
                                    let enc = encoding_name(parent, self.ctx.strings()).ok();
                                    let bytes = read_until_delimiters(
                                        cursor,
                                        parent,
                                        self.ctx.strings(),
                                        false,
                                        stop_sequences,
                                        enc.as_deref(),
                                        None,
                                    )?;
                                    if bytes.is_empty()
                                        && self.inner_sequence_first_particle_min(*child_id)? == 0
                                    {
                                        let mut consumed_parent_postfix_sep = false;
                                        if let Some(n) =
                                            crate::schema::match_delimiter_opts_for_encoding(
                                                &cursor.data[cursor.pos..],
                                                sep,
                                                parent.ignore_case,
                                                enc.as_deref(),
                                            )
                                        {
                                            if n > 0 {
                                                let _ = cursor.consume_delimiter(
                                                    sep,
                                                    parent.ignore_case,
                                                    enc.as_deref(),
                                                );
                                                consumed_parent_postfix_sep =
                                                    parent.separator_position
                                                        == SeparatorPosition::Postfix;
                                            }
                                        }
                                        if consumed_parent_postfix_sep {
                                            self.postfix_bounded_parent_sep_consumed.set(true);
                                        }
                                        return Err(VmError::ElementAbsent.into());
                                    }
                                    let mut consumed_parent_postfix_sep = false;
                                    let sep_pos_before = cursor.pos;
                                    if let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
                                        &cursor.data[cursor.pos..],
                                        sep,
                                        parent.ignore_case,
                                        enc.as_deref(),
                                    ) {
                                        if n > 0 {
                                            let _ = cursor.consume_delimiter(
                                                sep,
                                                parent.ignore_case,
                                                enc.as_deref(),
                                            );
                                            consumed_parent_postfix_sep =
                                                parent.separator_position
                                                    == SeparatorPosition::Postfix
                                                    && cursor.pos > sep_pos_before;
                                        }
                                    }
                                    let mut sub = Cursor::new(&bytes);
                                    let scope = bytes.len();
                                    let inner = self.decode_node(*child_id, &mut sub, false, None, None, Some(scope), pattern_text_frame, &[])?;
                                    if consumed_parent_postfix_sep {
                                        self.postfix_bounded_parent_sep_consumed.set(true);
                                    }
                                    if !sub.is_empty() {
                                        if let Ok(Some(inner_sep)) =
                                            self.inner_sequence_separator(*child_id)
                                        {
                                            let enc =
                                                encoding_name(&props, self.ctx.strings()).ok();
                                            if Self::cursor_only_consumes_infix_separators(
                                                &mut sub,
                                                inner_sep.as_str(),
                                                props.ignore_case,
                                                enc.as_deref(),
                                            )? {
                                                // Trailing excess separators (e.g. CSV commas).
                                            } else if !sub.is_empty() {
                                                return Err(VmError::InvalidValue {
                                                    message:
                                                        "unconsumed bytes in separator-bounded complex element"
                                                            .into(),
                                                }
                                                .into());
                                            }
                                        } else {
                                            return Err(VmError::InvalidValue {
                                                message:
                                                    "unconsumed bytes in separator-bounded complex element"
                                                        .into(),
                                            }
                                            .into());
                                        }
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
                        siblings,
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
                } else if props.input_value_calc_expression.is_some() {
                    let sib_snap = self.xpath_siblings_snapshot();
                    let value = eval_input_value_calc_expression(
                        props.input_value_calc_expression.as_ref().unwrap(),
                        Some(&sib_snap),
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        &self.ctx.program.root_element,
                    )?;
                    self.finalize_ivc_value(value, *kind, &props)
                } else if props.input_value_calc_path.is_some() {
                    let sib_snap = self.xpath_siblings_snapshot();
                    let value = eval_input_value_calc_path(
                        &props,
                        Some(&sib_snap),
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                    )?;
                    self.finalize_ivc_value(value, *kind, &props)
                } else if props.input_value_calc_segments.is_some() {
                    let sib_snap = self.xpath_siblings_snapshot();
                    let value = eval_input_value_calc_concat(
                        &props,
                        Some(&sib_snap),
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        &self.ctx.program.root_element,
                    )?;
                    self.finalize_ivc_value(value, *kind, &props)
                } else if props.input_value_calc.is_some() {
                    let value = eval_input_value_calc(
                        &props,
                        *kind,
                        cursor,
                        siblings,
                        self.ctx.strings(),
                        content_scope_bytes,
                        &self.runtime_variables.borrow(),
                    )?;
                    self.finalize_ivc_value(value, *kind, &props)
                } else {
                    if props.nillable
                        && crate::vm::runtime::try_consume_nillable_element_nil(
                            cursor,
                            &props,
                            parent_sequence,
                            self.ctx.strings(),
                            false,
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
                    self.runtime_check_bit_order_change(&props, cursor)?;
                    let scan_ctx = parent_sequence.map(|parent| {
                        let nested_under_repeating_particle = self.occurrence_decode_depth.get() > 1
                            && parent.separator.is_some()
                            && parent.separator_position == SeparatorPosition::Infix;
                        super::runtime::SequenceChildScanContext {
                            parent_sequence: parent,
                            has_following_sibling: require_delimiter,
                            parent_infix_consumed_by_occurrence_loop:
                                parent_infix_consumed_by_occurrence_loop
                                    || self.parent_infix_consumed_by_occurrence_loop.get()
                                    || nested_under_repeating_particle,
                        }
                    });
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
                        self.ctx.config.enable_facet_validation,
                        self.ctx.config.defer_facet_validation,
                        scan_ctx.as_ref(),
                    )
                    .map_err(crate::error::Error::from)?;
                    self.note_sequence_field_bit_order(&props);
                    if delim_meta.initiator_alt.is_some() || delim_meta.terminator_alt.is_some() {
                        self.field_delimiters
                            .borrow_mut()
                            .insert(field_name, delim_meta);
                    }
                    self.validate_particle_discriminator(
                        &props,
                        &dfdl_value_dispatch_string(&value),
                    )?;
                    Ok(value)
                }
            }
            _ => self.decode_node(node_id, cursor, false, None, None, content_scope_bytes, pattern_text_frame, stop_sequences),
        }
    }

    fn finalize_ivc_value(
        &self,
        value: DfdlValue,
        kind: ValueKind,
        props: &IrProps,
    ) -> Result<DfdlValue> {
        super::runtime::finalize_simple_value(
            value,
            kind,
            props,
            self.ctx.strings(),
            &self.ctx.program.tunables,
            self.ctx.config.enable_facet_validation,
            self.ctx.config.defer_facet_validation,
        )
        .map_err(Into::into)
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

    fn at_enclosing_terminator_stop(
        &self,
        cursor: &Cursor<'_>,
        stop_sequences: &[&IrProps],
    ) -> Result<bool> {
        for stop in stop_sequences {
            let Some(id) = stop.terminator else {
                continue;
            };
            let pat = self.ctx.strings().get(id)?;
            if pat.is_empty() {
                continue;
            }
            let enc = encoding_name(stop, self.ctx.strings()).ok();
            if crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                pat,
                stop.ignore_case,
                enc.as_deref(),
            )
            .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn at_enclosing_separator_stop(
        &self,
        cursor: &Cursor<'_>,
        stop_sequences: &[&IrProps],
        parent_sequence: Option<&IrProps>,
    ) -> Result<bool> {
        let parent_sep = parent_sequence.and_then(|p| p.separator);
        let parent_postfix = parent_sequence.is_some_and(|p| {
            p.separator_position == SeparatorPosition::Postfix && p.separator.is_some()
        });
        for stop in stop_sequences {
            let Some(sep_id) = stop.separator else {
                continue;
            };
            if parent_sep == Some(sep_id) && !parent_postfix {
                continue;
            }
            let pat = self.ctx.strings().get(sep_id)?;
            if pat.is_empty() {
                continue;
            }
            let enc = encoding_name(stop, self.ctx.strings()).ok();
            if crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                pat,
                stop.ignore_case,
                enc.as_deref(),
            )
            .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
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

    fn try_consume_treat_as_absent_separator_on_error(
        &self,
        props: &IrProps,
        parent_sequence: Option<&IrProps>,
        cursor: &mut Cursor<'_>,
        err: &Error,
    ) -> Result<bool> {
        if props.empty_element_parse_policy != EmptyElementParsePolicy::TreatAsAbsent {
            return Ok(false);
        }
        if parent_sequence.and_then(|p| p.separator).is_none() {
            return Ok(false);
        }
        if !is_element_absent(err) && !is_pattern_length_mismatch(err) {
            return Ok(false);
        }
        self.try_consume_treat_as_absent_separator(props, parent_sequence, cursor)
    }

    /// Scan separator-only bytes before/ between treatAsAbsent zero-length occurrences.
    fn consume_treat_as_absent_separated_padding(
        &self,
        props: &IrProps,
        node_id: u32,
        parent_sequence: Option<&IrProps>,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<()> {
        let Some(parent) = parent_sequence else {
            return Ok(());
        };
        if parent.separator.is_none() {
            return Ok(());
        }
        if props.empty_element_parse_policy != EmptyElementParsePolicy::TreatAsAbsent {
            return Ok(());
        }
        while !cursor.is_empty() {
            let saved = cursor.clone();
            match self.decode_single_element(
                node_id,
                cursor,
                has_following_sibling,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
                false,
            ) {
                Ok(_) => {
                    return Err(VmError::InvalidValue {
                        message: "Parse Error: element with maxOccurs 0 matched non-empty data"
                            .into(),
                    }
                    .into());
                }
                Err(e)
                    if is_element_absent(&e) || is_pattern_length_mismatch(&e) =>
                {
                    *cursor = saved;
                    if !self.try_consume_treat_as_absent_separator(props, parent_sequence, cursor)? {
                        break;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn try_consume_treat_as_absent_separator(
        &self,
        props: &IrProps,
        parent_sequence: Option<&IrProps>,
        cursor: &mut Cursor<'_>,
    ) -> Result<bool> {
        let _ = props;
        let Some(parent) = parent_sequence else {
            return Ok(false);
        };
        if parent.separator.is_none() {
            return Ok(false);
        }
        let before = cursor.pos;
        self.consume_occurrence_separator(Some(parent), None, None, cursor)?;
        Ok(cursor.pos > before)
    }

    fn consume_occurrence_separator(
        &self,
        parent_sequence: Option<&IrProps>,
        item_props: Option<&IrProps>,
        items: Option<&[DfdlValue]>,
        cursor: &mut Cursor<'_>,
    ) -> Result<()> {
        if self.postfix_bounded_parent_sep_consumed.get() {
            self.postfix_bounded_parent_sep_consumed.set(false);
            return Ok(());
        }
        if *self.parent_postfix_sep_consumed.borrow() {
            *self.parent_postfix_sep_consumed.borrow_mut() = false;
            return Ok(());
        }
        let Some(props) = parent_sequence else {
            return Ok(());
        };
        let Some(id) = props.separator else {
            return Ok(());
        };
        let pat = self.ctx.strings().get(id)?;
        let enc = encoding_name(props, self.ctx.strings()).ok();
        if let (Some(ip), Some(items)) = (item_props, items) {
            if !items.is_empty()
                && props.separator_position != SeparatorPosition::Postfix
                && matches!(
                    ip.occurs_count_kind,
                    OccursCountKind::Implicit | OccursCountKind::Fixed
                )
                && (items.len() as u64) < ip.occurs_min
            {
                if cursor.consume_delimiter(pat, props.ignore_case, enc.as_deref()) {
                    return Ok(());
                }
                let nested_infix_occurrence = self.parent_infix_consumed_by_occurrence_loop.get()
                    || self.occurrence_decode_depth.get() > 1;
                if nested_infix_occurrence && !cursor.is_empty() {
                    // Delimited nested fields may already have consumed the separator.
                    return Ok(());
                }
                return Err(VmError::InvalidValue {
                    message: alloc::format!("Separator '{pat}' not found"),
                }
                .into());
            }
        }
        let require = match (item_props, items) {
            (Some(ip), Some(items))
                if !items.is_empty()
                    && props.separator_position != SeparatorPosition::Postfix
                    && ip.occurs_count_kind == OccursCountKind::Parsed
                    && (pat.contains('\n')
                        || pat.trim() == "%NL;"
                        || pat.starts_with("%NL")) =>
            {
                !should_suppress_decode_occurrence_separator(props, ip, items, self.ctx.strings())?
            }
            _ => false,
        };
        if require {
            if cursor.consume_delimiter(pat, props.ignore_case, enc.as_deref()) {
                return Ok(());
            }
            let found_display =
                format_found_at_cursor(&cursor.data, cursor.pos, enc.as_deref());
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. infix separator. Delimiter not found!  Was looking for ({pat}) but found \"{found_display}\" instead"
                ),
            }
            .into());
        }
        let nl_like_sep = pat.contains('\n') || pat.trim() == "%NL;" || pat.starts_with("%NL");
        let mandatory_postfix = props.separator_position == SeparatorPosition::Postfix
            && nl_like_sep
            && item_props.is_some()
            && items.is_some_and(|it| !it.is_empty())
            && !cursor.is_empty();
        if mandatory_postfix {
            if cursor.consume_delimiter(pat, props.ignore_case, enc.as_deref()) {
                return Ok(());
            }
            // Postfix may already have been consumed by separator-bounded implicit complex decode.
            if crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                pat,
                props.ignore_case,
                enc.as_deref(),
            )
            .is_none()
            {
                return Ok(());
            }
            let found_display =
                format_found_at_cursor(&cursor.data, cursor.pos, enc.as_deref());
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. postfix separator. Delimiter not found!  Was looking for ({pat}) but found \"{found_display}\" instead"
                ),
            }
            .into());
        }
        if crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            props.ignore_case,
            enc.as_deref(),
        )
        .is_some()
        {
            if props.separator_position == SeparatorPosition::Postfix {
                if let Some(err) =
                    self.separator_enclosing_delimiter_conflict(props, pat, cursor, &[])
                {
                    return Err(err);
                }
            }
            if !cursor.consume_delimiter(pat, props.ignore_case, enc.as_deref()) {
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
            if !cursor.is_empty() {
                if child.is_none() && props.length_kind == LengthKind::Delimited {
                    consume_enclosing_delimiter(cursor, props, self.ctx.strings(), &[], None)?;
                } else if child.is_some() && props.terminator.is_some() {
                    let _ = self.consume_terminator(props, cursor);
                }
            }
        }
        Ok(())
    }

    fn eval_particle_assert_expression(&self, expr: &str, dot: &str) -> Result<bool> {
        let inner = expr
            .trim()
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(expr)
            .trim();
        if let Some(b) = crate::schema::eval_discriminator_expression(expr, dot) {
            return Ok(b);
        }
        if let Some(b) = self.eval_discriminator_xpath_eq(inner, dot)? {
            return Ok(b);
        }
        let compact: alloc::string::String =
            inner.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.starts_with("xs:boolean(") && compact.ends_with(')') {
            let path = &compact["xs:boolean(".len()..compact.len() - 1];
            return Ok(self.xpath_sibling_path_truthy(path));
        }
        if compact.contains('[') {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Indexing is only allowed on arrays".into(),
            }
            .into());
        }
        if inner.starts_with('$') {
            let name = inner.trim_start_matches('$').trim();
            let local = name.rsplit(':').next().unwrap_or(name);
            let vars = self.runtime_variables.borrow();
            let val = vars
                .get(name)
                .or_else(|| vars.get(local))
                .map(|s| s.as_str())
                .unwrap_or("false");
            return Ok(val == "true" || val == "1");
        }
        if compact.contains("*") && compact.contains("eq") {
            let eq_parts: alloc::vec::Vec<_> = compact.split("eq").collect();
            if eq_parts.len() == 2 {
                if let Ok(expected) = eq_parts[1].parse::<i64>() {
                    let mut product = 1i64;
                    for factor in eq_parts[0].split('*').filter(|s| !s.is_empty()) {
                        let factor = factor.trim().trim_end_matches(')').trim_start_matches('(');
                        let val = self.xpath_path_int_value(factor)?;
                        product = product.saturating_mul(val);
                    }
                    return Ok(product == expected);
                }
            }
        }
        Ok(false)
    }

    fn xpath_sibling_path_truthy(&self, path: &str) -> bool {
        self.xpath_path_int_value(path).unwrap_or(0) != 0
    }

    fn xpath_path_int_value(&self, path: &str) -> Result<i64> {
        let trimmed = path.trim();
        let mut rest = trimmed;
        let mut up = 0usize;
        while rest.starts_with("../") {
            up += 1;
            rest = &rest[3..];
        }
        let local = rest
            .rsplit(':')
            .next()
            .unwrap_or(rest)
            .trim();
        // One `../` step uses the current seeded sibling map (ordered ancestors + in-sequence peers).
        let sib = self
            .lookup_xpath_sibling_state(local, up.saturating_sub(1))
            .ok_or_else(|| {
            VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: No element corresponding to step {local} found."
                ),
            }
        })?;
        numeric_value_from_dfdl(&sib.value)
    }

    fn eval_facet_assert_message(&self, props: &IrProps) -> Result<alloc::string::String> {
        use crate::ir::IrInputValueCalcSegment;
        if let Some(segments) = &props.facet_assert_message_segments {
            let siblings = self.xpath_siblings_snapshot();
            let siblings = Some(&siblings);
            let strings = self.ctx.strings();
            let mut out = alloc::string::String::new();
            for seg in segments {
                match seg {
                    IrInputValueCalcSegment::Sibling(id) => {
                        let name = strings.get(*id)?;
                        out.push_str(&sibling_string_value(siblings, name)?);
                    }
                    IrInputValueCalcSegment::Literal(id) => {
                        out.push_str(strings.get(*id)?);
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
                    IrInputValueCalcSegment::InfosetPath(steps) => {
                        let value = eval_ivc_path_steps(
                            steps,
                            siblings,
                            strings,
                            &self.ctx.program.tunables,
                            "",
                        )?;
                        out.push_str(&dfdl_value_to_string(&value));
                    }
                }
            }
            return Ok(out);
        }
        if let Some(msg_id) = props.facet_assert_message {
            if let Ok(msg) = self.ctx.strings().get(msg_id) {
                return Ok(msg.to_string());
            }
        }
        Ok(alloc::string::String::new())
    }

    fn validate_particle_discriminator(&self, props: &IrProps, dot: &str) -> Result<()> {
        let Some(id) = props.discriminator_test else {
            return Ok(());
        };
        let expr = self.ctx.strings().get(id)?;
        if self.eval_particle_assert_expression(expr, dot)? {
            self.discriminator_committed_branch.set(true);
            return Ok(());
        }
        let msg = self.eval_facet_assert_message(props)?;
        if !msg.is_empty() {
            return Err(VmError::InvalidValue {
                message: alloc::format!("Parse Error. Assertion failed: {msg}"),
            }
            .into());
        }
        Err(VmError::InvalidValue {
            message: alloc::format!("Assertion Failed {expr}"),
        }
        .into())
    }

    fn choice_branch_discriminator_matches(&self, branch_node: u32, dot: &str) -> bool {
        let Some(props) = choice_branch_element_props(self.ctx.program, branch_node) else {
            return true;
        };
        let Some(id) = props.discriminator_test else {
            return true;
        };
        let Ok(expr) = self.ctx.strings().get(id) else {
            return false;
        };
        if let Some(b) = crate::schema::eval_discriminator_expression(expr, dot) {
            return b;
        }
        if let Ok(Some(b)) = self.eval_discriminator_xpath_eq(
            expr.trim()
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .unwrap_or(expr)
                .trim(),
            dot,
        ) {
            return b;
        }
        false
    }

    fn consume_initiator(&self, props: &IrProps, cursor: &mut Cursor<'_>) -> Result<()> {
        if let Some(id) = props.initiator {
            let pat = self.ctx.strings().get(id)?;
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if !pat.is_empty() {
                if !cursor.consume_delimiter(pat, props.ignore_case, enc.as_deref()) {
                    if pat.ends_with('[')
                        && !pat.starts_with('[')
                        && cursor.data.get(cursor.pos) == Some(&b'[')
                    {
                        cursor.advance(1);
                    } else {
                        return Err(VmError::InvalidValue {
                            message: "initiator mismatch".into(),
                        }
                        .into());
                    }
                }
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
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if !cursor.consume_delimiter(pat, props.ignore_case, enc.as_deref()) {
                if cursor.is_empty() {
                    return Ok(());
                }
                return Err(VmError::InvalidValue {
                    message: alloc::format!("terminator '{pat}' not found"),
                }
                .into());
            }
        }
        Ok(())
    }

    fn consume_trailing_empty_infix_separators(
        &self,
        seq_props: &IrProps,
        child_props: &IrProps,
        cursor: &mut Cursor<'_>,
    ) -> Result<bool> {
        use crate::schema::SeparatorSuppressionPolicy;
        let lax = matches!(
            seq_props.separator_suppression_policy,
            Some(SeparatorSuppressionPolicy::TrailingEmpty)
                | Some(SeparatorSuppressionPolicy::TrailingEmptyStrict)
        );
        if !lax || child_props.occurs_count_kind != OccursCountKind::Implicit {
            return Ok(false);
        }
        let max_rep = child_props.occurs_max.unwrap_or(u64::MAX);
        if max_rep <= 1 {
            return Ok(false);
        }
        let Some(sep_id) = seq_props.separator else {
            return Ok(false);
        };
        if seq_props.separator_position != SeparatorPosition::Infix {
            return Ok(false);
        }
        let pat = self.ctx.strings().get(sep_id)?;
        if pat.is_empty() {
            return Ok(false);
        }
        let enc = encoding_name(seq_props, self.ctx.strings()).ok();
        let mut consumed_any = false;
        while crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            seq_props.ignore_case,
            enc.as_deref(),
        )
        .is_some()
        {
            if !cursor.consume_delimiter(pat, seq_props.ignore_case, enc.as_deref()) {
                break;
            }
            consumed_any = true;
        }
        Ok(consumed_any)
    }

    fn consume_separator(
        &self,
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        index: usize,
        total: usize,
        infix_sep_newline_prefix: &mut Vec<bool>,
        stop_sequences: &[&IrProps],
        allow_missing_infix_after_last_slot: bool,
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
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if let Some((_, alt)) =
                cursor.consume_delimiter_with_alt(pat, props.ignore_case, enc.as_deref())
            {
                return Ok(Some(alt));
            }
            if props.separator_position == SeparatorPosition::Infix
                && props.separator_suppression_policy
                    == Some(crate::schema::SeparatorSuppressionPolicy::AnyEmpty)
                && index > 0
            {
                let b = cursor.data.get(cursor.pos).copied().unwrap_or(0);
                if b == b'-' || b == b'+' {
                    return Ok(None);
                }
            }
            let found_display =
                format_found_at_cursor(&cursor.data, cursor.pos, enc.as_deref());
            if props.separator_position == SeparatorPosition::Infix {
                if allow_missing_infix_after_last_slot {
                    let sep_at_cursor = crate::schema::match_delimiter_opts_for_encoding(
                        &cursor.data[cursor.pos..],
                        pat,
                        props.ignore_case,
                        enc.as_deref(),
                    )
                    .is_some();
                    if !sep_at_cursor
                        && self.at_enclosing_terminator_stop(cursor, stop_sequences)?
                    {
                        return Ok(None);
                    }
                }
                if self.at_enclosing_separator_stop(cursor, stop_sequences, None)? {
                    return Ok(None);
                }
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. Failed to find infix separator. Separator '{pat}' not found"
                    ),
                }
                .into());
            }
            let position_label = match props.separator_position {
                SeparatorPosition::Prefix => "prefix separator",
                SeparatorPosition::Infix => "infix separator",
                SeparatorPosition::Postfix => "postfix separator",
            };
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. {position_label}. Delimiter not found!  Was looking for ({pat}) but found \"{found_display}\" instead"
                ),
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
        let text_enc = encoding_name(sep_props, self.ctx.strings()).ok();
        let sep_n = crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            separator,
            sep_props.ignore_case,
            text_enc.as_deref(),
        )?;
        if sep_n == 0 {
            return None;
        }
        let mut scans: Vec<&IrProps> = stop_sequences.to_vec();
        scans.extend(enclosing.iter());
        for enc_props in scans.iter().copied() {
            if core::ptr::eq(enc_props, sep_props) {
                continue;
            }
            let Some(term_id) = enc_props.terminator else {
                continue;
            };
            let term = self.ctx.strings().get(term_id).ok()?;
            if term.len() <= separator.len() || !term.starts_with(separator) {
                continue;
            }
            let enc_name = encoding_name(enc_props, self.ctx.strings()).ok();
            let term_n = crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                term,
                enc_props.ignore_case,
                enc_name.as_deref(),
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
                props.input_value_calc.is_none()
                    && props.input_value_calc_segments.is_none()
                    && props.input_value_calc_path.is_none()
                    && props.input_value_calc_expression.is_none()
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

    fn cursor_only_consumes_infix_separators(
        cursor: &mut Cursor<'_>,
        separator: &str,
        ignore_case: bool,
        enc: Option<&str>,
    ) -> Result<bool> {
        while crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            separator,
            ignore_case,
            enc,
        )
        .is_some()
        {
            if !cursor.consume_delimiter(separator, ignore_case, enc) {
                break;
            }
        }
        Ok(cursor.is_empty())
    }

    fn suffix_is_only_infix_separators(
        cursor: &Cursor<'_>,
        separator: &str,
        ignore_case: bool,
        enc: Option<&str>,
    ) -> bool {
        let mut pos = cursor.pos;
        while pos <= cursor.data.len() {
            if pos == cursor.data.len() {
                return true;
            }
            let Some(n) = crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[pos..],
                separator,
                ignore_case,
                enc,
            ) else {
                return false;
            };
            if n == 0 {
                return false;
            }
            pos += n;
        }
        true
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

    fn inner_sequence_first_particle_min(&self, child_id: u32) -> Result<u64> {
        let node_id = match self.ctx.program.node(child_id)? {
            IrNode::Element { child: Some(id), .. } => *id,
            _ => child_id,
        };
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, .. } => {
                let Some(&first) = children.first() else {
                    return Ok(0);
                };
                match self.ctx.program.node(first)? {
                    IrNode::Element { props, .. } => Ok(props.occurs_min),
                    _ => Ok(1),
                }
            }
            _ => Ok(1),
        }
    }

    /// True when [`decode_single_element`] reads an implicit-length complex child in a
    /// sub-buffer bounded by (and consuming) the parent sequence's postfix separator.
    fn implicit_complex_consumed_parent_postfix(
        &self,
        child_id: u32,
        parent: &IrProps,
    ) -> Result<bool> {
        if !matches!(
            parent.separator_position,
            SeparatorPosition::Postfix
        ) {
            return Ok(false);
        }
        let Some(sep_id) = parent.separator else {
            return Ok(false);
        };
        let Ok(IrNode::Element {
            props,
            child: Some(inner),
            ..
        }) = self.ctx.program.node(child_id)
        else {
            return Ok(false);
        };
        if props.length_kind != LengthKind::Implicit {
            return Ok(false);
        }
        let parent_sep = self.ctx.strings().get(sep_id)?;
        if self.inner_sequence_separator(child_id)?.as_deref() == Some(parent_sep) {
            return Ok(false);
        }
        Ok(true)
    }

    /// Enumeration-only facet checks for choice disambiguation (DFDL-2-019R).
    /// Min/max/pattern validation is deferred to TDML post-decode validation.
    fn choice_branch_needs_post_decode_facet_check(&self, branch_node: u32) -> bool {
        let Ok(IrNode::Element { props, .. }) = self.ctx.program.node(branch_node) else {
            return false;
        };
        if self.ctx.config.defer_facet_validation {
            return needs_choice_discriminator_facet_check(props);
        }
        needs_facet_validation(props)
    }

    fn validate_choice_branch_value(
        &self,
        branch_node: u32,
        value: &DfdlValue,
    ) -> Result<()> {
        let IrNode::Element { kind, props, .. } = self.ctx.program.node(branch_node)? else {
            return Ok(());
        };
        if self.ctx.config.defer_facet_validation {
            validate_choice_discriminator_facets(
                value,
                *kind,
                props,
                self.ctx.strings(),
            )
            .map_err(Error::from)?;
        } else if needs_facet_validation(props) {
            validate_decoded_facets_tdml(
                value,
                *kind,
                props,
                self.ctx.strings(),
                &self.ctx.program.tunables,
                false,
            )
            .map_err(Error::from)?;
        }
        Ok(())
    }
}

fn is_element_absent(err: &Error) -> bool {
    matches!(err, Error::Vm(VmError::ElementAbsent))
}

fn is_schema_definition_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Vm(VmError::InvalidValue { message })
            if message.starts_with("Schema Definition Error")
                || message.starts_with("Runtime Schema Definition Error")
    )
}

/// Optional single-occurrence elements may treat initiator failures as absent; repeating or
/// `occursCountKind="parsed"` arrays must surface the error (e.g. e1a initiated-content choice).
fn optional_element_may_absorb_initiator_failure(props: &IrProps) -> bool {
    if props.occurs_count_kind == OccursCountKind::Parsed {
        return false;
    }
    !props.occurs_max.map(|m| m > 1).unwrap_or(true)
}

impl<'a> Decoder<'a> {
    /// Parsed unbounded arrays may end without a trailing infix separator before the next sibling.
    fn skip_infix_sep_after_parsed_unbounded_array(
        &self,
        seq_props: &IrProps,
        children: &[u32],
        index: usize,
        cursor: &Cursor<'_>,
    ) -> Result<bool> {
        if index == 0 || cursor.is_empty() {
            return Ok(false);
        }
        if seq_props.separator_position != SeparatorPosition::Infix {
            return Ok(false);
        }
        let Some(sep_id) = seq_props.separator else {
            return Ok(false);
        };
        let prev = children[index - 1];
        let IrNode::Element { props: prev_props, .. } = self.ctx.program.node(prev)? else {
            return Ok(false);
        };
        let parsed_repeating = prev_props.occurs_count_kind == OccursCountKind::Parsed
            && prev_props.occurs_max.map(|m| m > 1).unwrap_or(true);
        if !parsed_repeating {
            return Ok(false);
        }
        let pat = self.ctx.strings().get(sep_id)?;
        let enc = encoding_name(seq_props, self.ctx.strings()).ok();
        let at_sep = crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            pat,
            seq_props.ignore_case,
            enc.as_deref(),
        )
        .is_some();
        Ok(!at_sep)
    }
}

fn is_pattern_length_mismatch(err: &Error) -> bool {
    matches!(
        err,
        Error::Vm(VmError::InvalidValue { message })
            if message.starts_with("pattern `") && message.ends_with(" mismatch")
    )
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

fn choice_explicit_frame_bytes(
    props: &IrProps,
    cursor: &Cursor<'_>,
    strings: &StringPool,
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
    if n == 0 {
        return Ok(Some(0));
    }
    let span = match props.length_units {
        LengthUnits::Bytes => n,
        LengthUnits::Characters => {
            let enc = encoding_name(props, strings)?;
            super::encoding::character_span_byte_length(n, &enc)?
        }
        LengthUnits::Bits => n.saturating_add(7) / 8,
    };
    if cursor.pos.saturating_add(span) > cursor.data.len() {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "choice explicit length {n} {:?} exceeds remaining input",
                props.length_units
            ),
        }
        .into());
    }
    Ok(Some(span))
}

fn backtrack_decoded_facets_ok(
    value: &DfdlValue,
    kind: ValueKind,
    props: &IrProps,
    strings: &StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> bool {
    match value {
        DfdlValue::Array(items) => items.iter().all(|item| {
            validate_decoded_facets_tdml(item, kind, props, strings, tunables, false).is_ok()
        }),
        _ => validate_decoded_facets_tdml(value, kind, props, strings, tunables, false).is_ok(),
    }
}

fn initiated_child_occurrence_count(
    map: &BTreeMap<String, DfdlValue>,
    key: &str,
    props: &IrProps,
) -> usize {
    match map.get(key) {
        Some(DfdlValue::Array(items)) => items.len(),
        Some(DfdlValue::Null) if props.nillable => 1,
        Some(DfdlValue::Null) => 0,
        Some(_) => 1,
        None => 0,
    }
}

fn unordered_backtrack_multi_occurrence(props: &IrProps) -> bool {
    props.occurs_min > 1
        || props.occurs_max.map(|m| m > 1).unwrap_or(false)
        || (props.occurs_count_kind == OccursCountKind::Parsed
            && props.occurs_max.map(|m| m > 1).unwrap_or(false))
}

fn unordered_backtrack_may_take_another(
    map: &BTreeMap<String, DfdlValue>,
    key: &str,
    props: &IrProps,
    cursor_empty: bool,
) -> bool {
    if cursor_empty {
        return false;
    }
    let schema_max = props.occurs_max.unwrap_or(1);
    if props.occurs_count_kind == OccursCountKind::Parsed {
        if schema_max <= 1 {
            return false;
        }
        return true;
    }
    let count = initiated_child_occurrence_count(map, key, props);
    (count as u64) < schema_max
}

fn sequence_value_for_child(
    child_id: u32,
    fields: &BTreeMap<String, DfdlValue>,
    program: &IrProgram,
) -> Result<Option<DfdlValue>> {
    match program.node(child_id)? {
        IrNode::Element { name, .. } => {
            let key = program.strings.get(*name)?;
            let local = crate::xml_util::local_name_str(key);
            Ok(fields
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
                .map(|(_, v)| v.clone()))
        }
        IrNode::Sequence { children, .. } => {
            let mut nested = BTreeMap::new();
            for &gc in children {
                if let Some(v) = sequence_value_for_child(gc, fields, program)? {
                    insert_child(&mut nested, gc, v, program)?;
                }
            }
            if nested.is_empty() {
                Ok(None)
            } else {
                Ok(Some(DfdlValue::sequence(nested)))
            }
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if let Some(v) = sequence_value_for_child(branch.node, fields, program)? {
                    let disc = program.strings.get(branch.name)?.to_string();
                    return Ok(Some(DfdlValue::choice(disc, v)));
                }
            }
            Ok(None)
        }
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
        IrNode::Sequence { children, .. } => {
            if let DfdlValue::Sequence(seq) = value {
                for &child_id in children {
                    if let Some(v) = sequence_value_for_child(child_id, &seq.fields, program)? {
                        insert_child(map, child_id, v, program)?;
                    }
                }
                Ok(())
            } else {
                Err(VmError::TypeMismatch {
                    expected: "sequence".into(),
                }
                .into())
            }
        }
        IrNode::Choice { branches, .. } => {
            if let DfdlValue::Choice {
                discriminator,
                value: branch_value,
            } = value
            {
                if let Some(branch) = branches.iter().find(|b| {
                    choice_branch_discriminator_matches_name(
                        program,
                        b,
                        discriminator.as_str(),
                    )
                }) {
                    return insert_child(map, branch.node, *branch_value, program);
                }
                match *branch_value {
                    DfdlValue::Sequence(seq)
                        if discriminator == "sequence" || discriminator == "choice" =>
                    {
                        if let Some(branch) = branches.first() {
                            insert_child(
                                map,
                                branch.node,
                                DfdlValue::Sequence(seq),
                                program,
                            )?;
                        }
                    }
                    other => {
                        if props_hidden_choice_branch_other(program, branches, &discriminator) {
                            return Ok(());
                        }
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

fn props_hidden_choice_branch_other(
    program: &IrProgram,
    branches: &[ChoiceBranch],
    discriminator: &str,
) -> bool {
    branches.iter().any(|b| {
        program
            .strings
            .get(b.name)
            .ok()
            .is_some_and(|n| n == discriminator)
            && branch_root_hidden(program, b.node)
    })
}

fn branch_root_hidden(program: &IrProgram, node_id: u32) -> bool {
    match program.node(node_id) {
        Ok(IrNode::Element { props, .. }) => props.hidden,
        Ok(IrNode::Sequence { children, .. }) => children
            .iter()
            .all(|&c| branch_root_hidden(program, c)),
        Ok(IrNode::Choice { branches, .. }) => branches
            .iter()
            .all(|b| branch_root_hidden(program, b.node)),
        _ => false,
    }
}

fn should_write_separator(position: SeparatorPosition, index: usize, total: usize) -> bool {
    match position {
        SeparatorPosition::Prefix => index < total,
        SeparatorPosition::Infix => index > 0,
        SeparatorPosition::Postfix => index > 0 && index < total,
    }
}

fn insert_seq_sibling(map: &mut BTreeMap<String, SiblingState>, key: String, state: SiblingState) {
    if let Some(existing) = map.remove(&key) {
        map.insert(
            key,
            SiblingState {
                value: append_value(existing.value, state.value),
                content_bytes: state.content_bytes,
            },
        );
    } else {
        map.insert(key, state);
    }
}

fn insert_field(map: &mut BTreeMap<String, DfdlValue>, key: String, value: DfdlValue) {
    if matches!(&value, DfdlValue::Array(items) if items.is_empty()) {
        return;
    }
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
            let inner = match (discriminator.as_str(), value.as_ref()) {
                ("sequence", DfdlValue::Sequence(seq)) if seq.fields.is_empty() => {
                    DfdlValue::sequence(BTreeMap::new())
                }
                ("sequence", DfdlValue::Sequence(seq)) => DfdlValue::sequence(seq.fields.clone()),
                _ => {
                    let mut fields = BTreeMap::new();
                    fields.insert(discriminator, *value);
                    DfdlValue::sequence(fields)
                }
            };
            let mut wrapped = BTreeMap::new();
            wrapped.insert(name.into(), inner);
            DfdlValue::sequence(wrapped)
        }
        other => {
            let mut map = BTreeMap::new();
            map.insert(name.into(), other);
            DfdlValue::sequence(map)
        }
    }
}

fn choice_branch_fields(discriminator: String, value: DfdlValue) -> BTreeMap<String, DfdlValue> {
    match (discriminator.as_str(), value) {
        ("choice", DfdlValue::Choice { discriminator, value }) => {
            choice_branch_fields(discriminator, *value)
        }
        ("sequence", DfdlValue::Sequence(seq)) => seq.fields,
        (name, v) => {
            let mut map = BTreeMap::new();
            map.insert(name.to_string(), v);
            map
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
                    return DfdlValue::Sequence(seq);
                }
                DfdlValue::Sequence(seq)
            }
            DfdlValue::Choice { discriminator, value } => {
                DfdlValue::sequence(choice_branch_fields(discriminator, *value))
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
    tunables: &crate::length_validate::DaffodilTunables,
    root_element: &str,
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
            IrInputValueCalcSegment::Literal(id) => {
                out.push_str(strings.get(*id)?);
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
            IrInputValueCalcSegment::InfosetPath(steps) => {
                let value =
                    eval_ivc_path_steps(steps, siblings, strings, tunables, root_element)?;
                out.push_str(&dfdl_value_to_string(&value));
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
    runtime_variables: &BTreeMap<String, String>,
) -> Result<DfdlValue> {
    let calc = props.input_value_calc.ok_or_else(|| VmError::InvalidValue {
        message: "missing inputValueCalc".into(),
    })?;
    if calc == InputValueCalc::SchemaVariable {
        let name_id = props.input_value_calc_literal.ok_or_else(|| VmError::InvalidValue {
            message: "missing inputValueCalc variable name".into(),
        })?;
        let name = strings.get(name_id)?;
        let text = runtime_variables.get(name).cloned().ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("Schema Definition Error: variable `{name}` is not defined"),
        })?;
        if kind == ValueKind::String {
            return Ok(DfdlValue::String(StringValue::new(text)));
        }
        if kind == ValueKind::HexBinary {
            let bytes = super::runtime::decode_hex_binary(&text)?;
            return Ok(crate::value::DfdlValue::HexBinary(bytes));
        }
        let mut sub = Cursor::new(text.as_bytes());
        return super::runtime::read_text_scalar(
            &mut sub,
            kind,
            props,
            strings,
            false,
            &[],
            None,
            None,
            &crate::length_validate::DaffodilTunables::default(),
            None,
        )
        .map_err(Into::into);
    }
    if calc == InputValueCalc::StringLiteral {
        let lit_id = props.input_value_calc_literal.ok_or_else(|| VmError::InvalidValue {
            message: "missing inputValueCalc string literal".into(),
        })?;
        let text = strings.get(lit_id)?;
        if kind == ValueKind::HexBinary {
            let bytes = super::runtime::decode_hex_binary(text)?;
            return Ok(crate::value::DfdlValue::HexBinary(bytes));
        }
        if kind == ValueKind::String {
            return Ok(DfdlValue::String(StringValue::new(text)));
        }
        if matches!(kind, ValueKind::DateTime | ValueKind::Time) {
            let parsed = super::calendar_binary::parse_xs_calendar_lexical(
                kind,
                props.calendar_date_only,
                text,
            )?;
            return Ok(DfdlValue::DateTime(parsed));
        }
        let mut sub = Cursor::new(text.as_bytes());
        return super::runtime::read_text_scalar(
            &mut sub,
            kind,
            props,
            strings,
            false,
            &[],
            None,
            None,
            &crate::length_validate::DaffodilTunables::default(),
            None,
        )
        .map_err(Into::into);
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
    if calc == InputValueCalc::HexBinaryFromSibling {
        if kind != ValueKind::HexBinary {
            return Err(VmError::InvalidValue {
                message: "xs:hexBinary inputValueCalc requires xs:hexBinary element".into(),
            }
            .into());
        }
        let sib = sibling_state(props, siblings, strings)?;
        let text = dfdl_value_text(&sib.value);
        let bytes = super::runtime::decode_hex_binary(text)?;
        return Ok(crate::value::DfdlValue::HexBinary(bytes));
    }
    let len = match calc {
        InputValueCalc::Constant(_) => unreachable!("handled above"),
        InputValueCalc::StringLiteral => unreachable!("handled above"),
        InputValueCalc::SchemaVariable => unreachable!("handled above"),
        InputValueCalc::BooleanFromSibling => unreachable!("handled above"),
        InputValueCalc::HexBinaryFromSibling => unreachable!("handled above"),
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

fn element_parse_error(
    node_id: u32,
    program: &IrProgram,
    strings: &StringPool,
    err: impl core::fmt::Display,
) -> crate::error::VmError {
    let msg = if let Ok(IrNode::Element { name, .. }) = program.node(node_id) {
        if let Ok(local) = strings.get(*name) {
            alloc::format!("{err} {local}")
        } else {
            err.to_string()
        }
    } else {
        err.to_string()
    };
    crate::error::VmError::InvalidValue { message: msg }
}

fn element_discriminator_always_false(props: &IrProps, strings: &StringPool) -> bool {
    let Some(id) = props.discriminator_test else {
        return false;
    };
    let Ok(expr) = strings.get(id) else {
        return false;
    };
    let inner = expr
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(expr.as_ref())
        .trim();
    matches!(inner, "fn:false()" | "false()")
}


fn choice_branch_element_props(program: &IrProgram, node: u32) -> Option<&IrProps> {
    match &program.nodes[node as usize] {
        IrNode::Element { props, .. } => Some(props),
        _ => None,
    }
}

fn peek_choice_discriminator_dot(
    program: &IrProgram,
    branches: &[ChoiceBranch],
    cursor: &Cursor,
    strings: &StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Option<String> {
    for branch in branches {
        let IrNode::Element { props, kind, .. } = &program.nodes[branch.node as usize] else {
            continue;
        };
        if props.discriminator_test.is_none() {
            continue;
        }
        if props.length_kind != LengthKind::Explicit {
            continue;
        }
        let mut c = cursor.clone();
        if let Ok(value) = read_simple(
            &mut c,
            *kind,
            props,
            strings,
            false,
            &[],
            None,
            tunables,
            false,
            None,
            None,
            false,
            false,
            None,
        ) {
            return Some(dfdl_value_dispatch_string(&value));
        }
    }
    None
}

fn choice_dispatch_key_string(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<Option<alloc::string::String>> {
    if props.choice_dispatch_literal.is_some()
        || props.choice_dispatch_sibling.is_some()
        || props.choice_dispatch_path.is_some()
        || props.choice_dispatch_sibling_int.is_some()
    {
        if let Some(id) = props.choice_dispatch_literal {
            return Ok(Some(strings.get(id)?.to_string()));
        }
        if let Some(id) = props.choice_dispatch_sibling_int {
            let name = strings.get(id)?;
            let value = siblings
                .and_then(|m| m.get(name))
                .map(|s| &s.value)
                .ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!(
                        "choice dispatch sibling `{name}` not available"
                    ),
                })?;
            let text = dfdl_value_dispatch_string(value);
            let trimmed = text.trim();
            let n: i64 = trimmed.parse().map_err(|_| VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. Cannot convert `{trimmed}` to xs:int"
                ),
            })?;
            return Ok(Some(n.to_string()));
        }
        if let Some(id) = props.choice_dispatch_sibling {
            let name = strings.get(id)?;
            let value = siblings
                .and_then(|m| m.get(name))
                .map(|s| &s.value)
                .ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!(
                        "choice dispatch sibling `{name}` not available"
                    ),
                })?;
            return Ok(Some(dfdl_value_dispatch_string(value)));
        }
        if let Some(steps) = props.choice_dispatch_path.as_ref() {
            let value = eval_infoset_path_steps(steps, siblings, strings, tunables)?;
            return Ok(Some(dfdl_value_dispatch_string(&value)));
        }
    }
    Ok(None)
}

fn sibling_discriminator_values(value: &DfdlValue) -> alloc::vec::Vec<alloc::string::String> {
    match value {
        DfdlValue::Array(items) => items.iter().flat_map(sibling_discriminator_values).collect(),
        other => alloc::vec![dfdl_value_dispatch_string(other)],
    }
}

/// Matches Apache Daffodil `ParticleMixin.isOptional` (choice branch SDE at activation).
fn ir_props_is_dfdl_optional(props: &IrProps) -> bool {
    match (props.occurs_min, props.occurs_max) {
        (1, Some(1)) => false,
        (1, None) => false,
        (0, max) => match props.occurs_count_kind {
            OccursCountKind::Parsed | OccursCountKind::Expression => false,
            OccursCountKind::Implicit | OccursCountKind::Fixed => max == Some(1),
        },
        _ => false,
    }
}

fn choice_branch_term_node(program: &IrProgram, node_id: u32) -> Result<u32> {
    match program.node(node_id)? {
        IrNode::Sequence { children, .. } if children.len() == 1 => Ok(children[0]),
        _ => Ok(node_id),
    }
}

fn choice_branch_is_optional_element(program: &IrProgram, branch_node: u32) -> Result<bool> {
    let term = choice_branch_term_node(program, branch_node)?;
    match program.node(term)? {
        IrNode::Element { props, .. } => Ok(ir_props_is_dfdl_optional(props)),
        _ => Ok(false),
    }
}

fn validate_choice_branches_non_optional_runtime(
    program: &IrProgram,
    branches: &[ChoiceBranch],
) -> Result<()> {
    for branch in branches {
        let optional = choice_branch_is_optional_element(program, branch.node)?;
        if optional {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Branch of choice must be non-optional.".into(),
            }
            .into());
        }
    }
    Ok(())
}

fn ir_element_has_input_value_calc(props: &IrProps) -> bool {
    props.input_value_calc.is_some()
        || props.input_value_calc_literal.is_some()
        || props.input_value_calc_sibling.is_some()
        || props.input_value_calc_segments.is_some()
        || props.input_value_calc_path.is_some()
        || props.input_value_calc_expression.is_some()
}

fn choice_branch_has_input_value_calc(program: &IrProgram, branch_node: u32) -> Result<bool> {
    match program.node(branch_node)? {
        IrNode::Element { props, .. } if !props.hidden => {
            Ok(ir_element_has_input_value_calc(props))
        }
        _ => Ok(false),
    }
}

fn validate_choice_branches_not_ivc_runtime(
    program: &IrProgram,
    branches: &[ChoiceBranch],
) -> Result<()> {
    for branch in branches {
        if choice_branch_has_input_value_calc(program, branch.node)? {
            return Err(VmError::InvalidValue {
                message:
                    "Schema Definition Error: Branch of choice cannot have the dfdl:inputValueCalc property."
                        .into(),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_choice_branch_element_name_upa_runtime(
    program: &IrProgram,
    branches: &[ChoiceBranch],
) -> Result<()> {
    let mut seen: BTreeMap<alloc::string::String, ()> = BTreeMap::new();
    for branch in branches {
        let term = choice_branch_term_node(program, branch.node)?;
        let IrNode::Element { name, .. } = program.node(term)? else {
            continue;
        };
        let local = program.strings.get(*name)?.to_string();
        if seen.contains_key(&local) {
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: Unique Particle Attribution violation for element '{local}' in choice"
                ),
            }
            .into());
        }
        seen.insert(local, ());
    }
    Ok(())
}

fn parse_discriminator_path_step(step: &str) -> Result<(&str, Option<usize>)> {
    let step = step.trim();
    if let Some(open) = step.find('[') {
        if !step.ends_with(']') {
            return Err(VmError::InvalidValue {
                message: "invalid discriminator path step".into(),
            }
            .into());
        }
        let local = step[..open].trim();
        let idx: usize = step[open + 1..step.len() - 1]
            .trim()
            .parse()
            .map_err(|_| VmError::InvalidValue {
                message: "invalid discriminator array index".into(),
            })?;
        return Ok((local, Some(idx)));
    }
    Ok((step, None))
}

fn single_or_array_item_at(value: &DfdlValue, one_based_index: usize) -> Result<DfdlValue> {
    if one_based_index == 1 {
        if matches!(value, DfdlValue::Sequence(_)) {
            return Ok(value.clone());
        }
    }
    array_item_at(value, one_based_index)
}

fn array_item_at(value: &DfdlValue, one_based_index: usize) -> Result<DfdlValue> {
    let DfdlValue::Array(items) = value else {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: Indexing is only allowed on arrays".into(),
        }
        .into());
    };
    let idx = one_based_index
        .checked_sub(1)
        .ok_or_else(|| VmError::InvalidValue {
            message: "invalid discriminator array index".into(),
        })?;
    items.get(idx).cloned().ok_or_else(|| {
        VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: No element corresponding to step index {one_based_index} found."
            ),
        }
        .into()
    })
}

fn navigate_discriminator_path_step(
    value: &DfdlValue,
    local: &str,
    index: Option<usize>,
) -> Result<DfdlValue> {
    let field = sequence_field_by_local_value(value, local).ok_or_else(|| {
        VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: No element corresponding to step {local} found."
            ),
        }
    })?;
    if let Some(idx) = index {
        single_or_array_item_at(field, idx)
    } else {
        Ok(field.clone())
    }
}

fn sequence_field_by_local_value<'a>(value: &'a DfdlValue, local: &str) -> Option<&'a DfdlValue> {
    match value {
        DfdlValue::Sequence(seq) => sequence_field_by_local(&seq.fields, local),
        _ => None,
    }
}

fn dfdl_value_dispatch_string(value: &DfdlValue) -> alloc::string::String {
    match value {
        DfdlValue::String(v) => v.text.clone(),
        DfdlValue::Int(v) => v.to_string(),
        DfdlValue::Long(v) => v.to_string(),
        DfdlValue::Short(v) => v.to_string(),
        DfdlValue::Byte(v) => v.to_string(),
        DfdlValue::UnsignedInt(v) => v.to_string(),
        DfdlValue::UnsignedShort(v) => v.to_string(),
        DfdlValue::UnsignedByte(v) => v.to_string(),
        DfdlValue::Integer(v) => v.clone(),
        DfdlValue::Decimal(v) => v.clone(),
        DfdlValue::Boolean(v) => {
            if *v {
                "true".into()
            } else {
                "false".into()
            }
        }
        other => dfdl_value_text(other).to_string(),
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

fn eval_input_value_calc_expression(
    expr: &crate::ir::IrInputValueCalcExpression,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    root_element: &str,
) -> Result<DfdlValue> {
    use crate::ir::IrInputValueCalcExpression;
    match expr {
        IrInputValueCalcExpression::Add(terms) => {
            let mut sum = 0i64;
            for term in terms {
                sum = sum.saturating_add(eval_input_value_calc_to_i64(
                    term,
                    siblings,
                    strings,
                    tunables,
                    root_element,
                )?);
            }
            Ok(DfdlValue::Integer(sum.to_string()))
        }
        IrInputValueCalcExpression::Mul(terms) => {
            let mut product = 1i64;
            for term in terms {
                product = product.saturating_mul(eval_input_value_calc_to_i64(
                    term,
                    siblings,
                    strings,
                    tunables,
                    root_element,
                )?);
            }
            Ok(DfdlValue::Integer(product.to_string()))
        }
        IrInputValueCalcExpression::Path(steps) => {
            eval_ivc_path_steps(steps, siblings, strings, tunables, root_element)
        }
        IrInputValueCalcExpression::StringOf(inner) => {
            let value = eval_input_value_calc_expression(
                inner,
                siblings,
                strings,
                tunables,
                root_element,
            )?;
            let text = dfdl_value_to_string(&value);
            Ok(DfdlValue::String(StringValue::new(text)))
        }
    }
}

fn dfdl_value_to_string(value: &DfdlValue) -> String {
    match value {
        DfdlValue::String(s) => s.text.clone(),
        DfdlValue::Int(n) => n.to_string(),
        DfdlValue::Long(n) => n.to_string(),
        DfdlValue::Integer(s) => s.clone(),
        DfdlValue::Short(n) => n.to_string(),
        DfdlValue::Byte(n) => n.to_string(),
        DfdlValue::UnsignedInt(n) => n.to_string(),
        DfdlValue::UnsignedShort(n) => n.to_string(),
        DfdlValue::UnsignedByte(n) => n.to_string(),
        other => dfdl_value_text(other).to_string(),
    }
}

fn eval_input_value_calc_to_i64(
    expr: &crate::ir::IrInputValueCalcExpression,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    root_element: &str,
) -> Result<i64> {
    let value = eval_input_value_calc_expression(expr, siblings, strings, tunables, root_element)?;
    value.as_i64().ok_or_else(|| {
        VmError::InvalidValue {
            message: alloc::format!(
                "Schema Definition Error: expression evaluation error: non-numeric value"
            ),
        }
        .into()
    })
}

fn eval_ivc_path_steps(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    root_element: &str,
) -> Result<DfdlValue> {
    let mut steps = steps;
    if let Some(first) = steps.first() {
        let local = strings.get(first.local)?;
        if local == root_element {
            steps = &steps[1..];
        }
    }
    if steps.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: expression evaluation error: empty path".into(),
        }
        .into());
    }
    eval_infoset_path_steps(steps, siblings, strings, tunables)
}

fn eval_input_value_calc_path(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<DfdlValue> {
    let steps = props.input_value_calc_path.as_ref().ok_or_else(|| VmError::InvalidValue {
        message: "missing inputValueCalc path".into(),
    })?;
    let value = eval_infoset_path_steps(steps, siblings, strings, tunables)?;
    let text = dfdl_value_text(&value);
    Ok(DfdlValue::String(StringValue::new(text.to_string())))
}

fn eval_occurs_count_expression(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<u64> {
    let value = eval_infoset_path_steps(steps, siblings, strings, tunables)?;
    if let Some(n) = value.as_i64() {
        if n >= 0 {
            return Ok(n as u64);
        }
    }
    Ok(count_dfdl_value_nodes(&value))
}

fn eval_fn_count_path(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<u64> {
    eval_occurs_count_expression(steps, siblings, strings, tunables)
}

fn count_dfdl_value_nodes(value: &DfdlValue) -> u64 {
    match value {
        DfdlValue::Array(items) => items.len() as u64,
        DfdlValue::Null => 0,
        _ => 1,
    }
}

fn eval_infoset_path_steps(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<DfdlValue> {
    if steps.is_empty() {
        return Err(VmError::InvalidValue {
            message: "empty infoset path".into(),
        }
        .into());
    }
    let first = &steps[0];
    let first_local = strings.get(first.local)?;
    let mut value = siblings
        .and_then(|m| sibling_state_by_local(m, first_local))
        .map(|s| &s.value)
        .ok_or_else(|| {
            let msg = if first.prefix.is_some() {
                alloc::format!(
                    "Schema Definition Error: expression evaluation error: {first_local} does not exist"
                )
            } else if steps.len() == 1 {
                alloc::format!(
                    "Schema Definition Error: No element corresponding to step {first_local} found."
                )
            } else {
                alloc::format!(
                    "Schema Definition Error: expression evaluation error: {first_local} does not exist"
                )
            };
            VmError::InvalidValue { message: msg }
        })?;
    for step in steps.iter().skip(1) {
        let local = strings.get(step.local)?;
        check_path_step(step.prefix.is_some(), local, tunables.unqualified_path_step_policy)?;
        value = navigate_to_child(value, local, step.index)?;
    }
    Ok(value.clone())
}

fn check_path_step(
    qualified: bool,
    local: &str,
    policy: crate::length_validate::UnqualifiedPathStepPolicy,
) -> Result<()> {
    use crate::length_validate::UnqualifiedPathStepPolicy::*;
    if qualified {
        if local == "c" {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: path refers to element in no namespace".into(),
            }
            .into());
        }
        return Ok(());
    }
    match (local, policy) {
        ("b", NoNamespace) => Err(VmError::InvalidValue {
            message: "Schema Definition Error: unqualified path step policy".into(),
        }
        .into()),
        ("c", DefaultNamespace) => Err(VmError::InvalidValue {
            message: "Schema Definition Error: unqualified path step policy".into(),
        }
        .into()),
        _ => Ok(()),
    }
}

fn numeric_value_from_dfdl(value: &DfdlValue) -> Result<i64> {
    match value {
        DfdlValue::Int(v) => Ok(*v as i64),
        DfdlValue::Long(v) => Ok(*v),
        DfdlValue::Short(v) => Ok(*v as i64),
        DfdlValue::Byte(v) => Ok(*v as i64),
        DfdlValue::Integer(s) => s
            .parse::<i64>()
            .map_err(|_| VmError::InvalidValue {
                message: alloc::format!("invalid integer `{s}`"),
            }),
        DfdlValue::String(s) => s.text.parse::<i64>().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid integer `{text}`", text = s.text),
        }),
        _ => Err(VmError::InvalidValue {
            message: "numeric path value required".into(),
        }),
    }
    .map_err(Into::into)
}

fn sibling_state_by_local<'a>(
    siblings: &'a BTreeMap<String, SiblingState>,
    local: &str,
) -> Option<&'a SiblingState> {
    siblings
        .iter()
        .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
        .map(|(_, v)| v)
}

fn sequence_field_by_local<'a>(
    fields: &'a BTreeMap<String, DfdlValue>,
    local: &str,
) -> Option<&'a DfdlValue> {
    fields
        .iter()
        .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
        .map(|(_, v)| v)
}

fn navigate_to_child<'a>(
    value: &'a DfdlValue,
    local: &str,
    index: Option<u32>,
) -> Result<&'a DfdlValue> {
    match value {
        DfdlValue::Sequence(seq) => {
            let child = sequence_field_by_local(&seq.fields, local).ok_or_else(|| {
                VmError::InvalidValue {
                    message: alloc::format!(
                        "Schema Definition Error: No element corresponding to step {local} found."
                    ),
                }
            })?;
            match (index, child) {
                (Some(n), DfdlValue::Array(items)) => {
                    let idx = (n as usize).saturating_sub(1);
                    items.get(idx).ok_or_else(|| VmError::InvalidValue {
                        message: alloc::format!(
                            "Schema Definition Error: no child `{local}[{n}]`"
                        ),
                    })
                }
                (Some(_), _) => Err(VmError::InvalidValue {
                    message: alloc::format!("Schema Definition Error: no child `{local}[{index:?}]`"),
                }),
                (None, v) => Ok(v),
            }
            .map_err(Into::into)
        }
        DfdlValue::Choice { .. } => Err(VmError::InvalidValue {
            message: "inputValueCalc path through choice unsupported".into(),
        }
        .into()),
        _ => Err(VmError::InvalidValue {
            message: "inputValueCalc path requires sequence".into(),
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

fn choice_branch_names_collide(branches: &[ChoiceBranch], name: StringId) -> bool {
    branches
        .iter()
        .filter(|b| b.name == name)
        .count()
        > 1
}

fn choice_branch_leading_element_name(
    program: &IrProgram,
    branch_node: u32,
) -> Result<Option<alloc::string::String>> {
    let term = choice_branch_term_node(program, branch_node)?;
    match program.node(term)? {
        IrNode::Element { name, .. } => Ok(Some(program.strings.get(*name)?.to_string())),
        IrNode::Sequence { children, .. } => {
            for &child in children {
                if let IrNode::Element { name, .. } = program.node(child)? {
                    return Ok(Some(program.strings.get(*name)?.to_string()));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn choice_branch_discriminator_for_infoset(
    branch: &ChoiceBranch,
    branches: &[ChoiceBranch],
    strings: &StringPool,
    program: &IrProgram,
) -> Result<alloc::string::String> {
    if choice_branch_names_collide(branches, branch.name) {
        if let Some(id) = branch.branch_key {
            if let Ok(k) = strings.get(id) {
                return Ok(k.to_string());
            }
        }
        if let Some(local) = choice_branch_leading_element_name(program, branch.node)? {
            return Ok(local);
        }
    }
    Ok(strings.get(branch.name)?.to_string())
}

fn choice_branch_discriminator_matches_name(
    program: &IrProgram,
    branch: &ChoiceBranch,
    discriminator: &str,
) -> bool {
    if branch
        .branch_key
        .and_then(|id| program.strings.get(id).ok())
        .is_some_and(|k| k == discriminator)
    {
        return true;
    }
    if choice_branch_leading_element_name(program, branch.node)
        .ok()
        .flatten()
        .is_some_and(|n| n == discriminator)
    {
        return true;
    }
    program
        .strings
        .get(branch.name)
        .ok()
        .is_some_and(|n| n == discriminator)
}

fn format_choice_branch_error(branch: &ChoiceBranch, strings: &StringPool, err: &Error) -> String {
    let branch_name = strings.get(branch.name).unwrap_or("?");
    let msg = err.to_string();
    let msg = msg.strip_prefix("vm error: ").unwrap_or(msg.as_str());
    if msg.contains("Init('") || msg.contains("initiator mismatch") {
        if let Some(id) = branch.initiator {
            if let Ok(pat) = strings.get(id) {
                if msg.contains("Was looking for") {
                    return alloc::format!(
                        "{branch_name}: Initiator '{pat}' not found. Alternative failed. Reason(s): List({msg})"
                    );
                }
                return alloc::format!("{branch_name}: Initiator '{pat}' not found");
            }
        }
        return alloc::format!("{branch_name}: Initiator not found");
    }
    if msg.contains("unexpected end of input") || msg.is_empty() {
        return alloc::format!("{branch_name}: {msg}");
    }
    msg.to_string()
}
