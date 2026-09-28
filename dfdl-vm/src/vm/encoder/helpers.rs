use super::super::runtime::{
    encode_framing_delimiter_bytes, encoding_name, hex_binary_from_integer,
    resolve_encode_property_pattern, resolve_output_new_line_for_encode,
};
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProgram, IrProps, StringId, StringPool};
use crate::length_validate::validate_fill_byte_schema;
use crate::schema::{ByteOrder, ChoiceLengthKind, LengthKind, LengthUnits, Representation, SeparatorPosition};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn schema_context_field_name(local_name: &str) -> String {
    alloc::format!("ex:{local_name}")
}

pub(crate) fn encoded_bit_length(byte_len: usize, bit_count: u8) -> usize {
    byte_len.saturating_mul(8) + bit_count as usize
}

/// Bit length of a value for `dfdl:valueLength(..., 'bits')` (excludes unused high bits in the last byte).
pub(crate) fn encoded_value_length_bits(byte_len: usize, bit_count: u8) -> usize {
    if bit_count == 0 {
        byte_len.saturating_mul(8)
    } else if byte_len == 0 {
        bit_count as usize
    } else {
        byte_len.saturating_sub(1).saturating_mul(8) + bit_count as usize
    }
}

/// `dfdl:valueLength` for delimited simple types: encoded field value only (no initiator/terminator).
pub(crate) fn delimited_value_length_bits_from_encode(
    enc: &super::Encoder<'_>,
    props: &IrProps,
    buf: &[u8],
    bit_count: u8,
    encode_scope: Option<&BTreeMap<String, DfdlValue>>,
) -> Result<usize> {
    let strings = enc.ctx.strings();
    let encoding = encoding_name(props, strings)?;
    let output_nl =
        resolve_output_new_line_for_encode(props, encode_scope, strings)?.map(|s| s as String);
    let output_nl_ref = output_nl.as_deref();
    let mut total_bits = encoded_value_length_bits(buf.len(), bit_count);
    if let Some(id) = props.initiator {
        let raw = strings.get(id)?;
        let pat = resolve_encode_property_pattern(raw, encode_scope);
        if !pat.is_empty() {
            let bytes = encode_framing_delimiter_bytes(&pat, output_nl_ref, encoding, None);
            total_bits = total_bits.saturating_sub(encoded_bit_length(bytes.len(), 0));
        }
    }
    if let Some(id) = props.terminator {
        let raw = strings.get(id)?;
        let pat = resolve_encode_property_pattern(raw, encode_scope);
        if !pat.is_empty() {
            let bytes = encode_framing_delimiter_bytes(&pat, output_nl_ref, encoding, None);
            total_bits = total_bits.saturating_sub(encoded_bit_length(bytes.len(), 0));
        }
    }
    Ok(total_bits)
}

pub(crate) fn find_particle_by_local_name(
    enc: &super::Encoder<'_>,
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

pub(crate) fn root_sequence_children(enc: &super::Encoder<'_>) -> Result<Vec<u32>> {
    let root = enc.ctx.program.root;
    let IrNode::Element {
        child: Some(seq_id),
        ..
    } = enc.ctx.program.node(root)?
    else {
        return Ok(Vec::new());
    };
    match enc.ctx.program.node(*seq_id)? {
        IrNode::Sequence { children, .. } => Ok(children.clone()),
        _ => Ok(Vec::new()),
    }
}

pub(crate) fn apply_ovc_path_step_index(
    enc: &super::Encoder<'_>,
    step: &crate::ir::IrInputPathStep,
    local: &str,
    value: DfdlValue,
) -> Result<DfdlValue> {
    match value {
        DfdlValue::Array(items) => {
            if step.index_from_occurs {
                let (idx, _) = enc.array_occurrence_for_ovc.get().ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!(
                        "outputValueCalc path `{local}[dfdl:occursIndex()]` missing occurrence context"
                    ),
                })?;
                return items.get(idx.saturating_sub(1)).cloned().ok_or_else(|| {
                    VmError::InvalidValue {
                        message: alloc::format!(
                            "outputValueCalc path missing `{local}[dfdl:occursIndex()]`"
                        ),
                    }
                    .into()
                });
            }
            if let Some(n) = step.index {
                return items
                    .get((n as usize).saturating_sub(1))
                    .cloned()
                    .ok_or_else(|| {
                        VmError::InvalidValue {
                            message: alloc::format!("outputValueCalc path missing `{local}[{n}]`"),
                        }
                        .into()
                    });
            }
        }
        other if !step.index_from_occurs && step.index.is_none() => return Ok(other),
        _ => {}
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("outputValueCalc path invalid index on `{local}`"),
    }
    .into())
}

