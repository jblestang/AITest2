//! Data structure helpers and utility functions for VM decoder.

use super::*;
use crate::error::{Error, Result, VmError};
use crate::ir::{ChoiceBranch, IrNode, IrProgram, IrProps, StringPool, ValueKind};
use crate::schema::{ChoiceLengthKind, LengthUnits, OccursCountKind, SeparatorPosition};
use crate::value::DfdlValue;
use crate::vm::facet_validate::validate_decoded_facets_tdml;
use crate::vm::runtime::encoding_name;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

pub(crate) fn is_element_absent(err: &Error) -> bool {
    matches!(err, Error::Vm(VmError::ElementAbsent))
}

pub(crate) fn is_schema_definition_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Vm(VmError::InvalidValue { message })
            if message.starts_with("Schema Definition Error")
                || message.starts_with("Runtime Schema Definition Error")
    )
}

/// Optional single-occurrence elements may treat initiator failures as absent; repeating or
/// `occursCountKind="parsed"` arrays must surface the error (e.g. e1a initiated-content choice).
pub(crate) fn optional_element_may_absorb_initiator_failure(props: &IrProps) -> bool {
    if props.occurs_count_kind == OccursCountKind::Parsed {
        return false;
    }
    !props.occurs_max.map(|m| m > 1).unwrap_or(true)
}

pub(crate) fn is_pattern_length_mismatch(err: &Error) -> bool {
    matches!(
        err,
        Error::Vm(VmError::InvalidValue { message })
            if message.starts_with("pattern `") && message.ends_with(" mismatch")
    )
}

pub(crate) fn should_populate_array_errors(props: &IrProps) -> bool {
    matches!(
        props.occurs_count_kind,
        OccursCountKind::Implicit | OccursCountKind::Fixed
    ) && (props.occurs_min != 1 || props.occurs_max != Some(1))
}

pub(crate) fn element_prefixed_name(program: &IrProgram, node_id: u32) -> Result<String> {
    match program.node(node_id)? {
        IrNode::Element { name, .. } => {
            let local = program.strings.get(*name)?;
            Ok(alloc::format!("ex:{local}"))
        }
        _ => Ok(alloc::string::String::from("ex:unknown")),
    }
}

pub(crate) fn populate_failed_error(qname: &str, index: u64, cause: &str) -> VmError {
    VmError::InvalidValue {
        message: if cause.is_empty() {
            alloc::format!("Parse Error: Failed to populate {qname}[{index}].")
        } else {
            alloc::format!("Parse Error: Failed to populate {qname}[{index}]. Cause: {cause}")
        },
    }
}

pub(crate) fn element_kind(
    program: &IrProgram,
    node_id: u32,
) -> core::result::Result<ValueKind, VmError> {
    match program.node(node_id)? {
        IrNode::Element { kind, .. } => Ok(*kind),
        _ => Ok(ValueKind::Complex),
    }
}

