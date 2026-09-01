use super::runtime::{
    consume_enclosing_delimiter, default_value_for, read_delimited_bytes, read_length_span,
    read_simple, read_until_separator, Cursor, RuntimeConfig, VmContext,
};
use crate::error::{Error, Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps, ValueKind};
use crate::schema::{match_delimiter, LengthKind, SeparatorPosition};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

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
        let value = self.decode_node(self.ctx.program.root, &mut cursor, false, None, None)?;
        self.consume_root_delimited_suffix(&mut cursor)?;
        if self.ctx.config.strict_eos && !cursor.is_empty() {
            return Err(VmError::TrailingData {
                remaining: cursor.remaining(),
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
        enclosing_terminator: Option<&str>,
    ) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id)? {
            IrNode::Sequence { children, props } => {
                self.consume_initiator(props, cursor)?;
                let mut map = BTreeMap::new();
                for (idx, &child) in children.iter().enumerate() {
                    let child_has_following = idx + 1 < children.len();
                    self.consume_separator(props, cursor, idx, children.len())?;
                    let saved = cursor.clone();
                    match self.decode_particle(
                        child,
                        cursor,
                        child_has_following,
                        Some(props),
                        enclosing_terminator,
                    ) {
                        Ok(child_value) => {
                            insert_child(&mut map, child, child_value, self.ctx.program)?;
                        }
                        Err(e) if is_element_absent(&e) => {
                            *cursor = saved;
                        }
                        Err(e) => return Err(e),
                    }
                }
                self.consume_terminator(props, cursor)?;
                Ok(DfdlValue::Sequence(map))
            }
            IrNode::Choice { branches, .. } => {
                for branch in branches {
                    let saved = cursor.clone();
                    if let Some(init_id) = branch.initiator {
                        let pat = self.ctx.strings().get(init_id)?;
                        if match_delimiter(&cursor.data[cursor.pos..], pat).is_none() {
                            continue;
                        }
                    }
                    if let Ok(value) = self.decode_node(
                        branch.node,
                        cursor,
                        has_following_sibling,
                        parent_sequence,
                        enclosing_terminator,
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
                enclosing_terminator,
            ),
        }
    }

    fn decode_particle(
        &self,
        node_id: u32,
        cursor: &mut Cursor<'_>,
        has_following_sibling: bool,
        parent_sequence: Option<&IrProps>,
        enclosing_terminator: Option<&str>,
    ) -> Result<DfdlValue> {
        match self.ctx.program.node(node_id)? {
            IrNode::Element { props, .. } => self.decode_element_occurrences(
                node_id,
                props,
                cursor,
                has_following_sibling,
                parent_sequence,
                enclosing_terminator,
            ),
            _ => self.decode_node(
                node_id,
                cursor,
                has_following_sibling,
                parent_sequence,
                enclosing_terminator,
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
        enclosing_terminator: Option<&str>,
    ) -> Result<DfdlValue> {
        let min = props.occurs_min;
        let max = props.occurs_max.unwrap_or(u64::MAX);
        let mut items = Vec::new();

        while (items.len() as u64) < max {
            if items.len() as u64 >= min && cursor.is_empty() {
                break;
            }
            if !items.is_empty() {
                if let Some(extra) = self.try_decode_consecutive_empty_fields(
                    parent_sequence,
                    cursor,
                    enclosing_terminator,
                )? {
                    items.extend(extra);
                    continue;
                }
                self.consume_occurrence_separator(parent_sequence, cursor)?;
            }
            let require_delimiter = has_following_sibling;
            let saved = cursor.clone();
            match self.decode_single_element(
                node_id,
                cursor,
                require_delimiter,
                parent_sequence,
                enclosing_terminator,
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
        enclosing_terminator: Option<&str>,
    ) -> Result<DfdlValue> {
        let parent_term = effective_stop_delimiter(
            parent_sequence,
            enclosing_terminator,
            self.ctx.strings(),
        )?;
        match self.ctx.program.node(node_id)? {
            IrNode::Element {
                name,
                kind,
                props,
                child,
            } => {
                let child_enclosing =
                    element_terminator_str(props, self.ctx.strings())?.or(enclosing_terminator);
                if let Some(child_id) = child {
                    self.consume_initiator(props, cursor)?;
                    if props.length_kind == LengthKind::Explicit {
                        let len = props.length.ok_or(VmError::InvalidValue {
                            message: "explicit complex missing length".into(),
                        })? as usize;
                        let bytes = read_length_span(cursor, len, props.length_units)?;
                        let mut sub = Cursor::new(&bytes);
                        let inner = self.decode_node(*child_id, &mut sub, false, None, None)?;
                        if !sub.is_empty() {
                            return Err(VmError::InvalidValue {
                                message: "unconsumed bytes in explicit-length complex element".into(),
                            }
                            .into());
                        }
                        return Ok(wrap_named(
                            self.ctx.strings().get(*name)?,
                            inner,
                            ValueKind::Complex,
                        ));
                    }
                    if props.length_kind == LengthKind::Delimited {
                        let bytes = read_delimited_bytes(
                            cursor,
                            props,
                            self.ctx.strings(),
                            require_delimiter,
                            parent_term,
                        )?;
                        consume_enclosing_delimiter(cursor, props, self.ctx.strings(), parent_term)?;
                        let mut sub = Cursor::new(&bytes);
                        let inner = self.decode_node(*child_id, &mut sub, false, None, None)?;
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
                    if props.length_kind == LengthKind::Implicit {
                        if let Some(term_id) = props.terminator {
                            let term = self.ctx.strings().get(term_id)?;
                            if !term.is_empty() {
                                let bytes = read_until_separator(cursor, term, false)?;
                                if match_delimiter(&cursor.data[cursor.pos..], term).is_some() {
                                    let _ = cursor.consume_delimiter(term);
                                }
                                let mut sub = Cursor::new(&bytes);
                                let inner =
                                    self.decode_node(*child_id, &mut sub, false, None, None)?;
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
                                    let bytes = read_until_separator(cursor, sep, false)?;
                                    if match_delimiter(&cursor.data[cursor.pos..], sep).is_some() {
                                        let _ = cursor.consume_delimiter(sep);
                                    }
                                    let mut sub = Cursor::new(&bytes);
                                    let inner = self.decode_node(
                                        *child_id,
                                        &mut sub,
                                        false,
                                        None,
                                        None,
                                    )?;
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
                        child_enclosing,
                    )?;
                    self.consume_terminator(props, cursor)?;
                    Ok(wrap_named(
                        self.ctx.strings().get(*name)?,
                        inner,
                        ValueKind::Complex,
                    ))
                } else {
                    read_simple(
                        cursor,
                        *kind,
                        props,
                        self.ctx.strings(),
                        require_delimiter,
                        parent_term,
                    )
                    .map_err(Into::into)
                }
            }
            _ => self.decode_node(node_id, cursor, false, None, None),
        }
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
        if match_delimiter(&cursor.data[cursor.pos..], pat).is_some() {
            if !cursor.consume_delimiter(pat) {
                return Err(VmError::InvalidValue {
                    message: "separator mismatch".into(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// In infix-separated unbounded lists, `,,` before end is two empty fields and
    /// `,,X` is one empty field then `X` (NMEA/CSV trailing-empty semantics).
    fn try_decode_consecutive_empty_fields(
        &self,
        parent_sequence: Option<&IrProps>,
        cursor: &mut Cursor<'_>,
        enclosing_terminator: Option<&str>,
    ) -> Result<Option<Vec<DfdlValue>>> {
        let Some(props) = parent_sequence else {
            return Ok(None);
        };
        if props.separator_position != SeparatorPosition::Infix {
            return Ok(None);
        }
        let Some(sep_id) = props.separator else {
            return Ok(None);
        };
        let sep = self.ctx.strings().get(sep_id)?;
        if sep.is_empty() {
            return Ok(None);
        }

        let start = cursor.pos;
        let mut count = 0usize;
        while let Some(n) = match_delimiter(&cursor.data[cursor.pos..], sep) {
            if n == 0 {
                break;
            }
            cursor.advance(n);
            count += 1;
        }
        if count <= 1 {
            cursor.pos = start;
            return Ok(None);
        }

        let stop = effective_stop_delimiter(
            parent_sequence,
            enclosing_terminator,
            self.ctx.strings(),
        )?;
        let followed_by_data =
            !cursor.is_empty() && !at_stop_delimiter(cursor, stop);
        let num_empties = if followed_by_data {
            count - 1
        } else {
            count
        };
        let mut out = Vec::with_capacity(num_empties);
        for _ in 0..num_empties {
            out.push(DfdlValue::String(String::new()));
        }
        Ok(Some(out))
    }

    fn consume_root_delimited_suffix(&self, cursor: &mut Cursor<'_>) -> Result<()> {
        let node = self.ctx.program.node(self.ctx.program.root)?;
        if let IrNode::Element { props, child, .. } = node {
            if child.is_none() && props.length_kind == LengthKind::Delimited && !cursor.is_empty() {
                consume_enclosing_delimiter(cursor, props, self.ctx.strings(), None)?;
            }
        }
        Ok(())
    }

    fn consume_initiator(&self, props: &IrProps, cursor: &mut Cursor<'_>) -> Result<()> {
        if let Some(id) = props.initiator {
            let pat = self.ctx.strings().get(id)?;
            if !pat.is_empty() && !cursor.consume_delimiter(pat) {
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
            if !cursor.consume_delimiter(pat) {
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
    ) -> Result<()> {
        if !should_write_separator(props.separator_position, index, total) {
            return Ok(());
        }
        if let Some(id) = props.separator {
            let pat = self.ctx.strings().get(id)?;
            if match_delimiter(&cursor.data[cursor.pos..], pat).is_some() {
                if !cursor.consume_delimiter(pat) {
                    return Err(VmError::InvalidValue {
                        message: "separator mismatch".into(),
                    }
                    .into());
                }
            }
        }
        Ok(())
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

fn parent_terminator_str<'a>(
    parent: Option<&'a IrProps>,
    strings: &'a crate::ir::StringPool,
) -> Result<Option<&'a str>> {
    let Some(props) = parent else {
        return Ok(None);
    };
    let Some(id) = props.terminator else {
        return Ok(None);
    };
    let pat = strings.get(id)?;
    if pat.is_empty() {
        Ok(None)
    } else {
        Ok(Some(pat))
    }
}

fn element_terminator_str<'a>(
    props: &'a IrProps,
    strings: &'a crate::ir::StringPool,
) -> Result<Option<&'a str>> {
    let Some(id) = props.terminator else {
        return Ok(None);
    };
    let pat = strings.get(id)?;
    if pat.is_empty() {
        Ok(None)
    } else {
        Ok(Some(pat))
    }
}

fn effective_stop_delimiter<'a>(
    parent_sequence: Option<&'a IrProps>,
    enclosing_terminator: Option<&'a str>,
    strings: &'a crate::ir::StringPool,
) -> Result<Option<&'a str>> {
    if let Some(term) = parent_terminator_str(parent_sequence, strings)? {
        return Ok(Some(term));
    }
    Ok(enclosing_terminator)
}

fn at_stop_delimiter(cursor: &Cursor<'_>, stop: Option<&str>) -> bool {
    stop.filter(|term| !term.is_empty())
        .and_then(|term| match_delimiter(&cursor.data[cursor.pos..], term))
        .map(|n| n > 0)
        .unwrap_or(false)
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
            if let DfdlValue::Sequence(fields) = value {
                for (k, v) in fields {
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
        DfdlValue::Sequence(map) if map.contains_key(name) => DfdlValue::Sequence(map),
        DfdlValue::Sequence(map) => {
            let mut wrapped = BTreeMap::new();
            wrapped.insert(name.into(), DfdlValue::Sequence(map));
            DfdlValue::Sequence(wrapped)
        }
        other => {
            let mut map = BTreeMap::new();
            map.insert(name.into(), other);
            DfdlValue::Sequence(map)
        }
    }
}

fn wrap_named(name: &str, inner: DfdlValue, kind: ValueKind) -> DfdlValue {
    if kind == ValueKind::Complex {
        match inner {
            DfdlValue::Sequence(map) => {
                if !map.contains_key(name) {
                    return DfdlValue::Sequence(map);
                }
                DfdlValue::Sequence(map)
            }
            other => {
                let mut map = BTreeMap::new();
                map.insert(name.into(), other);
                DfdlValue::Sequence(map)
            }
        }
    } else {
        inner
    }
}