pub(crate) fn resolve_output_value_calc_path_value(
    enc: &super::Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
) -> Result<DfdlValue> {
    let strings = enc.ctx.strings();
    let mut steps = steps;
    while let Some(first) = steps.first() {
        let local = strings.get(first.local)?;
        if local == ".." || local == "." {
            steps = &steps[1..];
        } else {
            break;
        }
    }
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
        if let Some(v) = map.get(&k) {
            v.clone()
        } else {
            return Err(VmError::InvalidValue {
                message: "key not found".into(),
            }
            .into());
        }
    } else if let Some(cid) = find_particle_by_local_in_children(enc, scope, first_local)? {
        synthesize_element_subtree_value(enc, cid, map, scope)?
    } else if scope != sequence_children {
        if let Some(cid) = find_particle_by_local_in_children(enc, sequence_children, first_local)?
        {
            synthesize_element_subtree_value(enc, cid, map, sequence_children)?
        } else {
            resolve_path_step_value(enc, scope, map, first_local)?
        }
    } else {
        resolve_path_step_value(enc, scope, map, first_local)?
    };
    if first.index.is_some() || first.index_from_occurs {
        value = apply_ovc_path_step_index(enc, first, first_local, value)?;
    }
    if steps.len() == 1 {
        return Ok(value);
    }
    let root_scope = root_sequence_children(enc)?;
    let path_scope_vec =
        if let Some(b_id) = find_particle_by_local_in_children(enc, &root_scope, first_local)? {
            inner_sequence_children_of_element(enc, b_id).unwrap_or_else(|| scope.to_vec())
        } else {
            scope.to_vec()
        };
    let path_scope = path_scope_vec.as_slice();
    for step in steps.iter().skip(1) {
        let local = strings.get(step.local)?;
        value = match value {
            DfdlValue::Sequence(seq) => {
                let step_map = {
                    let mut merged = map.clone();
                    for (k, v) in &seq.fields {
                        merged.insert(k.clone(), v.clone());
                    }
                    merged
                };
                let child_val = if let Some(k) = map_has_local_key(&seq.fields, local) {
                    seq.fields.get(&k).cloned()
                } else if let Ok(v) = resolve_path_step_value(enc, path_scope, &step_map, local) {
                    Some(v)
                } else if let Ok(Some(cid)) =
                    find_particle_by_local_in_children(enc, path_scope, local)
                {
                    synthesize_element_subtree_value(enc, cid, &step_map, path_scope).ok()
                } else {
                    None
                };
                let child = child_val.as_ref().ok_or_else(|| VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc path missing `{local}`"),
                })?;
                if step.index.is_some() || step.index_from_occurs {
                    apply_ovc_path_step_index(enc, step, local, child.clone())?
                } else {
                    child.clone()
                }
            }
            _ => {
                return Err(VmError::InvalidValue {
                    message: "outputValueCalc path requires sequence".into(),
                }
                .into());
            }
        };
    }
    Ok(value)
}

pub(crate) fn eval_output_infoset_path(
    enc: &super::Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
) -> Result<i64> {
    let value = resolve_output_value_calc_path_value(enc, steps, sequence_children, map)?;
    numeric_from_dfdl_value(&value)
}

