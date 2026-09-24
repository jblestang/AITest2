//! Simple & complex element decoding, IVC handling, and nil evaluation for VM decoder.

use super::*;
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProps, ValueKind};
use crate::length_validate::{binary_length_validation_applies, validate_data_length_vm};
use crate::schema::{
    match_length_pattern, LengthKind, LengthUnits, Representation, SeparatorPosition,
};
use crate::value::DfdlValue;
use crate::vm::runtime::{
    consume_element_framing, consume_element_trailing_framing, consume_enclosing_delimiter,
    encoding_name, read_delimited_bytes, read_length_span, read_prefixed_payload, read_simple,
    read_until_delimiters, read_until_separator, validate_explicit_decimal_before_decode,
    would_read_empty_delimited_field,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

pub(crate) enum FramingExtraOccurrences {
    None,
    One,
    Double,
}

impl<'a> Decoder<'a> {
    pub(crate) fn decode_one_element_occurrence(
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
        one_shot.occurs_min = if self.initiator_present_at_cursor(cursor, props)? {
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

    pub(crate) fn validate_initiated_unordered_min_occurs(
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

    pub(crate) fn decode_single_element(
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
        let res = self.decode_single_element_inner(
            node_id,
            cursor,
            require_delimiter,
            parent_sequence,
            siblings,
            content_scope_bytes,
            pattern_text_frame,
            stop_sequences,
            parent_infix_consumed_by_occurrence_loop,
        )?;
        if let Ok(IrNode::Element { props, name, .. }) = self.ctx.program.node(node_id) {
            if !props.set_variables.is_empty() {
                let val_str = dfdl_value_to_string(&res);
                self.runtime_variables
                    .borrow_mut()
                    .insert(".".to_string(), val_str);
                let local_name = self.ctx.strings().get(*name)?.to_string();
                let mut sib_snap = self.xpath_siblings_snapshot();
                sib_snap.insert(
                    ".".to_string(),
                    SiblingState {
                        value: res.clone(),
                        content_bytes: 0,
                    },
                );
                sib_snap.insert(
                    local_name,
                    SiblingState {
                        value: res.clone(),
                        content_bytes: 0,
                    },
                );
                self.evaluate_and_set_variables(&props.set_variables, Some(&sib_snap))?;
            }
        }
        Ok(res)
    }

    pub(crate) fn decode_single_element_inner(
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
                let _var_scope =
                    self.enter_variable_scope(&props.new_variable_instances, siblings, true)?;
                let props = resolve_length_props(
                    props,
                    siblings,
                    *kind,
                    self.ctx.strings(),
                    &self.ctx.program.tunables,
                )?;
                let props = self.resolve_conditional_byte_order(&props)?;
                if props.length_kind == LengthKind::Explicit
                    && props.length == Some(0)
                    && !crate::ir::ir_props_has_input_value_calc(&props)
                {
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
                            stop_sequences,
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
                        let bytes = cursor.read_bytes(len).ok_or(VmError::UnexpectedEof)?;
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
                                message: "unconsumed bytes in pattern-length complex element"
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
                                    message: "unconsumed bytes in explicit-length complex element"
                                        .into(),
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
                            LengthUnits::Bytes | LengthUnits::Characters => len.saturating_mul(8),
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
                        if props.truncate_specified_length_string && !cursor.is_frame_consumed() {
                            cursor.frame_bit_limit = prev_limit;
                            return Err(VmError::InvalidValue {
                                message: "unconsumed bytes in explicit-length complex element"
                                    .into(),
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
                                } else {
                                    let bytes = read_until_separator(
                                        cursor,
                                        term,
                                        false,
                                        props.ignore_case,
                                        props.escape_scheme.as_ref(),
                                    )?;
                                    let enc = encoding_name(&props, self.ctx.strings()).ok();
                                    if crate::schema::match_delimiter_opts_for_encoding(
                                        &cursor.data[cursor.pos..],
                                        term,
                                        props.ignore_case,
                                        enc,
                                    )
                                    .is_some()
                                    {
                                        let _ =
                                            cursor.consume_delimiter(term, props.ignore_case, enc);
                                    }
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
                                    matches!(p.separator_position, SeparatorPosition::Postfix)
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
                                        enc,
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
                                                enc,
                                            )
                                        {
                                            if n > 0 {
                                                let _ = cursor.consume_delimiter(
                                                    sep,
                                                    parent.ignore_case,
                                                    enc,
                                                );
                                                consumed_parent_postfix_sep = parent
                                                    .separator_position
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
                                    if let Some(n) =
                                        crate::schema::match_delimiter_opts_for_encoding(
                                            &cursor.data[cursor.pos..],
                                            sep,
                                            parent.ignore_case,
                                            enc,
                                        )
                                    {
                                        if n > 0 {
                                            let _ = cursor.consume_delimiter(
                                                sep,
                                                parent.ignore_case,
                                                enc,
                                            );
                                            consumed_parent_postfix_sep = parent.separator_position
                                                == SeparatorPosition::Postfix
                                                && cursor.pos > sep_pos_before;
                                        }
                                    }
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
                                                enc,
                                            )? {
                                            } else if !sub.is_empty() {
                                                let sep_kind = if parent.separator_position
                                                    == SeparatorPosition::Postfix
                                                {
                                                    "postfix separator"
                                                } else {
                                                    "separator"
                                                };
                                                return Err(VmError::InvalidValue {
                                                    message: alloc::format!(
                                                        "Parse Error: {sep_kind} mismatch: unconsumed bytes in separator-bounded complex element"
                                                    ),
                                                }
                                                .into());
                                            }
                                        } else {
                                            let sep_kind = if parent.separator_position
                                                == SeparatorPosition::Postfix
                                            {
                                                "postfix separator"
                                            } else {
                                                "separator"
                                            };
                                            return Err(VmError::InvalidValue {
                                                message: alloc::format!(
                                                    "Parse Error: {sep_kind} mismatch: unconsumed bytes in separator-bounded complex element"
                                                ),
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
                        || crate::vm::runtime::has_non_empty_terminator(&props, self.ctx.strings())?
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
                        None,
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
                } else if let Some(expr) = props.input_value_calc_expression.as_ref() {
                    let ivc_element = Some(crate::xml_util::local_name_str(
                        self.ctx.strings().get(*name)?,
                    ));
                    let sib_snap = self.xpath_siblings_snapshot();
                    let ancestor_frames = self.xpath_ancestor_frames.borrow();
                    let value = eval_input_value_calc_expression(
                        expr,
                        IvcEvalCtx {
                            siblings: Some(&sib_snap),
                            ancestor_frames: Some(ancestor_frames.as_slice()),
                            root_element: self.ctx.program.root_element.as_str(),
                            define_variables: &self.ctx.program.variables,
                            runtime_variables: &self.runtime_variables.borrow(),
                            element_name: ivc_element,
                        },
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        *kind,
                        &props,
                    )?;
                    self.finalize_ivc_value(value, *kind, &props)
                } else if props.input_value_calc_path.is_some() {
                    let ivc_element = Some(crate::xml_util::local_name_str(
                        self.ctx.strings().get(*name)?,
                    ));
                    let sib_snap = self.xpath_siblings_snapshot();
                    let value = eval_input_value_calc_path(
                        &props,
                        Some(&sib_snap),
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                        ivc_element,
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
                    let ivc_element = Some(crate::xml_util::local_name_str(
                        self.ctx.strings().get(*name)?,
                    ));
                    let value = eval_input_value_calc(
                        &props,
                        *kind,
                        cursor,
                        siblings,
                        self.ctx.strings(),
                        content_scope_bytes,
                        &self.runtime_variables.borrow(),
                        ivc_element,
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
                            stop_sequences,
                        )?
                    {
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            DfdlValue::Null,
                            *kind,
                        ));
                    }
                    let field_name = self.ctx.strings().get(*name)?.to_string();
                    let mut delim_meta = crate::value::FieldDelimiterMeta::default();
                    let (sibling_text_map, sibling_bytes_map) = sibling_maps_for_boolean(siblings);
                    let sibling_env =
                        sibling_boolean_env(siblings, &sibling_text_map, &sibling_bytes_map);
                    self.runtime_check_bit_order_change(&props, cursor)?;
                    let mut resolved_stop_delimiters = self
                        .build_resolved_stop_delimiter_literals(stop_sequences, parent_sequence);
                    for id in crate::vm::runtime::delimiter_pattern_ids(&props) {
                        let Ok(raw) = self.ctx.strings().get(id) else {
                            continue;
                        };
                        if !raw.trim().starts_with('{') {
                            continue;
                        }
                        let lit = self.resolve_delimiter_property(
                            raw,
                            self.current_delimiter_occurrence_index(),
                        );
                        if let Some(entry) =
                            resolved_stop_delimiters.iter_mut().find(|(i, _)| *i == id)
                        {
                            entry.1 = lit;
                        } else {
                            resolved_stop_delimiters.push((id, lit));
                        }
                    }
                    let resolved_escape = match props.escape_scheme.as_ref() {
                        Some(s) => Some(self.resolve_escape_scheme_for_decode(s)?),
                        None => None,
                    };
                    let scan_ctx = parent_sequence.map(|parent| {
                        let nested_under_repeating_particle = self.occurrence_decode_depth.get()
                            > 1
                            && parent.separator.is_some()
                            && parent.separator_position == SeparatorPosition::Infix;
                        crate::vm::runtime::SequenceChildScanContext {
                            parent_sequence: parent,
                            has_following_sibling: require_delimiter,
                            parent_infix_consumed_by_occurrence_loop:
                                parent_infix_consumed_by_occurrence_loop
                                    || self.parent_infix_consumed_by_occurrence_loop.get()
                                    || nested_under_repeating_particle,
                            resolved_stop_delimiters: if resolved_stop_delimiters.is_empty() {
                                None
                            } else {
                                Some(resolved_stop_delimiters.as_slice())
                            },
                            resolved_escape_scheme: resolved_escape.clone(),
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
                        *kind,
                        &dfdl_value_dispatch_string(&value),
                        Some(cursor),
                    )?;
                    Ok(value)
                }
            }
            _ => self.decode_node(
                node_id,
                cursor,
                false,
                None,
                siblings,
                content_scope_bytes,
                pattern_text_frame,
                stop_sequences,
            ),
        }
    }

    pub(crate) fn framing_extra_occurrence(
        &self,
        node_id: u32,
        props: &IrProps,
        parent_sequence: Option<&IrProps>,
    ) -> FramingExtraOccurrences {
        if props.nillable {
            return FramingExtraOccurrences::None;
        }
        if props.length_kind == LengthKind::Delimited
            && crate::vm::runtime::has_non_empty_terminator(props, self.ctx.strings())
                .unwrap_or(false)
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

    pub(crate) fn is_delimited_element(&self, node_id: u32, props: &IrProps) -> bool {
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

    pub(crate) fn element_consumes_enclosing_delimiter(
        &self,
        node_id: u32,
        props: &IrProps,
    ) -> bool {
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

    #[allow(dead_code)]
    pub(crate) fn is_complex_delimited_element(
        &self,
        node_id: u32,
        props: &IrProps,
    ) -> Result<bool> {
        Ok(self.is_delimited_element(node_id, props)
            && matches!(
                self.ctx.program.node(node_id)?,
                IrNode::Element { child: Some(_), .. }
            ))
    }

    pub(crate) fn should_skip_empty_complex_delimited_occurrence(
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

    pub(crate) fn consume_initiator(&self, props: &IrProps, cursor: &mut Cursor<'_>) -> Result<()> {
        if let Some(id) = props.initiator {
            let pat = self.resolve_delimiter_property(
                self.ctx.strings().get(id)?,
                self.current_delimiter_occurrence_index(),
            );
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if std::env::var("DEBUG_LION").is_ok() {
                let rem = String::from_utf8_lossy(&cursor.data[cursor.pos..]);
                std::eprintln!("CONSUME_INIT pat={pat:?} pos={} rem={rem:?}", cursor.pos);
            }
            if !pat.is_empty() && !cursor.consume_delimiter(&pat, props.ignore_case, enc) {
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
        Ok(())
    }

    pub(crate) fn consume_terminator(
        &self,
        props: &IrProps,
        cursor: &mut Cursor<'_>,
    ) -> Result<()> {
        self.consume_terminator_tag(props, cursor, "default")
    }

    pub(crate) fn consume_terminator_tag(
        &self,
        props: &IrProps,
        cursor: &mut Cursor<'_>,
        tag: &str,
    ) -> Result<()> {
        if let Some(id) = props.terminator {
            let pat = self.resolve_delimiter_property(
                self.ctx.strings().get(id)?,
                self.current_delimiter_occurrence_index(),
            );
            if pat.is_empty() {
                return Ok(());
            }
            let enc = encoding_name(props, self.ctx.strings()).ok();
            if std::env::var("DEBUG_LION").is_ok() {
                let rem = String::from_utf8_lossy(&cursor.data[cursor.pos..]);
                std::eprintln!(
                    "CONSUME_TERM[{tag}] pat={pat:?} pos={} rem={rem:?}",
                    cursor.pos
                );
            }
            if let Some(enc_name) = enc {
                if let Some(spec) = crate::vm::encoding::bits_charset_spec(enc_name) {
                    if std::env::var("DEBUG_7BIT").is_ok() {
                        std::eprintln!(
                            "CONSUME TERMINATOR BITS CHARSET at bit_idx={} pat={pat:?}",
                            cursor.absolute_bit_index()
                        );
                    }
                    let patterns = vec![crate::vm::runtime::DelimScanPattern {
                        pat: pat.clone(),
                        ignore_case: props.ignore_case,
                    }];
                    if crate::vm::runtime::consume_bits_charset_delimiter(cursor, &patterns, spec)?
                    {
                        return Ok(());
                    }
                    if cursor.is_empty() {
                        return Ok(());
                    }
                    return Err(VmError::InvalidValue {
                        message: crate::vm::runtime::format_terminator_not_found_error(&pat),
                    }
                    .into());
                }
            }
            if !cursor.consume_delimiter(&pat, props.ignore_case, enc) {
                if cursor.is_empty() {
                    return Ok(());
                }
                if std::env::var("DEBUG_LION").is_ok() {
                    let rem = String::from_utf8_lossy(&cursor.data[cursor.pos..]);
                    std::eprintln!(
                        "FAIL_TERM_NOT_FOUND tag={tag} pat={pat:?} pos={} rem={rem:?}",
                        cursor.pos
                    );
                }
                let found_display =
                    crate::vm::runtime::format_found_at_cursor(cursor.data, cursor.pos, enc);
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Parse Error. terminator mismatch tag={tag}: expected '{pat}', found \"{found_display}\""
                    ),
                }
                .into());
            }
        }
        Ok(())
    }

    pub(crate) fn consume_root_delimited_suffix(&self, cursor: &mut Cursor<'_>) -> Result<()> {
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
}