pub(crate) fn choice_explicit_frame_bytes(
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
            crate::vm::encoding::character_span_byte_length(n, enc)?
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

pub(crate) fn backtrack_decoded_facets_ok(
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

pub(crate) fn initiated_child_occurrence_count(
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

pub(crate) fn unordered_backtrack_multi_occurrence(props: &IrProps) -> bool {
    props.occurs_min > 1
        || props.occurs_max.map(|m| m > 1).unwrap_or(false)
        || (props.occurs_count_kind == OccursCountKind::Parsed
            && props.occurs_max.map(|m| m > 1).unwrap_or(false))
}

pub(crate) fn unordered_backtrack_may_take_another(
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

pub(crate) fn choice_value_fields(value: &DfdlValue) -> BTreeMap<String, DfdlValue> {
    match value {
        DfdlValue::Choice { value, .. } => choice_value_fields(value),
        DfdlValue::Sequence(seq) => seq.fields.clone(),
        _ => BTreeMap::new(),
    }
}

pub(crate) fn sequence_value_for_child(
    child_id: u32,
    fields: &BTreeMap<String, DfdlValue>,
    program: &IrProgram,
) -> Result<Option<DfdlValue>> {
    let mut expanded_fields = BTreeMap::new();
    for (k, v) in fields {
        match v {
            DfdlValue::Choice { .. } => {
                let choice_map = choice_value_fields(v);
                for (ck, cv) in choice_map {
                    expanded_fields.insert(ck, cv);
                }
            }
            other => {
                expanded_fields.insert(k.clone(), other.clone());
            }
        }
    }
    sequence_value_for_child_inner(child_id, &expanded_fields, program)
}

fn sequence_value_for_child_inner(
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
                if let Some(v) = sequence_value_for_child_inner(gc, fields, program)? {
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
                if let Some(v) = sequence_value_for_child_inner(branch.node, fields, program)? {
                    let disc = choice_branch_discriminator_for_infoset(
                        branch,
                        branches,
                        &program.strings,
                        program,
                    )?;
                    return Ok(Some(DfdlValue::choice(disc, v)));
                }
            }
            Ok(None)
        }
    }
}

pub(crate) fn insert_child(
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
        IrNode::Sequence { children, .. } => match value {
            DfdlValue::Sequence(seq) => {
                for &child_id in children {
                    if let Some(v) = sequence_value_for_child(child_id, &seq.fields, program)? {
                        insert_child(map, child_id, v, program)?;
                    }
                }
                Ok(())
            }
            DfdlValue::Choice { value: inner_v, .. } => {
                insert_child(map, node_id, *inner_v, program)
            }
            _ => Err(VmError::TypeMismatch {
                expected: "sequence".into(),
            }
            .into()),
        },
        IrNode::Choice { branches, .. } => {
            if let DfdlValue::Choice {
                discriminator,
                value: branch_value,
            } = value
            {
                if let Some(branch) = branches.iter().find(|b| {
                    choice_branch_discriminator_matches_name(program, b, discriminator.as_str())
                }) {
                    return insert_child(map, branch.node, *branch_value, program);
                }
                match *branch_value {
                    DfdlValue::Sequence(seq)
                        if discriminator == "sequence" || discriminator == "choice" =>
                    {
                        if let Some(branch) = branches.first() {
                            insert_child(map, branch.node, DfdlValue::Sequence(seq), program)?;
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
            } else if let DfdlValue::Sequence(seq) = value {
                for branch in branches {
                    if let Some(v) = sequence_value_for_child(branch.node, &seq.fields, program)? {
                        insert_child(map, branch.node, v, program)?;
                        return Ok(());
                    }
                }
                for (k, v) in seq.fields {
                    map.insert(k, v);
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

pub(crate) fn props_hidden_choice_branch_other(
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

pub(crate) fn branch_root_hidden(program: &IrProgram, node_id: u32) -> bool {
    match program.node(node_id) {
        Ok(IrNode::Element { props, .. }) => props.hidden,
        Ok(IrNode::Sequence { children, .. }) => {
            children.iter().all(|&c| branch_root_hidden(program, c))
        }
        Ok(IrNode::Choice { branches, .. }) => {
            branches.iter().all(|b| branch_root_hidden(program, b.node))
        }
        _ => false,
    }
}

pub(crate) fn should_write_separator(
    position: SeparatorPosition,
    index: usize,
    total: usize,
) -> bool {
    match position {
        SeparatorPosition::Prefix => index < total,
        SeparatorPosition::Infix => index > 0,
        SeparatorPosition::Postfix => index > 0 && index < total,
    }
}

pub(crate) fn insert_seq_sibling(
    map: &mut BTreeMap<String, SiblingState>,
    key: String,
    state: SiblingState,
) {
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

pub(crate) fn insert_field(map: &mut BTreeMap<String, DfdlValue>, key: String, value: DfdlValue) {
    if matches!(&value, DfdlValue::Array(items) if items.is_empty()) {
        return;
    }
    if let Some(existing) = map.remove(&key) {
        map.insert(key, append_value(existing, value));
    } else {
        map.insert(key, value);
    }
}

pub(crate) fn append_value(existing: DfdlValue, value: DfdlValue) -> DfdlValue {
    match existing {
        DfdlValue::Array(mut items) => {
            items.push(value);
            DfdlValue::Array(items)
        }
        other => DfdlValue::Array(alloc::vec![other, value]),
    }
}

pub(crate) fn wrap_root(name: &str, value: DfdlValue) -> DfdlValue {
    match value {
        DfdlValue::Sequence(seq) if seq.fields.contains_key(name) => DfdlValue::Sequence(seq),
        DfdlValue::Sequence(seq) => {
            let mut wrapped = BTreeMap::new();
            wrapped.insert(name.into(), DfdlValue::Sequence(seq));
            DfdlValue::sequence(wrapped)
        }
        DfdlValue::Choice {
            discriminator,
            value,
        } => {
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

pub(crate) fn choice_branch_fields(
    discriminator: String,
    value: DfdlValue,
) -> BTreeMap<String, DfdlValue> {
    match (discriminator.as_str(), value) {
        (
            "choice",
            DfdlValue::Choice {
                discriminator,
                value,
            },
        ) => choice_branch_fields(discriminator, *value),
        ("sequence", DfdlValue::Sequence(seq)) => seq.fields,
        (name, DfdlValue::Sequence(seq)) if seq.fields.contains_key(name) => seq.fields,
        (name, v) => {
            let mut map = BTreeMap::new();
            map.insert(name.to_string(), v);
            map
        }
    }
}

pub(crate) fn wrap_named(_name: &str, inner: DfdlValue, _kind: ValueKind) -> DfdlValue {
    inner
}

pub(crate) fn dfdl_value_text(value: &DfdlValue) -> &str {
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

pub(crate) fn element_parse_error(
    node_id: u32,
    program: &IrProgram,
    strings: &StringPool,
    err: impl core::fmt::Display,
) -> crate::error::VmError {
    let mut msg = err.to_string();
    while let Some(rest) = msg.strip_prefix("vm error: ") {
        msg = rest.to_string();
    }
    if let Ok(IrNode::Element { name, .. }) = program.node(node_id) {
        if let Ok(local) = strings.get(*name) {
            if !msg.ends_with(local) {
                msg = alloc::format!("{msg} {local}");
            }
        }
    }
    crate::error::VmError::InvalidValue { message: msg }
}

pub(crate) fn element_discriminator_always_false(props: &IrProps, strings: &StringPool) -> bool {
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
    inner == "fn:false()" || inner == "false()"
}

pub(crate) fn sequence_field_by_local<'a>(
    fields: &'a BTreeMap<String, DfdlValue>,
    local: &str,
) -> Option<&'a DfdlValue> {
    fields
        .iter()
        .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
        .map(|(_, v)| v)
}

pub(crate) fn sequence_field_by_local_value<'a>(
    value: &'a DfdlValue,
    local: &str,
) -> Option<&'a DfdlValue> {
    match value {
        DfdlValue::Sequence(seq) => sequence_field_by_local(&seq.fields, local),
        _ => None,
    }
}

pub(crate) fn sibling_string_value(
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

pub(crate) fn sibling_state_value_by_name<'a>(
    siblings: &'a BTreeMap<String, SiblingState>,
    name: &str,
) -> Option<&'a DfdlValue> {
    if let Some(state) = siblings.get(name) {
        return Some(&state.value);
    }
    let name_local = crate::xml_util::local_name_str(name);
    siblings
        .iter()
        .find(|(k, _)| crate::xml_util::local_name_str(k) == name_local)
        .map(|(_, state)| &state.value)
}

pub(crate) fn negative_runtime_length_error(value: i64) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Runtime Schema Definition Error. dfdl:length expression result must be non-negative, but was: {value}"
        ),
    }
}

pub(crate) fn apply_length_sibling_adjust(len: u64, adjust: i64) -> Result<u64> {
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

pub(crate) fn length_from_value(value: &DfdlValue, cast_long: bool) -> Result<u64> {
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
        other => err(alloc::format!(
            "length sibling has unsupported type: {other:?}"
        )),
    }
}

pub(crate) fn resolve_length_props(
    props: &IrProps,
    siblings: Option<&BTreeMap<String, SiblingState>>,
    kind: ValueKind,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<IrProps> {
    use crate::length_validate::{binary_length_validation_applies, validate_data_length_vm};
    use crate::schema::LengthKind;
    use crate::vm::runtime::validate_explicit_decimal_before_decode;

    let mut resolved = props.clone();

    if props.length_kind == LengthKind::Explicit && props.length.is_none() {
        if let Some(sib_id) = props.length_sibling {
            let sib_name = strings.get(sib_id)?;
            let sib_val = siblings
                .and_then(|m| sibling_state_value_by_name(m, sib_name))
                .ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("length sibling `{sib_name}` not available"),
                })?;
            let base_len = length_from_value(sib_val, props.length_sibling_cast_long)?;
            resolved.length = Some(apply_length_sibling_adjust(
                base_len,
                props.length_sibling_adjust,
            )?);
            if kind == ValueKind::Decimal {
                validate_explicit_decimal_before_decode(kind, &resolved, tunables, strings)?;
            } else if let Some(len) = resolved.length {
                if crate::length_validate::is_packed_binary_rep(resolved.binary_number_rep) {
                    let n_bits = match resolved.length_units {
                        LengthUnits::Bits => len as usize,
                        LengthUnits::Bytes => len.saturating_mul(8) as usize,
                        LengthUnits::Characters => len as usize,
                    };
                    crate::length_validate::validate_packed_binary_bit_length_parse(
                        n_bits,
                        kind,
                        resolved.binary_number_rep,
                    )?;
                } else if binary_length_validation_applies(kind, resolved.binary_number_rep) {
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
                message: "Schema Definition Error: Length of string must be exactly 1 character"
                    .into(),
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
    if resolved.text_standard_infinity_rep != crate::ir::StringId(0) {
        let raw = strings
            .get(resolved.text_standard_infinity_rep)
            .unwrap_or("")
            .to_string();
        distinct.push(("textStandardInfinityRep", raw));
    }
    if resolved.text_standard_nan_rep != crate::ir::StringId(0) {
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
        let refs: alloc::vec::Vec<(&str, &str)> =
            distinct.iter().map(|(n, s)| (*n, s.as_str())).collect();
        crate::schema::validate_text_standard_distinct_values(&refs).map_err(|msg| {
            VmError::InvalidValue {
                message: alloc::format!("Schema Definition Error: {msg}"),
            }
        })?;
    }

    Ok(resolved)
}
