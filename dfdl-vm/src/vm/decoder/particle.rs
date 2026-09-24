//! Particle dispatch, choice branch execution, and array occurrence decoding for VM decoder.

use super::*;
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps, ValueKind};
use crate::length_validate::{binary_length_validation_applies, validate_data_length_vm};
use crate::schema::{
    EmptyElementParsePolicy, LengthKind, OccursCountKind, Representation, SeparatorPosition,
};
use crate::value::DfdlValue;
use crate::vm::facet_validate::{
    needs_choice_discriminator_facet_check, needs_facet_validation,
    validate_assert_eq_occurs_index, validate_choice_discriminator_facets,
    validate_decoded_facets_tdml,
};
use crate::vm::runtime::{
    default_value_for, encoding_name, is_suppressible_empty_representation,
    validate_explicit_decimal_before_decode, validate_unbounded_wsp_star_terminator,
    would_read_empty_delimited_field,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

pub(crate) fn choice_branch_element_props(program: &IrProgram, node: u32) -> Option<&IrProps> {
    match &program.nodes[node as usize] {
        IrNode::Element { props, .. } => Some(props),
        IrNode::Sequence {
            props, children, ..
        } => {
            if props.discriminator_test.is_some() {
                Some(props)
            } else if let Some(&first) = children.first() {
                choice_branch_element_props(program, first)
            } else {
                Some(props)
            }
        }
        IrNode::Choice { props, .. } => Some(props),
        _ => None,
    }
}

pub(crate) fn choice_branch_first_element(
    program: &IrProgram,
    node: u32,
) -> Option<(&IrProps, ValueKind)> {
    match &program.nodes[node as usize] {
        IrNode::Element { props, kind, .. } => Some((props, *kind)),
        IrNode::Sequence { children, .. } => {
            if let Some(&first) = children.first() {
                choice_branch_first_element(program, first)
            } else {
                None
            }
        }
        _ => None,
    }
}

impl<'a> Decoder<'a> {
    pub(crate) fn is_hidden_group_carrier(&self, children: &[u32]) -> bool {
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

    pub(crate) fn decode_particle(
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

    pub(crate) fn resolve_conditional_byte_order(&self, props: &IrProps) -> Result<IrProps> {
        let mut out = props.clone();
        let Some(test_id) = props.byte_order_conditional_test else {
            return Ok(out);
        };
        let test = self.ctx.strings().get(test_id)?;
        let pick_true = self
            .eval_particle_assert_expression(test, "", None)
            .unwrap_or(false);
        out.byte_order = if pick_true {
            props.byte_order_if_true
        } else {
            props.byte_order_if_false
        };
        out.byte_order_defined = true;
        Ok(out)
    }

    pub(crate) fn push_decoded_array_item(
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
}

/// Parses a string representation into a simple `DfdlValue` according to `ValueKind`.
/// Used when evaluating `dfdl:checkConstraints(.)` expressions during choice discriminator matching.
pub(crate) fn parse_simple_val_from_str(text: &str, kind: ValueKind) -> core::result::Result<DfdlValue, VmError> {
    match kind {
        ValueKind::String => Ok(DfdlValue::string(text)),
        ValueKind::Boolean => text
            .trim()
            .parse::<bool>()
            .map(DfdlValue::Boolean)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid boolean".into(),
            }),
        ValueKind::Int => text
            .trim()
            .parse::<i32>()
            .map(DfdlValue::Int)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid int".into(),
            }),
        ValueKind::Long => text
            .trim()
            .parse::<i64>()
            .map(DfdlValue::Long)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid long".into(),
            }),
        ValueKind::Short => text
            .trim()
            .parse::<i16>()
            .map(DfdlValue::Short)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid short".into(),
            }),
        ValueKind::Byte => text
            .trim()
            .parse::<i8>()
            .map(DfdlValue::Byte)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid byte".into(),
            }),
        ValueKind::UnsignedInt => text
            .trim()
            .parse::<u32>()
            .map(DfdlValue::UnsignedInt)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid uint".into(),
            }),
        ValueKind::UnsignedShort => text
            .trim()
            .parse::<u16>()
            .map(DfdlValue::UnsignedShort)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid ushort".into(),
            }),
        ValueKind::UnsignedByte => text
            .trim()
            .parse::<u8>()
            .map(DfdlValue::UnsignedByte)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid ubyte".into(),
            }),
        ValueKind::Integer => Ok(DfdlValue::Integer(text.trim().to_string())),
        ValueKind::Decimal => Ok(DfdlValue::Decimal(text.trim().to_string())),
        ValueKind::Float => text
            .trim()
            .parse::<f32>()
            .map(DfdlValue::Float)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid float".into(),
            }),
        ValueKind::Double => text
            .trim()
            .parse::<f64>()
            .map(DfdlValue::Double)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid double".into(),
            }),
        ValueKind::HexBinary => crate::vm::runtime::decode_hex(text.trim())
            .map(DfdlValue::HexBinary)
            .map_err(|_| VmError::InvalidValue {
                message: "invalid hexBinary".into(),
            }),
        _ => Ok(DfdlValue::string(text)),
    }
}

