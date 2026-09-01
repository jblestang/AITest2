use super::runtime::{
    consume_alignment, consume_enclosing_delimiter, default_value_for, encoding_name,
    prefixed_payload_byte_length, read_delimited_bytes, read_length_span, read_prefixed_payload,
    read_simple, read_until_separator, validate_explicit_decimal_before_decode,
    would_read_empty_delimited_field, Cursor, RuntimeConfig, VmContext,
};
use crate::length_validate::validate_data_length_vm;
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps, ValueKind};
use crate::schema::{match_length_pattern, InputValueCalc, LengthKind, LengthUnits, Representation, SeparatorPosition};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
struct SiblingState {
    value: DfdlValue,
    content_bytes: usize,
}

/// DFDL decoder VM — executes compiled IR against an input byte stream.
pub struct Decoder<'a> {
    ctx: VmContext<'a>,
}

impl<'a> Decoder<'a> {
    pub fn new(program: &'a IrProgram) -> Self {
        Self::with_config(program, RuntimeConfig::default())
    }

    pub fn with_config(program: &'a IrProgram, config: RuntimeConfig) -> Self {
        Self {
            ctx: VmContext { program, config },
        }
    }

    /// Decode one logical value from `input`.
    pub fn decode(&self, input: &[u8]) -> Result<DfdlValue> {
        let mut cursor = Cursor::new(input);
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
        if self.ctx.config.strict_eos && cursor.bit_count == 0 && cursor.remaining() > 0 {
            return Err(VmError::TrailingData {
                remaining_bits: cursor.remaining() * 8,
            }
            .into());
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
                self.consume_initiator(props, cursor)?;
                let mut extended = stop_sequences.to_vec();
                extended.push(props);
                let child_stops = extended.as_slice();
                let mut map = BTreeMap::new();
                let mut seq_siblings = BTreeMap::new();
                let mut infix_sep_newline_prefix = Vec::new();
                for (idx, &child) in children.iter().enumerate() {
                    let child_has_following = self.following_sibling_consumes_input(children, idx);
                    self.consume_separator(
                        props,
                        cursor,
                        idx,
                        children.len(),
                        &mut infix_sep_newline_prefix,
                    )?;
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
                        }
                        Err(e) if is_element_absent(&e) => {
                            *cursor = saved;
                        }
                        Err(e) => return Err(e),
                    }
                }
                self.consume_terminator(props, cursor)?;
                Ok(DfdlValue::Sequence(crate::value::SequenceValue {
                    fields: map,
                    meta: crate::value::SequenceMeta {
                        infix_sep_newline_prefix,
                    },
                }))
            }
            IrNode::Choice { branches, props } => {
                for branch in branches {
                    let saved = cursor.clone();
                    if let Some(init_id) = branch.initiator {
                        let pat = self.ctx.strings().get(init_id)?;
                        if crate::schema::match_delimiter_opts(
                            &cursor.data[cursor.pos..],
                            pat,
                            props.ignore_case,
                        )
                        .is_none()
                        {
                            continue;
                        }
                    }
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
        let min = props.occurs_min;
        let max = props.occurs_max.unwrap_or(u64::MAX);
        let mut items = Vec::new();

        while (items.len() as u64) < max {
            if items.len() as u64 >= min && cursor.is_empty() {
                break;
            }
            if !items.is_empty() {
                if !self.element_consumes_enclosing_delimiter(node_id, props) {
                    self.consume_occurrence_separator(parent_sequence, cursor)?;
                } else if items.last().is_some_and(|v| matches!(v, DfdlValue::Null)) {
                    self.consume_occurrence_separator(parent_sequence, cursor)?;
                } else if self.should_skip_empty_complex_delimited_occurrence(
                    node_id,
                    props,
                    parent_sequence,
                    cursor,
                    stop_sequences,
                )? {
                    self.consume_occurrence_separator(parent_sequence, cursor)?;
                }
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
                Ok(v) => items.push(v),
                Err(e) => {
                    if (items.len() as u64) >= min {
                        *cursor = saved;
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
                    return Err(e);
                }
            }
        }

        if (items.len() as u64) < min {
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
                consume_alignment(cursor, &props)?;
                if let Some(child_id) = child {
                    self.consume_initiator(&props, cursor)?;
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
                        let bytes = read_delimited_bytes(
                            cursor,
                            &props,
                            self.ctx.strings(),
                            require_delimiter,
                            stop_sequences,
                        )?;
                        consume_enclosing_delimiter(cursor, &props, self.ctx.strings(), stop_sequences)?;
                        let mut sub = Cursor::new(&bytes);
                        let scope = bytes.len();
                        let inner = self.decode_node(*child_id, &mut sub, false, None, None, Some(scope), pattern_text_frame, &[])?;
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
                        if let Some(parent) = parent_sequence {
                            if let Some(sep_id) = parent.separator {
                                let sep = self.ctx.strings().get(sep_id)?;
                                if self.inner_sequence_separator(*child_id)?.as_deref() != Some(sep)
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
                    let inner = self.decode_node(
                        *child_id,
                        cursor,
                        false,
                        parent_sequence,
                        None,
                        content_scope_bytes,
                        pattern_text_frame,
                        stop_sequences,
                    )?;
                    self.consume_terminator(&props, cursor)?;
                    Ok(wrap_named(
                        self.ctx.strings().get(*name)?,
                        inner,
                        ValueKind::Complex,
                    ))
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
                    read_simple(
                        cursor,
                        *kind,
                        &props,
                        self.ctx.strings(),
                        require_delimiter,
                        stop_sequences,
                        Some(self.ctx.strings().get(*name)?),
                        &self.ctx.program.tunables,
                        false,
                    )
                    .map_err(Into::into)
                }
            }
            _ => self.decode_node(node_id, cursor, false, None, None, content_scope_bytes, pattern_text_frame, stop_sequences),
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
        self.is_delimited_element(node_id, props)
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
    ) -> Result<()> {
        if !should_write_separator(props.separator_position, index, total) {
            return Ok(());
        }
        if let Some(id) = props.separator {
            let pat = self.ctx.strings().get(id)?;
            if crate::schema::is_nl_comma_space_pattern(pat)
                && props.separator_position == SeparatorPosition::Infix
                && index > 0
            {
                if let Some((n, had_nl)) =
                    crate::schema::match_nl_comma_space_separator_with_flag(&cursor.data[cursor.pos..])
                {
                    cursor.advance(n);
                    infix_sep_newline_prefix.push(had_nl);
                    return Ok(());
                }
            }
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
        }
        Ok(())
    }

    fn following_sibling_consumes_input(&self, children: &[u32], idx: usize) -> bool {
        children[idx + 1..]
            .iter()
            .any(|&child| self.particle_consumes_input(child))
    }

    fn particle_consumes_input(&self, node_id: u32) -> bool {
        match self.ctx.program.node(node_id) {
            Ok(IrNode::Element { props, .. }) => props.input_value_calc.is_none(),
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
        IrNode::Element { name, .. } => {
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
                map.insert(discriminator, *value);
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
        SeparatorPosition::Postfix => index + 1 < total,
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
        match inner {
            DfdlValue::Sequence(seq) => {
                if !seq.fields.contains_key(name) {
                    return DfdlValue::Sequence(seq);
                }
                DfdlValue::Sequence(seq)
            }
            DfdlValue::Choice { discriminator, value } => {
                let mut map = BTreeMap::new();
                map.insert(discriminator, *value);
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
    if let InputValueCalc::Constant(v) = calc {
        return constant_input_value(kind, v);
    }
    let len = match calc {
        InputValueCalc::Constant(_) => unreachable!("handled above"),
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
        other => Err(VmError::InvalidValue {
            message: alloc::format!("inputValueCalc constant unsupported for `{other:?}`"),
        }),
    }
    .map_err(Into::into)
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

fn resolve_length_props(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    kind: ValueKind,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<IrProps> {
    if props.length_kind != LengthKind::Explicit || props.length.is_some() {
        return Ok(props.clone());
    }
    let Some(sib_id) = props.length_sibling else {
        return Ok(props.clone());
    };
    let sib_name = strings.get(sib_id)?;
    let sib_val = siblings
        .and_then(|m| m.get(sib_name))
        .map(|state| &state.value)
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("length sibling `{sib_name}` not available"),
        })?;
    let mut resolved = props.clone();
    resolved.length = Some(length_from_value(sib_val, props.length_sibling_cast_long)?);
    if kind == ValueKind::Decimal {
        validate_explicit_decimal_before_decode(kind, &resolved, tunables)?;
    } else if let Some(len) = resolved.length {
        validate_data_length_vm(kind, len, resolved.length_units)?;
    }
    Ok(resolved)
}

fn negative_runtime_length_error(value: i64) -> crate::error::VmError {
    use crate::error::VmError;
    VmError::InvalidValue {
        message: alloc::format!("Runtime Schema Definition Error. dfdl:length {value}"),
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