pub(crate) fn resolve_path_step_value(
    enc: &super::Encoder<'_>,
    sequence_children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
    local: &str,
) -> Result<DfdlValue> {
    if let Some(k) = map_has_local_key(map, local) {
        if let Some(v) = map.get(&k) {
            return Ok(v.clone());
        }
    }
    for &child_id in sequence_children {
        let IrNode::Element { name, props, .. } = enc.ctx.program.node(child_id)? else {
            continue;
        };
        let ename = enc.ctx.strings().get(*name)?;
        if crate::xml_util::local_name_str(ename) != crate::xml_util::local_name_str(local) {
            continue;
        }
        if props.output_value_calc.is_some() || props.output_value_calc_conditional {
            return super::ovc::eval_output_value_calc(enc, props, map, sequence_children, props);
        }
        if let IrNode::Element {
            child: Some(cid), ..
        } = enc.ctx.program.node(child_id)?
        {
            return synthesize_element_subtree_value(enc, *cid, map, sequence_children);
        }
    }
    Err(VmError::InvalidValue {
        message: alloc::format!("outputValueCalc path missing `{local}`"),
    }
    .into())
}

pub(crate) fn synthesize_element_subtree_value(
    enc: &super::Encoder<'_>,
    node_id: u32,
    map: &BTreeMap<String, DfdlValue>,
    sequence_children: &[u32],
) -> Result<DfdlValue> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Sequence {
            children,
            props: seq_props,
        } => {
            let mut effective = super::ovc::precompute_output_values(enc, children, map, seq_props)?;
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
            Ok(DfdlValue::Sequence(crate::value::SequenceValue::new(
                effective,
            )))
        }
        IrNode::Element {
            name,
            kind: _,
            props,
            child,
        } => {
            let key = enc.ctx.strings().get(*name)?;
            if let Some(k) = map_has_local_key(map, crate::xml_util::local_name_str(key)) {
                if let Some(v) = map.get(&k) {
                    return Ok(v.clone());
                }
            }
            if props.output_value_calc.is_some() || props.output_value_calc_conditional {
                return super::ovc::eval_output_value_calc(enc, props, map, sequence_children, props);
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


pub(crate) fn inner_sequence_children_of_element(enc: &super::Encoder<'_>, element_id: u32) -> Option<Vec<u32>> {
    let IrNode::Element { child: Some(c), .. } = enc.ctx.program.node(element_id).ok()? else {
        return None;
    };
    match enc.ctx.program.node(*c).ok()? {
        IrNode::Sequence { children, .. } => Some(children.clone()),
        _ => None,
    }
}

pub(crate) fn find_particle_by_local_in_children(
    enc: &super::Encoder<'_>,
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

pub(crate) fn find_particle_for_ovc_path(
    enc: &super::Encoder<'_>,
    scope_children: &[u32],
    steps: &[crate::ir::IrInputPathStep],
) -> Result<Option<u32>> {
    let strings = enc.ctx.strings();
    let first = steps.first().ok_or_else(|| VmError::InvalidValue {
        message: "empty outputValueCalc path".into(),
    })?;
    let first_local = strings.get(first.local)?;
    let mut current = match find_particle_by_local_in_children(enc, scope_children, first_local)? {
        Some(id) => id,
        None => return Ok(None),
    };
    for step in steps.iter().skip(1) {
        let local = strings.get(step.local)?;
        current = match find_particle_by_local_name(enc, current, local)? {
            Some(id) => id,
            None => return Ok(None),
        };
    }
    Ok(Some(current))
}

pub(crate) fn find_descendant_element_by_local(
    enc: &super::Encoder<'_>,
    node_id: u32,
    local_name: &str,
) -> Result<Option<u32>> {
    let sib_local = crate::xml_util::local_name_str(local_name);
    match enc.ctx.program.node(node_id)? {
        IrNode::Element { name, child, .. } => {
            let n = enc.ctx.strings().get(*name)?;
            let n_local = crate::xml_util::local_name_str(n);
            if n == local_name || n_local == local_name || n_local == sib_local {
                return Ok(Some(node_id));
            }
            if let Some(c) = child {
                if let Some(found) = find_descendant_element_by_local(enc, *c, local_name)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        IrNode::Sequence { children, .. } => {
            for &c in children {
                if let Some(found) = find_descendant_element_by_local(enc, c, local_name)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

pub(crate) fn find_child_element_by_name(
    enc: &super::Encoder<'_>,
    children: &[u32],
    local_name: &str,
) -> Result<Option<u32>> {
    for &child in children {
        if let Some(found) = find_descendant_element_by_local(enc, child, local_name)? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

pub(crate) fn value_length_encode_target(enc: &super::Encoder<'_>, node_id: u32) -> Result<u32> {
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

pub(crate) fn measure_value_length(
    enc: &super::Encoder<'_>,
    node_id: u32,
    value: &DfdlValue,
    units: LengthUnits,
    encode_scope: Option<&BTreeMap<String, DfdlValue>>,
) -> Result<usize> {
    if let Ok(IrNode::Element { props, .. }) = enc.ctx.program.node(node_id) {
        if props.object_kind == crate::schema::ObjectKind::Bytes {
            let len = blob_value_byte_len(value)?;
            return length_in_units(len, units);
        }
    }
    if let Ok(IrNode::Element {
        kind, child, props, ..
    }) = enc.ctx.program.node(node_id)
    {
        if child.is_none()
            && props.length_kind == LengthKind::Delimited
            && props.escape_scheme.is_none()
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
    if enc.encode_node(encode_id, value, &mut buf, &mut bit_count, encode_scope).is_err() {
        let bytes = value_byte_length(value)?;
        return match units {
            LengthUnits::Bits => Ok(bytes.saturating_mul(8)),
            LengthUnits::Bytes => Ok(bytes),
            LengthUnits::Characters => Ok(bytes),
        };
    }
    let value_bits = if let Ok(IrNode::Element {
        props, child: None, ..
    }) = enc.ctx.program.node(encode_id)
    {
        if props.length_kind == LengthKind::Delimited {
            delimited_value_length_bits_from_encode(enc, props, &buf, bit_count, encode_scope)?
        } else {
            encoded_value_length_bits(buf.len(), bit_count)
        }
    } else {
        encoded_value_length_bits(buf.len(), bit_count)
    };
    match units {
        LengthUnits::Bits => Ok(value_bits),
        LengthUnits::Bytes => Ok(value_bits.div_ceil(8)),
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "outputValueCalc character units".into(),
        }
        .into()),
    }
}

pub(crate) fn apply_output_value_calc_scale(props: &IrProps, len: i64) -> i64 {
    len.saturating_mul(props.output_value_calc_scale.unwrap_or(1))
}

pub(crate) fn split_top_level_commas_ovc(s: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut out = alloc::vec::Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim().to_string());
    out
}

pub(crate) fn ovc_error_unquote_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return Some(s[1..s.len() - 1].replace("''", "'"));
    }
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return Some(s[1..s.len() - 1].replace("\"\"", "\""));
    }
    None
}

pub(crate) fn dfdl_value_to_string_fragment(value: &DfdlValue) -> String {
    match value {
        DfdlValue::String(s) => s.text.clone(),
        DfdlValue::Int(n) => n.to_string(),
        DfdlValue::Long(n) => n.to_string(),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.clone(),
        other => alloc::format!("{other:?}"),
    }
}

pub(crate) fn xpath_cast_inner<'a>(expr: &'a str, cast: &str) -> Option<&'a str> {
    let expr = expr.trim();
    let prefix = alloc::format!("{cast}(");
    let rest = expr.strip_prefix(prefix.as_str())?;
    let mut depth = 0i32;
    for (i, ch) in rest.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(rest[..i].trim()),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

pub(crate) fn minimal_signed_byte_width(n: i64) -> usize {
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

pub(crate) fn dfdl_hex_binary_lexical_from_integer(n: i64) -> String {
    if n >= 0 {
        let s = alloc::format!("{n:x}");
        if s.len() % 2 == 1 {
            return alloc::format!("0{s}");
        }
        return s;
    }
    let raw = hex_binary_from_integer(n, Some(1));
    let s: String = raw.iter().map(|b| alloc::format!("{:02x}", b)).collect();
    let trimmed = s.trim_start_matches('0');
    if trimmed.is_empty() {
        return "00".into();
    }
    if trimmed.len() % 2 == 1 {
        alloc::format!("0{trimmed}")
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn parse_sibling_property_expr(raw: &str) -> Option<String> {
    crate::vm::runtime::parse_sibling_property_expr(raw)
}

pub(crate) fn lookup_sibling_string_in_encode_map(
    map: &BTreeMap<String, DfdlValue>,
    sibling: &str,
) -> Result<String> {
    let sib_val =
        lookup_sibling_value_in_map(map, sibling).ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("property sibling `{sibling}` not available"),
        })?;
    match sib_val {
        DfdlValue::String(s) => Ok(s.text.clone()),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => Ok(s.clone()),
        DfdlValue::Int(n) => Ok(n.to_string()),
        DfdlValue::Long(n) => Ok(n.to_string()),
        DfdlValue::Short(n) => Ok(n.to_string()),
        DfdlValue::Byte(n) => Ok(n.to_string()),
        DfdlValue::UnsignedInt(n) => Ok(n.to_string()),
        DfdlValue::UnsignedShort(n) => Ok(n.to_string()),
        DfdlValue::UnsignedByte(n) => Ok(n.to_string()),
        DfdlValue::Integer(s) => Ok(s.clone()),
        other => Ok(crate::vm::decoder::xpath::dfdl_value_to_string(other)),
    }
}

pub(crate) fn resolve_text_standard_props_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &StringPool,
) -> Result<IrProps> {
    let mut resolved = props.clone();
    if let Some(sib_id) = props.text_standard_decimal_separator_sibling {
        let local = strings.get(sib_id)?;
        resolved.resolved_text_standard_decimal_separator =
            Some(lookup_sibling_string_in_encode_map(map, local)?);
    }
    if let Some(sib_id) = props.text_standard_grouping_separator_sibling {
        let local = strings.get(sib_id)?;
        resolved.resolved_text_standard_grouping_separator =
            Some(lookup_sibling_string_in_encode_map(map, local)?);
    }
    if let Some(sib_id) = props.text_standard_exponent_rep_sibling {
        let local = strings.get(sib_id)?;
        resolved.resolved_text_standard_exponent_rep =
            Some(lookup_sibling_string_in_encode_map(map, local)?);
    }
    Ok(resolved)
}

pub(crate) fn resolve_byte_order_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &StringPool,
) -> Result<IrProps> {
    let Some(test_id) = props.byte_order_conditional_test else {
        return Ok(props.clone());
    };
    let raw = strings.get(test_id)?;
    let Some(sibling) = parse_sibling_property_expr(raw) else {
        return Ok(props.clone());
    };
    let text = lookup_sibling_string_in_encode_map(map, &sibling)?;
    let order = match text.as_str() {
        "bigEndian" => ByteOrder::BigEndian,
        "littleEndian" => ByteOrder::LittleEndian,
        other => {
            return Err(VmError::InvalidValue {
                message: alloc::format!("unknown byteOrder `{other}`"),
            }
            .into());
        }
    };
    let mut resolved = props.clone();
    resolved.byte_order = order;
    resolved.byte_order_defined = true;
    Ok(resolved)
}

pub(crate) fn resolve_encoding_for_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &StringPool,
) -> Result<IrProps> {
    let raw = strings.get(props.encoding)?;
    let Some(sibling) = parse_sibling_property_expr(raw) else {
        return Ok(props.clone());
    };
    let enc = lookup_sibling_string_in_encode_map(map, &sibling)?;
    let mut resolved = props.clone();
    resolved.encoding =
        lookup_encoding_string_id(strings, &enc).ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("resolved encoding `{enc}` not in string pool"),
        })?;
    Ok(resolved)
}

pub(crate) fn lookup_encoding_string_id(
    pool: &StringPool,
    enc: &str,
) -> Option<StringId> {
    if let Some(id) = pool.lookup(enc) {
        return Some(id);
    }
    if let Some(id) = pool
        .values
        .iter()
        .enumerate()
        .find(|(_, v)| v.eq_ignore_ascii_case(enc))
        .map(|(idx, _)| StringId(idx as u32))
    {
        return Some(id);
    }
    let upper = enc.to_ascii_uppercase();
    let canonical = match upper.as_str() {
        "US-ASCII" | "ASCII" | "ISO646-US" => "US-ASCII",
        "UTF8" => "UTF-8",
        "UTF16" | "UTF_16" => "UTF-16",
        "UTF32" | "UTF_32" => "UTF-32BE",
        "UTF-32" => "UTF-32BE",
        "ISO8859-1" | "ISO_8859-1" | "LATIN1" => "ISO-8859-1",
        _ => return None,
    };
    pool.lookup(canonical)
}

pub(crate) fn choice_explicit_frame_bytes_encode(
    props: &IrProps,
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
    let span = match props.length_units {
        LengthUnits::Bytes => n,
        LengthUnits::Characters => {
            let enc = encoding_name(props, strings)?;
            super::super::encoding::character_span_byte_length(n, enc)?
        }
        LengthUnits::Bits => n.saturating_add(7) / 8,
    };
    Ok(Some(span))
}

pub(crate) fn choice_explicit_pad_byte(props: &IrProps) -> u8 {
    if props.representation == Representation::Text {
        return b' ';
    }
    props.fill_byte
}

pub(crate) fn pad_choice_explicit_frame(
    enc: &super::Encoder<'_>,
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
        super::super::runtime::write_byte_aligned(out, bit_count, core::slice::from_ref(&pad))?;
        written += 1;
    }
    Ok(())
}

pub(crate) fn validate_fill_byte_for_encode(props: &IrProps, strings: &StringPool) -> Result<()> {
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

pub(crate) fn resolve_length_props_encode(
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    strings: &StringPool,
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
    let sib_val = map
        .get(sib_name)
        .or_else(|| {
            map.iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == sib_name)
                .map(|(_, v)| v)
        })
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("length sibling `{sib_name}` not available"),
        })?;
    let mut resolved = props.clone();
    let base_len = length_from_value(sib_val, props.length_sibling_cast_long)?;
    resolved.length = Some(apply_length_sibling_adjust_encode(
        base_len,
        props.length_sibling_adjust,
    )?);
    Ok(resolved)
}