impl<'a> Decoder<'a> {

    pub(crate) fn choice_branch_discriminator_matches(&self, branch_node: u32, dot: &str) -> bool {
        let Some(props) = choice_branch_element_props(self.ctx.program, branch_node) else {
            return true;
        };
        let Some(id) = props.discriminator_test else {
            return true;
        };
        let Ok(expr) = self.ctx.strings().get(id) else {
            return false;
        };
        if expr.contains("checkConstraints") {
            let Some((branch_props, kind)) = choice_branch_first_element(self.ctx.program, branch_node) else {
                return true;
            };
            if let Ok(val) = parse_simple_val_from_str(dot, kind) {
                return crate::vm::facet_validate::validate_decoded_facets(
                    &val,
                    kind,
                    branch_props,
                    self.ctx.strings(),
                    &self.ctx.program.tunables,
                )
                .is_ok();
            }
        }
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

    pub(crate) fn validate_particle_discriminator(
        &self,
        props: &IrProps,
        kind: ValueKind,
        dot: &str,
        cursor: Option<&Cursor<'_>>,
    ) -> Result<()> {
        let Some(id) = props.discriminator_test else {
            return Ok(());
        };
        let expr = self.ctx.strings().get(id)?;
        if expr.contains("checkConstraints") {
            if let Ok(val) = parse_simple_val_from_str(dot, kind) {
                if crate::vm::facet_validate::validate_decoded_facets(
                    &val,
                    kind,
                    props,
                    self.ctx.strings(),
                    &self.ctx.program.tunables,
                )
                .is_ok()
                {
                    self.discriminator_committed_branch.set(true);
                    return Ok(());
                }
            }
            let msg = self.eval_facet_assert_message(props)?;
            let reason = if !msg.is_empty() {
                msg
            } else {
                "Discriminator failed for dfdl:checkConstraints(.)".to_string()
            };
            return Err(VmError::InvalidValue {
                message: alloc::format!("Parse Error. Assertion failed: {reason}"),
            }
            .into());
        }
        if self.eval_particle_assert_expression(expr, dot, cursor)? {
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

    pub(crate) fn decode_element_occurrences(
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
        let _occurrence_depth = OccurrenceDecodeDepthGuard(&self.occurrence_decode_depth);

        let orig_occurs_min = props.occurs_min;
        let mut min = props.occurs_min;
        let mut max = props.occurs_max.unwrap_or(u64::MAX);
        let mut occurs_from_expression = false;
        if props.occurs_count_kind == OccursCountKind::Expression {
            if let Some(expr_id) = props.occurs_count_expr {
                let expr = self.ctx.strings().get(expr_id)?;
                if let Ok(n) = self.eval_occurs_count_xpath_expr(expr, siblings) {
                    min = n;
                    max = n;
                    occurs_from_expression = true;
                }
            }
            if !occurs_from_expression {
                if let Some(steps) = props.occurs_count_fn_path.as_ref() {
                    let mut sib_snap = self.xpath_siblings_snapshot();
                    if let Some(s) = siblings {
                        for (k, v) in s {
                            sib_snap.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                    if let Ok(n) = eval_occurs_count_expression(
                        steps,
                        Some(&sib_snap),
                        self.ctx.strings(),
                        &self.ctx.program.tunables,
                    ) {
                        min = n;
                        max = n;
                        occurs_from_expression = true;
                    }
                }
            }
        }
        if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
            if !occurs_from_expression {
                min = 0;
            }
            if crate::ir::ir_props_has_input_value_calc(props) {
                let v = self.decode_single_element(
                    node_id,
                    cursor,
                    has_following_sibling,
                    parent_sequence,
                    siblings,
                    content_scope_bytes,
                    pattern_text_frame,
                    stop_sequences,
                    false,
                )?;
                return Ok(v);
            }
            let required_complex_shell = matches!(
                self.ctx.program.node(node_id),
                Ok(IrNode::Element {
                    child: Some(_),
                    kind: ValueKind::Complex,
                    ..
                })
            ) && orig_occurs_min > 0;
            if cursor.is_empty() && min == 0 && !required_complex_shell {
                return Ok(DfdlValue::Array(Vec::new()));
            }
        }
        if props.occurs_count_kind == OccursCountKind::Parsed && props.occurs_max != Some(1) {
            if props.occurs_max == Some(0) && min == 0 {
                max = u64::MAX;
            } else if max != u64::MAX && min == max {
                match self.framing_extra_occurrence(node_id, props, parent_sequence) {
                    FramingExtraOccurrences::One => max = max.saturating_add(1),
                    FramingExtraOccurrences::Double => max = max.saturating_mul(2),
                    FramingExtraOccurrences::None => {}
                }
            } else if max > 1 {
                max = u64::MAX;
            }
        }
        let populate_path = element_prefixed_name(self.ctx.program, node_id).ok();
        let populate_errors = should_populate_array_errors(props);
        let item_value_kind = element_kind(self.ctx.program, node_id)?;
        let mut items = Vec::new();
        let mut implicit_empty_probe = false;
        let never_optional_array = parent_sequence.and_then(|p| {
            if p.separator_suppression_policy
                == Some(crate::schema::SeparatorSuppressionPolicy::Never)
            {
                props.occurs_max.filter(|&m| m > 1)
            } else {
                None
            }
        });
        let mut never_infix_separators_consumed = 0u64;
        let delimiter_stops = filter_delimiter_stop_sequences(stop_sequences, self.ctx.strings())?;
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
            let delim_idx = items.len() as u64 + 1;
            self.delimiter_occurrence_stack.borrow_mut().push(delim_idx);
            struct DelimiterOccurrenceGuard<'a>(&'a RefCell<Vec<u64>>);
            impl Drop for DelimiterOccurrenceGuard<'_> {
                fn drop(&mut self) {
                    let _ = self.0.borrow_mut().pop();
                }
            }
            let _delim_occ_guard = DelimiterOccurrenceGuard(&self.delimiter_occurrence_stack);
            if props.length_kind == LengthKind::Explicit && props.length == Some(0) {
                let kind = element_kind(self.ctx.program, node_id)?;
                if !crate::ir::ir_props_has_input_value_calc(props) {
                    validate_explicit_decimal_before_decode(
                        kind,
                        props,
                        &self.ctx.program.tunables,
                        self.ctx.strings(),
                    )?;
                    if kind != ValueKind::Decimal
                        && binary_length_validation_applies(kind, props.binary_number_rep)
                    {
                        validate_data_length_vm(
                            kind,
                            0,
                            props.length_units,
                            props.binary_number_rep,
                        )?;
                    }
                }
                let more_array_occurrences = (items.len() as u64).saturating_add(1) < max;
                let parent_infix_consumed_by_occurrence_loop = more_array_occurrences
                    && parent_sequence.is_some_and(|p| {
                        p.separator.is_some() && p.separator_position == SeparatorPosition::Infix
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
                if parent_sequence
                    .is_some_and(|p| p.separator_position == SeparatorPosition::Postfix)
                {
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
                if max == u64::MAX && self.at_enclosing_terminator_stop(cursor, stop_sequences)? {
                    break;
                }
                let at_sep_stop =
                    self.at_enclosing_separator_stop(cursor, stop_sequences, parent_sequence)?;
                if at_sep_stop {
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
                && !parent_sequence
                    .is_some_and(|p| p.separator_position == SeparatorPosition::Postfix)
            {
                let sep_pos = cursor.pos;
                if let Err(e) = self.consume_occurrence_separator(
                    parent_sequence,
                    Some(props),
                    Some(items.as_slice()),
                    cursor,
                ) {
                    if populate_errors && (items.len() as u64) < min {
                        if let Some(path) = populate_path.as_deref() {
                            return Err(populate_failed_error(
                                path,
                                items.len() as u64 + 1,
                                &e.to_string(),
                            )
                            .into());
                        }
                    }
                    return Err(e);
                }
                if never_optional_array.is_some() && cursor.pos > sep_pos {
                    never_infix_separators_consumed += 1;
                }
            }
            let element_is_delimited = matches!(
                props.length_kind,
                LengthKind::Delimited | LengthKind::Implicit
            );
            let has_initiator = props.initiator.is_some();
            let at_empty_slot = element_is_delimited
                && !has_initiator
                && (would_read_empty_delimited_field(
                    cursor,
                    props,
                    self.ctx.strings(),
                    delimiter_stops.as_slice(),
                )? || cursor_at_parent_infix_separator(
                    cursor,
                    parent_sequence,
                    self.ctx.strings(),
                )?);
            if min == 0 && at_empty_slot {
                let optional_complex = matches!(
                    self.ctx.program.node(node_id),
                    Ok(IrNode::Element {
                        child: Some(_),
                        kind: ValueKind::Complex,
                        ..
                    })
                );
                if !optional_complex {
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
            if min > 0 && (items.len() as u64) >= min && max == u64::MAX {
                if let Some(parent) = parent_sequence.filter(|p| {
                    p.separator.is_some() && p.separator_position == SeparatorPosition::Infix
                }) {
                    if cursor.remaining() == 0 && at_empty_slot {
                        if let Some(sep_id) = parent.separator {
                            let pat = self.ctx.strings().get(sep_id)?;
                            let enc = encoding_name(parent, self.ctx.strings()).ok();
                            if Self::suffix_is_only_infix_separators(
                                cursor,
                                pat,
                                parent.ignore_case,
                                enc,
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
                    }
                }
            }
            let more_array_occurrences = (items.len() as u64).saturating_add(1) < max;
            let require_delimiter = has_following_sibling || more_array_occurrences;
            let parent_infix_consumed_by_occurrence_loop = more_array_occurrences
                && parent_sequence.is_some_and(|p| {
                    p.separator.is_some() && p.separator_position == SeparatorPosition::Infix
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
            let _infix_defer_guard = ParentInfixDeferGuard(
                &self.parent_infix_consumed_by_occurrence_loop,
                prev_infix_defer,
            );
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
                        && is_suppressible_empty_representation(&v, props, self.ctx.strings())?
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
                        if cursor.absolute_bit_index() == saved.absolute_bit_index() {
                            break;
                        }
                        continue;
                    }
                    if v.as_str().is_some_and(str::is_empty)
                        && (items.len() as u64) >= min
                        && min > 0
                        && max == u64::MAX
                    {
                        if let Some(parent) = parent_sequence.filter(|p| {
                            p.separator.is_some()
                                && p.separator_position == SeparatorPosition::Infix
                        }) {
                            if let Some(sep_id) = parent.separator {
                                let pat = self.ctx.strings().get(sep_id)?;
                                let enc = encoding_name(parent, self.ctx.strings()).ok();
                                let skip_leading_excess =
                                    items.iter().all(|i| i.as_str().is_some_and(str::is_empty));
                                let skip_trailing_excess = items
                                    .iter()
                                    .any(|i| i.as_str().is_some_and(|s| !s.is_empty()))
                                    && Self::suffix_is_only_infix_separators(
                                        cursor,
                                        pat,
                                        parent.ignore_case,
                                        enc,
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
                        }
                    }
                    self.push_decoded_array_item(&mut items, v, props)?;
                    if parent_sequence
                        .is_some_and(|p| p.separator_position == SeparatorPosition::Postfix)
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
                    }
                }
                Err(e) => {
                    if implicit_empty_probe && (items.len() as u64) < min {
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
                            return Err(element_parse_error(
                                node_id,
                                self.ctx.program,
                                self.ctx.strings(),
                                e,
                            )
                            .into());
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
                        let has_real_initiator = props
                            .initiator
                            .and_then(|id| self.ctx.strings().get(id).ok())
                            .is_some_and(|s| !s.is_empty());
                        if has_real_initiator
                            && cursor.pos > rewind.pos
                            && (items.len() as u64) < min
                        {
                            return Err(element_parse_error(
                                node_id,
                                self.ctx.program,
                                self.ctx.strings(),
                                e,
                            )
                            .into());
                        }
                        let err_msg = e.to_string();
                        if err_msg.contains("initiator mismatch") && !rewind.is_empty() {
                            if !items.is_empty()
                                && (items.len() as u64) >= min
                                && props.occurs_count_kind == OccursCountKind::Implicit
                            {
                                *cursor = before_occurrence_sep;
                                break;
                            }
                            return Err(element_parse_error(
                                node_id,
                                self.ctx.program,
                                self.ctx.strings(),
                                e,
                            )
                            .into());
                        }
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
                    return Err(element_parse_error(
                        node_id,
                        self.ctx.program,
                        self.ctx.strings(),
                        e,
                    )
                    .into());
                }
            }
        }

        if (items.len() as u64) < min {
            if populate_errors {
                if let Some(path) = populate_path.as_deref() {
                    let index = items.len() as u64 + 1;
                    return Err(populate_failed_error(
                        path,
                        index,
                        &alloc::format!("expected at least {min} occurrences, got {}", items.len()),
                    )
                    .into());
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
                        message:
                            "Parse Error: maxOccurs required with separatorSuppressionPolicy never"
                                .into(),
                    }
                    .into());
                }
            } else if n < m {
                return Err(VmError::InvalidValue {
                    message:
                        "Parse Error: maxOccurs required with separatorSuppressionPolicy never"
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

    pub(crate) fn initiated_child_occurrence_count(
        &self,
        child_id: u32,
        map: &BTreeMap<String, DfdlValue>,
    ) -> Result<usize> {
        let IrNode::Element { name, props, .. } = self.ctx.program.node(child_id)? else {
            return Ok(0);
        };
        let key = self.ctx.strings().get(*name)?;
        match map.get(key) {
            Some(DfdlValue::Array(v)) => Ok(v.len()),
            Some(DfdlValue::Null) if props.nillable => Ok(1),
            Some(DfdlValue::Null) => Ok(0),
            Some(_) => Ok(1),
            None => Ok(0),
        }
    }

    pub(crate) fn unordered_backtrack_multi_occurrence(&self, props: &IrProps) -> bool {
        props.occurs_max.map(|m| m > 1).unwrap_or(true)
    }

    pub(crate) fn unordered_backtrack_may_take_another(
        &self,
        props: &IrProps,
        current_count: usize,
    ) -> bool {
        let limit = props.occurs_max.unwrap_or(u64::MAX);
        (current_count as u64) < limit
    }

    pub(crate) fn backtrack_decoded_facets_ok(
        &self,
        value: &DfdlValue,
        kind: ValueKind,
        props: &IrProps,
    ) -> bool {
        if !needs_facet_validation(props) {
            return true;
        }
        if needs_choice_discriminator_facet_check(props)
            && validate_choice_discriminator_facets(value, kind, props, self.ctx.strings()).is_err()
        {
            return false;
        }
        validate_decoded_facets_tdml(
            value,
            kind,
            props,
            self.ctx.strings(),
            &self.ctx.program.tunables,
            false,
        )
        .is_ok()
    }
}
