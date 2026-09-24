//! Sequence decoding logic, child iteration, and separator handling for VM decoder.

use super::*;
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProps, StringId, StringPool};
use crate::schema::boolean_reps::BooleanSiblingEnv;
use crate::schema::{
    EmptyElementParsePolicy, LengthKind, OccursCountKind, SeparatorPosition, SequenceKind,
};
use crate::value::DfdlValue;
use crate::vm::runtime::{encoding_name, format_found_at_cursor, has_non_empty_terminator, Cursor};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn sibling_text_map_for_delimiters(
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

pub(crate) fn sibling_boolean_env<'a>(
    siblings: Option<&'a BTreeMap<String, SiblingState>>,
    text_map: &'a BTreeMap<String, String>,
    bytes_map: &'a BTreeMap<String, usize>,
) -> Option<BooleanSiblingEnv<'a>> {
    siblings?;
    Some(BooleanSiblingEnv {
        text: text_map,
        content_bytes: bytes_map,
    })
}

pub(crate) fn sibling_maps_for_boolean(
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

pub(crate) fn props_contribute_delimiter_stops(
    props: &IrProps,
    strings: &StringPool,
) -> Result<bool> {
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

pub(crate) fn cursor_at_own_sequence_terminator(
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
    Ok(crate::schema::match_delimiter_opts_for_encoding(
        &cursor.data[cursor.pos..],
        pat,
        seq_props.ignore_case,
        enc,
    )
    .is_some_and(|n| n > 0))
}

pub(crate) fn trailing_empty_strict_parse_error() -> Error {
    VmError::InvalidValue {
        message:
            "Parse Error. Empty trailing optional separatorSuppressionPolicy trailingEmptyStrict"
                .into(),
    }
    .into()
}

pub(crate) fn cursor_at_parent_infix_separator(
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
    Ok(crate::schema::match_delimiter_opts_for_encoding(
        &cursor.data[cursor.pos..],
        pat,
        parent.ignore_case,
        None,
    )
    .is_some_and(|n| n > 0))
}

pub(crate) fn filter_delimiter_stop_sequences<'a>(
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

impl<'a> Decoder<'a> {
    pub(crate) fn current_delimiter_occurrence_index(&self) -> u64 {
        self.delimiter_occurrence_stack
            .borrow()
            .last()
            .copied()
            .unwrap_or(1)
    }

    pub(crate) fn xpath_delimiter_value_map(&self) -> BTreeMap<String, DfdlValue> {
        let mut out = BTreeMap::new();
        for frame in self.xpath_ancestor_frames.borrow().iter() {
            for (k, v) in frame {
                out.insert(k.clone(), v.value.clone());
            }
        }
        for (k, v) in self.xpath_siblings.borrow().iter() {
            out.insert(k.clone(), v.value.clone());
        }
        out
    }

    pub(crate) fn delimiter_occurrence_index_for_sequence(&self) -> u64 {
        let stack = self.delimiter_occurrence_stack.borrow();
        match stack.len() {
            0 => 1,
            1 => stack[0],
            2 => stack[1],
            n => stack[n - 2],
        }
    }

    pub(crate) fn build_resolved_stop_delimiter_literals(
        &self,
        stop_sequences: &[&IrProps],
        parent_sequence: Option<&IrProps>,
    ) -> alloc::vec::Vec<(StringId, String)> {
        use alloc::collections::BTreeSet;
        let mut seen = BTreeSet::new();
        let mut out = alloc::vec::Vec::new();
        let mut collect = |props: &IrProps| {
            for id in crate::vm::runtime::delimiter_pattern_ids(props) {
                if !seen.insert(id) {
                    continue;
                }
                let Ok(raw) = self.ctx.strings().get(id) else {
                    continue;
                };
                if raw.trim().starts_with('{') {
                    out.push((
                        id,
                        self.resolve_delimiter_property(
                            raw,
                            self.delimiter_occurrence_index_for_sequence(),
                        ),
                    ));
                }
            }
        };
        for stop in stop_sequences {
            collect(stop);
        }
        if let Some(parent) = parent_sequence {
            collect(parent);
        }
        out
    }

    pub(crate) fn resolve_delimiter_property(&self, pat: &str, occurs_index: u64) -> String {
        if !pat.trim().starts_with('{') {
            return pat.to_string();
        }
        let values = self.xpath_delimiter_value_map();
        let idx = occurs_index;
        if let Some(s) = crate::schema::eval_path_indexed_delimiter_expression(pat, idx, &values) {
            return s;
        }
        let mut sib_text = BTreeMap::new();
        for (k, v) in &values {
            if let Some(s) = v.as_str() {
                sib_text.insert(k.clone(), s.to_string());
            }
        }
        if let Some(s) = crate::schema::eval_runtime_delimiter_expression(pat, &sib_text) {
            return s;
        }
        pat.to_string()
    }

    pub(crate) fn runtime_check_bit_order_change(
        &self,
        props: &IrProps,
        cursor: &Cursor,
    ) -> Result<()> {
        if !props.bit_order_defined {
            return Ok(());
        }
        let order = props.bit_order;
        if let Some(prev) = *self.seq_bit_order.borrow() {
            if prev != order && !cursor.absolute_bit_index().is_multiple_of(8) {
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

    pub(crate) fn note_sequence_field_bit_order(&self, props: &IrProps) {
        if props.bit_order_defined {
            *self.seq_bit_order.borrow_mut() = Some(props.bit_order);
        }
    }

    pub(crate) fn skip_infix_sep_after_parsed_unbounded_array(
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
        let IrNode::Element {
            props: prev_props, ..
        } = self.ctx.program.node(prev)?
        else {
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
            enc,
        )
        .is_some();
        Ok(!at_sep)
    }

    pub(crate) fn decode_node(
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
                self.push_xpath_ancestor_frame(siblings);
                struct XpathAncestorGuard<'a>(&'a Decoder<'a>);
                impl Drop for XpathAncestorGuard<'_> {
                    fn drop(&mut self) {
                        self.0.pop_xpath_ancestor_frame();
                    }
                }
                let _xpath_ancestor_guard = XpathAncestorGuard(self);
                self.seed_xpath_siblings(siblings);
                *self.seq_bit_order.borrow_mut() = None;
                self.evaluate_and_set_variables(&props.set_variables, siblings)?;
                let _var_scope =
                    self.enter_variable_scope(&props.new_variable_instances, siblings, false)?;
                let mut initiator_alt = None;
                if let Some(id) = props.initiator {
                    let pat = self.resolve_delimiter_property(
                        self.ctx.strings().get(id)?,
                        self.current_delimiter_occurrence_index(),
                    );
                    let enc = encoding_name(props, self.ctx.strings()).ok();
                    if !pat.is_empty() {
                        let (_len, alt) = cursor
                            .consume_delimiter_with_alt(&pat, props.ignore_case, enc)
                            .ok_or_else(|| VmError::InvalidValue {
                                message: "sequence initiator mismatch".into(),
                            })?;
                        initiator_alt = Some(alt);
                    }
                }
                if props.sequence_kind == SequenceKind::Unordered {
                    if props.separator.is_none() {
                        let res = self.decode_unordered_unseparated_sequence(
                            node_id,
                            children,
                            props,
                            cursor,
                            has_following_sibling,
                            parent_sequence,
                            siblings,
                            content_scope_bytes,
                            pattern_text_frame,
                            stop_sequences,
                        )?;
                        self.consume_terminator(props, cursor)?;
                        return Ok(res);
                    }
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
                            stop_sequences,
                            initiator_alt,
                            Vec::new(),
                            Vec::new(),
                            BTreeMap::new(),
                        );
                    }
                }
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
                        stop_sequences,
                        initiator_alt,
                        Vec::new(),
                        Vec::new(),
                        BTreeMap::new(),
                    );
                }
                let mut child_stops = stop_sequences.to_vec();
                if props.separator.is_some() || has_non_empty_terminator(props, self.ctx.strings())?
                {
                    child_stops.push(props);
                }
                let mut map = BTreeMap::new();
                let mut seq_siblings = siblings.cloned().unwrap_or_default();
                let mut infix_sep_newline_prefix = Vec::new();
                let mut separator_alts = Vec::new();
                self.field_delimiters.borrow_mut().clear();
                let total = children.len();
                let saved_bit_order = *self.seq_bit_order.borrow();
                for (i, &child) in children.iter().enumerate() {
                    let has_following = i + 1 < total || has_following_sibling;
                    let last_child_slot = i + 1 >= total;
                    if props.separator.is_some()
                        && props.separator_position == SeparatorPosition::Infix
                        && i > 0
                        && self.skip_infix_sep_after_parsed_unbounded_array(
                            props, children, i, cursor,
                        )?
                    {
                    } else {
                        let sep_alt = self.consume_separator(
                            props,
                            cursor,
                            i,
                            total,
                            &mut infix_sep_newline_prefix,
                            &child_stops,
                            last_child_slot,
                        )?;
                        if let Some(alt) = sep_alt {
                            separator_alts.push(Some(alt));
                        }
                    }
                    if let Ok(IrNode::Element {
                        props: child_props, ..
                    }) = self.ctx.program.node(child)
                    {
                        if self.trailing_empty_implicit_optional_empty_slot(
                            props,
                            child_props,
                            i,
                            total,
                            cursor,
                            &child_stops,
                        )? {
                            continue;
                        }
                    }
                    let child_start = cursor.pos;
                    let child_res = self.decode_particle(
                        child,
                        cursor,
                        has_following,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        &child_stops,
                    );
                    match child_res {
                        Ok(val) => {
                            if let Ok(IrNode::Element {
                                name,
                                props: child_props,
                                ..
                            }) = self.ctx.program.node(child)
                            {
                                let key = self.ctx.strings().get(*name)?.to_string();
                                let consumed = cursor.pos.saturating_sub(child_start);
                                let content_bytes =
                                    if child_props.length_kind == LengthKind::Prefixed {
                                        crate::vm::runtime::prefixed_payload_byte_length(
                                            &cursor.data[child_start..cursor.pos],
                                            child_props,
                                            self.ctx.strings(),
                                        )?
                                    } else {
                                        consumed
                                    };
                                let state = SiblingState {
                                    value: val.clone(),
                                    content_bytes,
                                };
                                insert_seq_sibling(&mut seq_siblings, key.clone(), state.clone());
                                self.insert_xpath_sibling(key, state);
                                if self.consume_trailing_empty_infix_separators(
                                    props,
                                    child_props,
                                    cursor,
                                )? {
                                    insert_child(&mut map, child, val, self.ctx.program)?;
                                    break;
                                }
                            }
                            insert_child(&mut map, child, val, self.ctx.program)?;
                        }
                        Err(e) => {
                            if is_element_absent(&e) {
                                if let Ok(IrNode::Element {
                                    props: child_props, ..
                                }) = self.ctx.program.node(child)
                                {
                                    let strict_err = props.separator_suppression_policy
                                        == Some(
                                            crate::schema::SeparatorSuppressionPolicy::TrailingEmptyStrict,
                                        )
                                        && child_props.occurs_count_kind == OccursCountKind::Implicit
                                        && child_props.occurs_min == 0
                                        && cursor_at_parent_infix_separator(
                                            cursor,
                                            parent_sequence,
                                            self.ctx.strings(),
                                        )?;
                                    if strict_err {
                                        return Err(trailing_empty_strict_parse_error());
                                    }
                                }
                                continue;
                            }
                            return Err(e);
                        }
                    }
                }
                *self.seq_bit_order.borrow_mut() = saved_bit_order;
                let mut terminator_alt = None;
                if let Some(id) = props.terminator {
                    let pat = self.resolve_delimiter_property(
                        self.ctx.strings().get(id)?,
                        self.current_delimiter_occurrence_index(),
                    );
                    let enc = encoding_name(props, self.ctx.strings()).ok();
                    if !pat.is_empty() {
                        if let Some((_len, alt)) =
                            cursor.consume_delimiter_with_alt(&pat, props.ignore_case, enc)
                        {
                            terminator_alt = Some(alt);
                        } else if !cursor.is_empty() {
                            return Err(VmError::InvalidValue {
                                message: "sequence terminator mismatch".into(),
                            }
                            .into());
                        }
                    }
                }
                let mut field_delim_meta = BTreeMap::new();
                for (k, v) in self.field_delimiters.borrow().iter() {
                    field_delim_meta.insert(k.clone(), v.clone());
                }
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
            IrNode::Choice { .. } => self.decode_choice(
                node_id,
                cursor,
                has_following_sibling,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            ),
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

    pub(crate) fn initiated_content_uses_initiator_discriminator(&self, props: &IrProps) -> bool {
        props.occurs_min == 0 && props.occurs_max != Some(1)
    }

    pub(crate) fn skip_initiated_content_sibling(
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
        if fi <= ci {
            return Ok(false);
        }
        let IrNode::Element { props, .. } = self.ctx.program.node(child)? else {
            return Ok(false);
        };
        let Some(init_id) = props.initiator else {
            return Ok(false);
        };
        let pat = self.resolve_delimiter_property(
            self.ctx.strings().get(init_id)?,
            self.current_delimiter_occurrence_index(),
        );
        if pat.is_empty() {
            return Ok(false);
        }
        let enc = encoding_name(props, self.ctx.strings()).ok();
        Ok(crate::schema::match_delimiter_opts_for_encoding(
            &cursor.data[cursor.pos..],
            &pat,
            props.ignore_case,
            enc,
        )
        .is_none())
    }

    pub(crate) fn unordered_sequence_uses_initiator_scan(&self, children: &[u32]) -> bool {
        children.iter().all(|&child| {
            matches!(
                self.ctx.program.node(child),
                Ok(IrNode::Element { props, .. }) if props.initiator.is_some()
            )
        })
    }

    pub(crate) fn decode_unordered_unseparated_sequence(
        &self,
        node_id: u32,
        children: &[u32],
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        _parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<DfdlValue> {
        let mut map = BTreeMap::new();
        if self.unordered_sequence_uses_initiator_scan(children) {
            let mut seq_siblings = siblings.cloned().unwrap_or_default();
            loop {
                if cursor.is_empty() {
                    break;
                }
                let mut matched_any = false;
                for &child in children {
                    let IrNode::Element {
                        props: child_props, ..
                    } = self.ctx.program.node(child)?
                    else {
                        continue;
                    };
                    if !self.initiator_present_at_cursor(cursor, child_props)? {
                        continue;
                    }
                    let value = self.decode_one_element_occurrence(
                        child,
                        cursor,
                        has_following_sibling,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    )?;
                    let name_id = match self.ctx.program.node(child)? {
                        IrNode::Element { name, .. } => *name,
                        _ => return Err(crate::error::Error::Vm(VmError::InvalidValue { message: "expected element node".into() })),
                    };
                    let name = self
                        .ctx
                        .strings()
                        .get(name_id)?
                        .to_string();
                    insert_seq_sibling(
                        &mut seq_siblings,
                        name,
                        SiblingState {
                            value: value.clone(),
                            content_bytes: 0,
                        },
                    );
                    insert_child(&mut map, child, value, self.ctx.program)?;
                    matched_any = true;
                    break;
                }
                if !matched_any {
                    break;
                }
            }
            self.validate_initiated_unordered_min_occurs(children, &map)?;
        } else {
            let mut visited = alloc::vec::Vec::new();
            self.unordered_unseparated_backtrack(
                children,
                0,
                cursor,
                &mut map,
                &mut visited,
                has_following_sibling,
                Some(props),
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            )?;
        }
        let _ = node_id;
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
    }

    pub(crate) fn unordered_unseparated_backtrack(
        &self,
        children: &[u32],
        child_index: usize,
        cursor: &mut Cursor<'_>,
        map: &mut BTreeMap<String, DfdlValue>,
        visited: &mut alloc::vec::Vec<usize>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        stop_sequences: &[&IrProps],
    ) -> Result<()> {
        let _ = child_index;
        if visited.len() == children.len() {
            return Ok(());
        }
        for (i, &child) in children.iter().enumerate() {
            if visited.contains(&i) {
                continue;
            }
            let saved = cursor.clone();
            let saved_map = map.clone();
            let has_following = (visited.len() + 1 < children.len()) || has_following_sibling;
            let IrNode::Element {
                props: child_props,
                kind,
                ..
            } = self.ctx.program.node(child)?
            else {
                continue;
            };
            let multi_occ = self.unordered_backtrack_multi_occurrence(child_props);
            if multi_occ {
                let mut current_count = self.initiated_child_occurrence_count(child, map)?;
                let mut took_any = false;
                while self.unordered_backtrack_may_take_another(child_props, current_count) {
                    let iter_saved = cursor.clone();
                    let iter_map = map.clone();
                    match self.decode_one_element_occurrence(
                        child,
                        cursor,
                        has_following,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    ) {
                        Ok(v) if self.backtrack_decoded_facets_ok(&v, *kind, child_props) => {
                            insert_child(map, child, v, self.ctx.program)?;
                            current_count += 1;
                            took_any = true;
                            if cursor.pos == iter_saved.pos {
                                break;
                            }
                        }
                        _ => {
                            *cursor = iter_saved;
                            *map = iter_map;
                            break;
                        }
                    }
                }
                if took_any || child_props.occurs_min == 0 {
                    visited.push(i);
                    let res = self.unordered_unseparated_backtrack(
                        children,
                        0,
                        cursor,
                        map,
                        visited,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    );
                    if res.is_ok() {
                        return Ok(());
                    }
                    visited.pop();
                }
                *cursor = saved;
                *map = saved_map;
                continue;
            }
            let child_res = self.decode_particle(
                child,
                cursor,
                has_following,
                parent_sequence,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            );
            match child_res {
                Ok(v) if self.backtrack_decoded_facets_ok(&v, *kind, child_props) => {
                    if self.initiated_child_occurrence_count(child, map)? > 0 {
                        let name_id = match self.ctx.program.node(child)? {
                            IrNode::Element { name, .. } => *name,
                            _ => return Err(crate::error::Error::Vm(VmError::InvalidValue { message: "expected element node".into() })),
                        };
                        let name = self
                            .ctx
                            .strings()
                            .get(name_id)?;
                        return Err(
                            self.unordered_unseparated_scalar_duplicate_error(name, child_props)
                        );
                    }
                    insert_child(map, child, v, self.ctx.program)?;
                    visited.push(i);
                    let res = self.unordered_unseparated_backtrack(
                        children,
                        0,
                        cursor,
                        map,
                        visited,
                        has_following_sibling,
                        parent_sequence,
                        siblings,
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    );
                    if res.is_ok() {
                        return Ok(());
                    }
                    visited.pop();
                    *cursor = saved;
                    *map = saved_map;
                }
                Ok(_) => {
                    *cursor = saved;
                    *map = saved_map;
                }
                Err(e) if is_element_absent(&e) => {
                    if child_props.occurs_min == 0 {
                        visited.push(i);
                        let res = self.unordered_unseparated_backtrack(
                            children,
                            0,
                            cursor,
                            map,
                            visited,
                            has_following_sibling,
                            parent_sequence,
                            siblings,
                            content_scope_bytes,
                            pattern_text_frame,
                            stop_sequences,
                        );
                        if res.is_ok() {
                            return Ok(());
                        }
                        visited.pop();
                    }
                    *cursor = saved;
                    *map = saved_map;
                }
                Err(_) => {
                    *cursor = saved;
                    *map = saved_map;
                }
            }
        }
        if visited.len() == children.len() {
            Ok(())
        } else {
            Err(VmError::InvalidValue {
                message: "unordered sequence backtrack failed".into(),
            }
            .into())
        }
    }

    pub(crate) fn unordered_unseparated_scalar_duplicate_error(
        &self,
        name: &str,
        props: &IrProps,
    ) -> Error {
        let max = props.occurs_max.unwrap_or(1);
        VmError::InvalidValue {
            message: alloc::format!(
                "Parse Error: Duplicate element `{name}` in unordered sequence exceeds maxOccurs {max}."
            ),
        }
        .into()
    }

    pub(crate) fn decode_sequence_initiated_content(
        &self,
        node_id: u32,
        children: &[u32],
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        _parent_sequence: Option<&IrProps>,
        siblings: Option<&BTreeMap<String, SiblingState>>,
        content_scope_bytes: Option<usize>,
        pattern_text_frame: bool,
        child_stops: &[&IrProps],
        initiator_alt: Option<u8>,
        mut infix_sep_newline_prefix: Vec<bool>,
        separator_alts: Vec<Option<u8>>,
        field_delim_meta: BTreeMap<String, FieldDelimiterMeta>,
    ) -> Result<DfdlValue> {
        let mut map = BTreeMap::new();
        let mut seq_siblings = siblings.cloned().unwrap_or_default();
        let mut committed_child: Option<u32> = None;
        let mut initiated_block_lower_indices: Option<usize> = None;
        let ordered_initiated =
            props.initiated_content && props.sequence_kind != SequenceKind::Unordered;
        let mut local_stops = child_stops.to_vec();
        local_stops.push(props);
        loop {
            if cursor.is_empty() {
                break;
            }
            if props.separator.is_some() {
                if props.separator_position == SeparatorPosition::Prefix {
                    let _ = self.consume_separator(
                        props,
                        cursor,
                        0,
                        children.len(),
                        &mut infix_sep_newline_prefix,
                        &local_stops,
                        false,
                    );
                } else if props.separator_position == SeparatorPosition::Infix && !map.is_empty() {
                    let _ = self.consume_separator(
                        props,
                        cursor,
                        map.len(),
                        children.len(),
                        &mut infix_sep_newline_prefix,
                        &local_stops,
                        false,
                    );
                } else if props.separator_position == SeparatorPosition::Postfix {
                    while !cursor.is_empty() {
                        let before = cursor.pos;
                        let _ = self.consume_separator(
                            props,
                            cursor,
                            1,
                            children.len(),
                            &mut infix_sep_newline_prefix,
                            &local_stops,
                            false,
                        )?;
                        if cursor.pos == before {
                            break;
                        }
                    }
                }
            }
            while cursor.pos < cursor.data.len() {
                let b = cursor.data[cursor.pos];
                if b == b'\r' || b == b'\n' {
                    cursor.advance(1);
                } else {
                    break;
                }
            }
            if cursor.is_empty() {
                break;
            }
            let mut matched_child = false;
            for (idx, &child) in children.iter().enumerate() {
                if ordered_initiated {
                    if let Some(lower) = initiated_block_lower_indices {
                        if idx < lower {
                            continue;
                        }
                    }
                }
                if let Some(committed) = committed_child {
                    if self.skip_initiated_content_sibling(cursor, child, committed, children)? {
                        continue;
                    }
                }
                let IrNode::Element {
                    props: child_props,
                    name,
                    ..
                } = self.ctx.program.node(child)?
                else {
                    continue;
                };
                if !self.initiator_present_at_cursor(cursor, child_props)? {
                    continue;
                }
                let el_name = self.ctx.strings().get(*name)?.to_string();
                let is_repeating = child_props.occurs_max.map(|m| m > 1).unwrap_or(false);
                let is_first_occurrence = !map.contains_key(&el_name);

                if self.initiated_content_uses_initiator_discriminator(child_props)
                    && is_first_occurrence
                {
                    let value = self.decode_single_element(
                        child,
                        cursor,
                        has_following_sibling,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        &local_stops,
                        false,
                    )?;
                    let state = SiblingState {
                        value: value.clone(),
                        content_bytes: 0,
                    };
                    insert_seq_sibling(&mut seq_siblings, el_name.clone(), state.clone());
                    self.insert_xpath_sibling(el_name.clone(), state);
                    insert_child(&mut map, child, value, self.ctx.program)?;
                    matched_child = true;
                    if ordered_initiated {
                        let next_idx = if is_repeating { idx } else { idx + 1 };
                        initiated_block_lower_indices = Some(next_idx);
                    }
                    if is_first_occurrence && self.discriminator_committed_branch.get() {
                        committed_child = Some(child);
                        self.discriminator_committed_branch.set(false);
                    }
                    break;
                }

                let saved_occ = cursor.clone();
                let single_elem =
                    !is_repeating || self.initiator_present_at_cursor(cursor, child_props)?;
                let value_res = if single_elem {
                    self.decode_single_element(
                        child,
                        cursor,
                        has_following_sibling,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        &local_stops,
                        false,
                    )
                } else {
                    self.decode_one_element_occurrence(
                        child,
                        cursor,
                        has_following_sibling,
                        Some(props),
                        Some(&seq_siblings),
                        content_scope_bytes,
                        pattern_text_frame,
                        &local_stops,
                    )
                };

                let value = match value_res {
                    Ok(v) => v,
                    Err(e) if is_element_absent(&e) => {
                        *cursor = saved_occ;
                        continue;
                    }
                    Err(e) => return Err(e),
                };

                let state = SiblingState {
                    value: value.clone(),
                    content_bytes: 0,
                };
                insert_seq_sibling(&mut seq_siblings, el_name.clone(), state.clone());
                self.insert_xpath_sibling(el_name.clone(), state);
                insert_child(&mut map, child, value, self.ctx.program)?;
                matched_child = true;
                if ordered_initiated {
                    let next_idx = if is_repeating { idx } else { idx + 1 };
                    initiated_block_lower_indices = Some(next_idx);
                }
                if is_first_occurrence && self.discriminator_committed_branch.get() {
                    committed_child = Some(child);
                    self.discriminator_committed_branch.set(false);
                }
                break;
            }

            if !matched_child {
                break;
            }
        }

        self.validate_initiated_unordered_min_occurs(children, &map)?;

        let mut terminator_alt = None;
        if let Some(id) = props.terminator {
            let pat = self.resolve_delimiter_property(
                self.ctx.strings().get(id)?,
                self.current_delimiter_occurrence_index(),
            );
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if !pat.is_empty() {
                if let Some((_len, alt)) =
                    cursor.consume_delimiter_with_alt(&pat, props.ignore_case, enc)
                {
                    terminator_alt = Some(alt);
                } else if !cursor.is_empty() {
                    return Err(VmError::InvalidValue {
                        message: "sequence terminator mismatch".into(),
                    }
                    .into());
                }
            }
        }

        let _ = node_id;
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

    pub(crate) fn at_enclosing_terminator_stop(
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
                enc,
            )
            .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn at_enclosing_separator_stop(
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
            let pat = self.resolve_delimiter_property(
                self.ctx.strings().get(sep_id)?,
                self.delimiter_occurrence_index_for_sequence(),
            );
            if pat.is_empty() {
                continue;
            }
            let enc = encoding_name(stop, self.ctx.strings()).ok();
            if crate::schema::match_delimiter_opts_for_encoding(
                &cursor.data[cursor.pos..],
                &pat,
                stop.ignore_case,
                enc,
            )
            .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn try_consume_treat_as_absent_separator_on_error(
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

    pub(crate) fn consume_treat_as_absent_separated_padding(
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
                Err(e) if is_element_absent(&e) || is_pattern_length_mismatch(&e) => {
                    *cursor = saved;
                    if !self.try_consume_treat_as_absent_separator(
                        props,
                        parent_sequence,
                        cursor,
                    )? {
                        break;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub(crate) fn try_consume_treat_as_absent_separator(
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

    pub(crate) fn consume_occurrence_separator(
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
        let pat_owned = self.resolve_delimiter_property(
            self.ctx.strings().get(id)?,
            self.delimiter_occurrence_index_for_sequence(),
        );
        let pat = pat_owned.as_str();
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
                if cursor.consume_delimiter(pat, props.ignore_case, enc) {
                    return Ok(());
                }
                let nested_infix_occurrence = self.parent_infix_consumed_by_occurrence_loop.get()
                    || self.occurrence_decode_depth.get() > 1;
                if nested_infix_occurrence && !cursor.is_empty() {
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
                    && (pat.contains('\n') || pat.trim() == "%NL;" || pat.starts_with("%NL")) =>
            {
                !crate::vm::runtime::should_suppress_decode_occurrence_separator(
                    props,
                    ip,
                    items,
                    self.ctx.strings(),
                )?
            }
            _ => false,
        };
        if require {
            if cursor.consume_delimiter(pat, props.ignore_case, enc) {
                return Ok(());
            }
            let found_display = format_found_at_cursor(cursor.data, cursor.pos, enc);
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Parse Error. infix separator. Delimiter not found!  Was looking for ({pat}) but found \"{found_display}\" instead"
                ),
            }
            .into());
        }
        let mandatory_postfix = props.separator_position == SeparatorPosition::Postfix
            && item_props.is_some_and(|ip| !crate::ir::ir_props_has_input_value_calc(ip))
            && items.is_some_and(|it| !it.is_empty());
        if mandatory_postfix {
            if cursor.consume_delimiter(pat, props.ignore_case, enc) {
                return Ok(());
            }
            if item_props.is_some_and(|ip| {
                ip.length_kind == LengthKind::Delimited || ip.length_kind == LengthKind::Implicit
            }) {
                return Ok(());
            }
            if !cursor.is_empty()
                && crate::schema::match_delimiter_opts_for_encoding(
                    &cursor.data[cursor.pos..],
                    pat,
                    props.ignore_case,
                    enc,
                )
                .is_none()
            {
                return Ok(());
            }
            let found_display = if cursor.is_empty() {
                "End of file".into()
            } else {
                format_found_at_cursor(cursor.data, cursor.pos, enc)
            };
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
            enc,
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
            if !cursor.consume_delimiter(pat, props.ignore_case, enc) {
                return Err(VmError::InvalidValue {
                    message: "separator mismatch".into(),
                }
                .into());
            }
        }
        Ok(())
    }

    pub(crate) fn consume_trailing_empty_position_separator(
        &self,
        seq_props: &IrProps,
        child_props: &IrProps,
        child_index: usize,
        child_total: usize,
        cursor: &mut Cursor<'_>,
    ) -> Result<bool> {
        use crate::schema::SeparatorSuppressionPolicy;
        if seq_props.separator_suppression_policy != Some(SeparatorSuppressionPolicy::TrailingEmpty)
        {
            return Ok(false);
        }
        if child_props.occurs_count_kind != OccursCountKind::Implicit || child_props.occurs_min != 0
        {
            return Ok(false);
        }
        let last = child_index + 1 >= child_total;
        if last && child_props.occurs_max.is_none() {
            return Ok(false);
        }
        if !last {
            return self.consume_one_sequence_infix_separator(seq_props, cursor);
        }
        Ok(false)
    }

    pub(crate) fn consume_one_sequence_infix_separator(
        &self,
        seq_props: &IrProps,
        cursor: &mut Cursor<'_>,
    ) -> Result<bool> {
        if seq_props.separator_position != SeparatorPosition::Infix {
            return Ok(false);
        }
        let Some(sep_id) = seq_props.separator else {
            return Ok(false);
        };
        let pat = self.ctx.strings().get(sep_id)?;
        if pat.is_empty() {
            return Ok(false);
        }
        let enc = encoding_name(seq_props, self.ctx.strings()).ok();
        Ok(cursor.consume_delimiter(pat, seq_props.ignore_case, enc))
    }

    pub(crate) fn trailing_empty_implicit_optional_empty_slot(
        &self,
        seq_props: &IrProps,
        child_props: &IrProps,
        child_index: usize,
        child_total: usize,
        cursor: &mut Cursor<'_>,
        stop_sequences: &[&IrProps],
    ) -> Result<bool> {
        use crate::schema::SeparatorSuppressionPolicy;
        if seq_props.separator_suppression_policy != Some(SeparatorSuppressionPolicy::TrailingEmpty)
        {
            return Ok(false);
        }
        if child_props.occurs_count_kind != OccursCountKind::Implicit || child_props.occurs_min != 0
        {
            return Ok(false);
        }
        let empty_slot =
            crate::vm::runtime::would_read_empty_delimited_field(
                cursor,
                child_props,
                self.ctx.strings(),
                stop_sequences,
            )? || cursor_at_parent_infix_separator(cursor, Some(seq_props), self.ctx.strings())?;
        if !empty_slot {
            return Ok(false);
        }
        self.consume_trailing_empty_position_separator(
            seq_props,
            child_props,
            child_index,
            child_total,
            cursor,
        )?;
        Ok(true)
    }

    pub(crate) fn consume_trailing_empty_infix_separators(
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
            enc,
        )
        .is_some()
        {
            if !cursor.consume_delimiter(pat, seq_props.ignore_case, enc) {
                break;
            }
            consumed_any = true;
        }
        Ok(consumed_any)
    }

    pub(crate) fn consume_separator(
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
            let pat_owned = self.resolve_delimiter_property(
                self.ctx.strings().get(id)?,
                self.delimiter_occurrence_index_for_sequence(),
            );
            let pat = pat_owned.as_str();
            if let Some(err) =
                self.separator_enclosing_delimiter_conflict(props, pat, cursor, stop_sequences)
            {
                return Err(err);
            }
            if crate::schema::is_nl_comma_space_pattern(pat)
                && props.separator_position == SeparatorPosition::Infix
                && index > 0
            {
                if let Some((n, had_nl)) = crate::schema::match_nl_comma_space_separator_with_flag(
                    &cursor.data[cursor.pos..],
                ) {
                    cursor.advance(n);
                    infix_sep_newline_prefix.push(had_nl);
                    return Ok(None);
                }
            }
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if let Some((_, alt)) = cursor.consume_delimiter_with_alt(pat, props.ignore_case, enc) {
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
            let found_display = format_found_at_cursor(cursor.data, cursor.pos, enc);
            if props.separator_position == SeparatorPosition::Infix {
                if allow_missing_infix_after_last_slot {
                    let sep_at_cursor = crate::schema::match_delimiter_opts_for_encoding(
                        &cursor.data[cursor.pos..],
                        pat,
                        props.ignore_case,
                        enc,
                    )
                    .is_some();
                    if !sep_at_cursor
                        && (cursor.is_empty()
                            || self.at_enclosing_terminator_stop(cursor, stop_sequences)?)
                    {
                        return Ok(None);
                    }
                }
                if self.at_enclosing_separator_stop(cursor, stop_sequences, None)? {
                    return Ok(None);
                }
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. Failed to find infix separator. Delimiter not found!  Was looking for ({pat}) but found \"{found_display}\" instead"
                    ),
                }
                .into());
            }
            if props.separator_position == SeparatorPosition::Postfix {
                return Ok(None);
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

    pub(crate) fn separator_enclosing_delimiter_conflict(
        &self,
        props: &IrProps,
        sep_pat: &str,
        cursor: &Cursor<'_>,
        stop_sequences: &[&IrProps],
    ) -> Option<Error> {
        let enc = encoding_name(props, self.ctx.strings()).ok();
        let enc_term = self
            .enclosing
            .borrow()
            .last()
            .and_then(|p| p.terminator)
            .and_then(|id| self.ctx.strings().get(id).ok().map(|s| s.to_string()));
        let enc_name = self.enclosing_names.borrow().last().cloned();
        if let (Some(enc_term), Some(enc_name)) = (enc_term, enc_name) {
            if !enc_term.is_empty() && enc_term != sep_pat {
                let bytes = &cursor.data[cursor.pos..];
                let sep_len = crate::schema::match_delimiter_opts_for_encoding(
                    bytes,
                    sep_pat,
                    props.ignore_case,
                    enc,
                );
                let term_len = crate::schema::match_delimiter_opts_for_encoding(
                    bytes,
                    &enc_term,
                    props.ignore_case,
                    enc,
                );
                if let (Some(sl), Some(tl)) = (sep_len, term_len) {
                    if sl > 0 && tl >= sl {
                        return Some(
                            VmError::InvalidValue {
                                message: alloc::format!(
                                    "Parse Error. {sep_pat} - Choice failed. Delimiter ({enc_term}) found for enclosing complex element {enc_name}"
                                ),
                            }
                            .into(),
                        );
                    }
                }
            }
        }

        for &stop in stop_sequences.iter().rev() {
            let Some(term_id) = stop.terminator else {
                continue;
            };
            let Ok(stop_term) = self.ctx.strings().get(term_id) else {
                continue;
            };
            if stop_term.is_empty() || stop_term == sep_pat {
                continue;
            }
            let bytes = &cursor.data[cursor.pos..];
            let sep_len = crate::schema::match_delimiter_opts_for_encoding(
                bytes,
                sep_pat,
                props.ignore_case,
                enc,
            );
            let stop_len = crate::schema::match_delimiter_opts_for_encoding(
                bytes,
                stop_term,
                stop.ignore_case,
                enc,
            );
            if let (Some(sl), Some(tl)) = (sep_len, stop_len) {
                if sl > 0 && tl >= sl {
                    return Some(
                        VmError::InvalidValue {
                            message: alloc::format!(
                                "Parse Error. {sep_pat} - Choice failed. Delimiter ({stop_term}) found for enclosing complex element"
                            ),
                        }
                        .into(),
                    );
                }
            }
        }
        None
    }

    pub(crate) fn following_sibling_consumes_input(&self, children: &[u32], idx: usize) -> bool {
        for &sibling_id in children.iter().skip(idx + 1) {
            if self.particle_consumes_input(sibling_id) {
                return true;
            }
        }
        false
    }

    pub(crate) fn particle_consumes_input(&self, node_id: u32) -> bool {
        match self.ctx.program.node(node_id) {
            Ok(IrNode::Element { props, child, .. }) => {
                if props.occurs_min > 0 {
                    return true;
                }
                if let Some(child_id) = child {
                    return self.particle_consumes_input(*child_id);
                }
                false
            }
            Ok(IrNode::Sequence { children, .. }) => {
                children.iter().any(|&c| self.particle_consumes_input(c))
            }
            Ok(IrNode::Choice { branches, .. }) => branches
                .iter()
                .any(|b| self.particle_consumes_input(b.node)),
            _ => false,
        }
    }

    pub(crate) fn cursor_only_consumes_infix_separators(
        cursor: &mut Cursor<'_>,
        sep_pat: &str,
        ignore_case: bool,
        enc: Option<&str>,
    ) -> Result<bool> {
        if cursor.is_empty() || sep_pat.is_empty() {
            return Ok(cursor.is_empty());
        }
        let start_pos = cursor.pos;
        while !cursor.is_empty() {
            let before = cursor.pos;
            if !cursor.consume_delimiter(sep_pat, ignore_case, enc) {
                cursor.pos = start_pos;
                return Ok(false);
            }
            if cursor.pos == before {
                break;
            }
        }
        Ok(cursor.is_empty())
    }

    pub(crate) fn suffix_is_only_infix_separators(
        cursor: &Cursor<'_>,
        sep_pat: &str,
        ignore_case: bool,
        enc: Option<&str>,
    ) -> bool {
        let mut probe = cursor.clone();
        Self::cursor_only_consumes_infix_separators(&mut probe, sep_pat, ignore_case, enc)
            .unwrap_or(false)
    }

    pub(crate) fn inner_sequence_separator(&self, child_id: u32) -> Result<Option<String>> {
        let Ok(IrNode::Sequence { props, .. }) = self.ctx.program.node(child_id) else {
            return Ok(None);
        };
        let Some(sep_id) = props.separator else {
            return Ok(None);
        };
        let sep = self.ctx.strings().get(sep_id)?.to_string();
        Ok(Some(sep))
    }

    pub(crate) fn inner_sequence_first_particle_min(&self, child_id: u32) -> Result<u64> {
        let Ok(IrNode::Sequence { children, .. }) = self.ctx.program.node(child_id) else {
            return Ok(0);
        };
        let Some(&first) = children.first() else {
            return Ok(0);
        };
        let Ok(IrNode::Element { props, .. }) = self.ctx.program.node(first) else {
            return Ok(0);
        };
        Ok(props.occurs_min)
    }

    pub(crate) fn implicit_complex_consumed_parent_postfix(&self) -> bool {
        let mut f = self.parent_postfix_sep_consumed.borrow_mut();
        if *f {
            *f = false;
            true
        } else {
            false
        }
    }
}