pub(crate) fn apply_length_sibling_adjust_encode(len: u64, adjust: i64) -> Result<u64> {
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

pub(crate) fn merged_encode_lookup(
    outer: Option<&BTreeMap<String, DfdlValue>>,
    inner: &BTreeMap<String, DfdlValue>,
) -> BTreeMap<String, DfdlValue> {
    let mut merged = BTreeMap::new();
    if let Some(o) = outer {
        for (k, v) in o {
            merged.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in inner {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

pub(crate) fn lookup_sibling_value_in_map<'a>(
    map: &'a BTreeMap<String, DfdlValue>,
    local: &str,
) -> Option<&'a DfdlValue> {
    lookup_sibling_value_in_map_depth(map, local, 0)
}

pub(crate) fn lookup_sibling_value_in_map_depth<'a>(
    map: &'a BTreeMap<String, DfdlValue>,
    local: &str,
    depth: usize,
) -> Option<&'a DfdlValue> {
    if depth > 16 {
        return None;
    }
    if let Some(k) = map_has_local_key(map, local) {
        return map.get(&k);
    }
    for v in map.values() {
        if let DfdlValue::Sequence(seq) = v {
            if let Some(found) = lookup_sibling_value_in_map_depth(&seq.fields, local, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

pub(crate) fn sibling_from_map<'a>(
    id: Option<StringId>,
    map: &'a BTreeMap<String, DfdlValue>,
    strings: &StringPool,
) -> Result<&'a DfdlValue> {
    let id = id.ok_or_else(|| VmError::InvalidValue {
        message: "outputValueCalc sibling missing".into(),
    })?;
    let name = strings.get(id)?;
    let local = crate::xml_util::local_name_str(name);
    lookup_sibling_value_in_map(map, local)
        .or_else(|| lookup_sibling_value_in_map(map, name))
        .ok_or_else(|| VmError::InvalidValue {
            message: alloc::format!("outputValueCalc sibling `{name}` not available"),
        })
        .map_err(Into::into)
}

pub(crate) fn length_in_units(byte_len: usize, units: LengthUnits) -> Result<usize> {
    match units {
        LengthUnits::Bytes => Ok(byte_len),
        LengthUnits::Bits => Ok(byte_len.saturating_mul(8)),
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "outputValueCalc character units".into(),
        }
        .into()),
    }
}

pub(crate) fn blob_payload_bytes(value: &DfdlValue) -> Result<Vec<u8>> {
    match value {
        DfdlValue::Blob(b) => Ok(b.clone()),
        DfdlValue::String(s) => crate::tdml::resolve_blob_uri_to_bytes(&s.text)
            .map_err(|m| VmError::InvalidValue { message: m }.into()),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => crate::tdml::resolve_blob_uri_to_bytes(s)
            .map_err(|m| VmError::InvalidValue { message: m }.into()),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("valueLength on unsupported blob value `{other:?}`"),
        }
        .into()),
    }
}

pub(crate) fn blob_value_byte_len(value: &DfdlValue) -> Result<usize> {
    Ok(blob_payload_bytes(value)?.len())
}

pub(crate) fn value_byte_length(value: &DfdlValue) -> Result<usize> {
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

pub(crate) fn looks_like_blob_uri(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("file:") || t.contains("/blobs/") || t.ends_with(".bin")
}

pub(crate) fn negative_runtime_length_error(value: i64) -> VmError {
    VmError::InvalidValue {
        message: alloc::format!(
            "Runtime Schema Definition Error. dfdl:length expression result must be non-negative, but was: {value}"
        ),
    }
}

pub(crate) fn length_from_value(value: &DfdlValue, cast_long: bool) -> Result<u64> {
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

pub(crate) fn should_emit_separator(
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

pub(crate) fn needs_length_frame(props: &IrProps) -> bool {
    matches!(
        props.length_kind,
        LengthKind::Prefixed | LengthKind::Explicit | LengthKind::Fixed | LengthKind::Delimited
    )
}

pub(crate) fn element_payload_value<'a>(value: &'a DfdlValue, local_name: &str) -> &'a DfdlValue {
    if let Some(fields) = value.sequence_fields() {
        if let Some(val) = fields.get(local_name) {
            return val;
        }
    }
    value
}

pub(crate) fn unwrap_root_for_encode<'a>(
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

pub(crate) fn numeric_from_dfdl_value(value: &DfdlValue) -> Result<i64> {
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

pub(crate) fn map_has_local_key(map: &BTreeMap<String, DfdlValue>, local: &str) -> Option<String> {
    for key in map.keys() {
        if crate::xml_util::local_name_str(key) == local {
            return Some(key.clone());
        }
    }
    None
}

pub(crate) fn ir_props_encodable_without_infoset(props: &IrProps) -> bool {
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

pub(crate) fn infoset_particle_can_absent_enc(enc: &super::Encoder<'_>, node_id: u32) -> Result<bool> {
    match enc.ctx.program.node(node_id)? {
        IrNode::Element {
            props, child, kind, ..
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

pub(crate) fn ir_branch_encodable_without_infoset(enc: &super::Encoder<'_>, branch_node: u32) -> Result<bool> {
    match enc.ctx.program.node(branch_node)? {
        IrNode::Element {
            props, child, kind, ..
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

pub(crate) fn choice_branches_are_all_hidden(
    enc: &super::Encoder<'_>,
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

pub(crate) fn choice_branch_infoset_local_key_enc(
    enc: &super::Encoder<'_>,
    branch_node: u32,
) -> Result<Option<String>> {
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

pub(crate) fn choice_branch_data_key(
    enc: &super::Encoder<'_>,
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
