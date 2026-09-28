use super::helpers::*;
use super::Encoder;
use crate::error::{Result, VmError};
use crate::ir::{IrNode, IrProps};
use crate::schema::{OccursCountKind, OutputValueCalc};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) struct OvcPrecomputeEntry {
    pub(crate) node_id: u32,
    pub(crate) name_key: String,
    pub(crate) local: String,
    pub(crate) kind: crate::ir::ValueKind,
    pub(crate) props: IrProps,
}

pub(crate) fn collect_ovc_elements_in_sequence_subtree(
    enc: &Encoder<'_>,
    children: &[u32],
    out: &mut Vec<OvcPrecomputeEntry>,
) -> Result<()> {
    collect_ovc_elements_in_sequence_subtree_depth(enc, children, out, 0)
}

pub(crate) fn collect_ovc_elements_in_sequence_subtree_depth(
    enc: &Encoder<'_>,
    children: &[u32],
    out: &mut Vec<OvcPrecomputeEntry>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        return Ok(());
    }
    for &cid in children {
        match enc.ctx.program.node(cid)? {
            IrNode::Element {
                name,
                kind,
                props,
                child,
                ..
            } => {
                if props.output_value_calc_conditional && props.output_value_calc_literal.is_none()
                {
                    let elem = enc.ctx.strings().get(*name)?;
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "Unparse Error: Element `{elem}` does not have a value, due to a circular dependency"
                        ),
                    }
                    .into());
                }
                if props.output_value_calc.is_some() || props.output_value_calc_conditional {
                    let key = enc.ctx.strings().get(*name)?.to_string();
                    out.push(OvcPrecomputeEntry {
                        node_id: cid,
                        local: crate::xml_util::local_name_str(&key).to_string(),
                        name_key: key,
                        kind: *kind,
                        props: props.clone(),
                    });
                }
                if let Some(c) = child {
                    collect_ovc_elements_in_subtree_depth(enc, *c, out, depth + 1)?;
                }
            }
            IrNode::Sequence {
                children: nested, ..
            } => {
                collect_ovc_elements_in_sequence_subtree_depth(enc, nested, out, depth + 1)?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn collect_ovc_elements_in_subtree(
    enc: &Encoder<'_>,
    node_id: u32,
    out: &mut Vec<OvcPrecomputeEntry>,
) -> Result<()> {
    collect_ovc_elements_in_subtree_depth(enc, node_id, out, 0)
}

pub(crate) fn collect_ovc_elements_in_subtree_depth(
    enc: &Encoder<'_>,
    node_id: u32,
    out: &mut Vec<OvcPrecomputeEntry>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        return Ok(());
    }
    match enc.ctx.program.node(node_id)? {
        IrNode::Sequence { children, .. } => {
            collect_ovc_elements_in_sequence_subtree_depth(enc, children, out, depth + 1)
        }
        IrNode::Element { child: Some(c), .. } => {
            collect_ovc_elements_in_subtree_depth(enc, *c, out, depth + 1)
        }
        _ => Ok(()),
    }
}

pub(crate) fn count_dfdl_value_for_fn_count(value: &DfdlValue) -> i64 {
    match value {
        DfdlValue::Array(items) => items.len() as i64,
        DfdlValue::Null => 0,
        DfdlValue::Int(n) => *n as i64,
        DfdlValue::Long(n) => *n,
        _ => 1,
    }
}

pub(crate) fn eval_occurs_count_for_encode(
    enc: &Encoder<'_>,
    steps: &[crate::ir::IrInputPathStep],
    map: &BTreeMap<String, DfdlValue>,
    sequence_children: &[u32],
) -> Result<u64> {
    let value = resolve_output_value_calc_path_value(enc, steps, sequence_children, map)?;
    Ok(count_dfdl_value_for_fn_count(&value).max(0) as u64)
}

pub(crate) fn value_for_occurrence_encode(
    enc: &Encoder<'_>,
    props: &IrProps,
    value: DfdlValue,
    map: &BTreeMap<String, DfdlValue>,
    sequence_children: &[u32],
) -> Result<DfdlValue> {
    if props.occurs_count_kind != OccursCountKind::Expression {
        return Ok(value);
    }
    let Some(steps) = props.occurs_count_fn_path.as_ref() else {
        return Ok(value);
    };
    let n = eval_occurs_count_for_encode(enc, steps, map, sequence_children)? as usize;
    if n == 0 {
        return Ok(DfdlValue::Array(alloc::vec::Vec::new()));
    }
    match value {
        DfdlValue::Array(items) if items.len() > n => {
            Ok(DfdlValue::Array(items.into_iter().take(n).collect()))
        }
        other => Ok(other),
    }
}

pub(crate) fn ovc_deferred_to_encode_occurrence(props: &IrProps) -> bool {
    matches!(
        props.output_value_calc,
        Some(OutputValueCalc::OccursIndexPath { .. })
    )
}

pub(crate) fn ovc_length_cycle_error(enc: &Encoder<'_>, children: &[u32]) -> Result<()> {
    let strings = enc.ctx.strings();
    for &a in children {
        let IrNode::Element {
            name: name_a,
            props: props_a,
            ..
        } = enc.ctx.program.node(a)?
        else {
            continue;
        };
        let Some(ovc_sib) = props_a.output_value_calc_sibling else {
            continue;
        };
        let ovc_sib_name = strings.get(ovc_sib)?;
        for &b in children {
            if a == b {
                continue;
            }
            let IrNode::Element {
                name: name_b,
                props: props_b,
                ..
            } = enc.ctx.program.node(b)?
            else {
                continue;
            };
            let elem_b = strings.get(*name_b)?;
            if elem_b != ovc_sib_name {
                continue;
            }
            if let Some(len_sib) = props_b.length_sibling {
                let len_sib_name = strings.get(len_sib)?;
                let elem_a = strings.get(*name_a)?;
                if len_sib_name == elem_a {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!(
                            "Unparse Error: Element `{elem_a}` does not have a value, due to a circular dependency between `{elem_a}` and `{elem_b}`"
                        ),
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn precompute_output_values<'a>(
    enc: &Encoder<'a>,
    children: &[u32],
    map: &BTreeMap<String, DfdlValue>,
    parent_props: &IrProps,
) -> Result<BTreeMap<String, DfdlValue>> {
    let _ = ovc_length_cycle_error(enc, children);
    let mut ovc_entries = Vec::new();
    collect_ovc_elements_in_sequence_subtree(enc, children, &mut ovc_entries)?;
    let mut effective = map.clone();
    let max_passes = ovc_entries.len().saturating_mul(2).max(4);
    for _pass in 0..max_passes {
        let mut progress = false;
        for entry in &ovc_entries {
            if ovc_deferred_to_encode_occurrence(&entry.props) {
                continue;
            }
            let computed =
                match eval_output_value_calc(enc, &entry.props, &effective, children, parent_props)
                {
                    Ok(v) => v,
                    Err(e) => {
                        if matches!(
                            entry.props.output_value_calc,
                            Some(OutputValueCalc::FnError)
                        ) {
                            return Err(e);
                        }
                        continue;
                    }
                };
            let computed = ovc_value_for_element_kind(entry.kind, computed);
            let target_key = map_has_local_key(&effective, &entry.local)
                .unwrap_or_else(|| entry.name_key.clone());
            let prev = effective.get(&target_key);
            if prev == Some(&computed) {
                continue;
            }
            effective.insert(target_key, computed);
            progress = true;
        }
        if !progress {
            break;
        }
    }
    for entry in &ovc_entries {
        if ovc_deferred_to_encode_occurrence(&entry.props) {
            continue;
        }
        if map_has_local_key(&effective, &entry.local).is_none() {
            let elem = &entry.local;
            return Err(VmError::InvalidValue {
                message: alloc::format!(
                    "Unparse Error: Element `{elem}` does not have a value, due to a circular dependency"
                ),
            }
            .into());
        }
    }
    Ok(effective)
}

pub(crate) fn ovc_value_for_element_kind(kind: crate::ir::ValueKind, value: DfdlValue) -> DfdlValue {
    use crate::ir::ValueKind;
    match (kind, value) {
        (ValueKind::String, DfdlValue::Int(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Long(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::UnsignedLong(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::UnsignedInt(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Short(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::UnsignedShort(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Byte(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::UnsignedByte(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Float(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Double(n)) => DfdlValue::string(n.to_string()),
        (ValueKind::String, DfdlValue::Boolean(b)) => DfdlValue::string(b.to_string()),
        (ValueKind::String, DfdlValue::Integer(s)) => DfdlValue::string(s),
        (ValueKind::String, DfdlValue::Decimal(s)) => DfdlValue::string(s),
        (ValueKind::String, DfdlValue::DateTime(s)) => DfdlValue::string(s),
        (ValueKind::String, DfdlValue::HexBinary(b)) => DfdlValue::string(crate::vm::runtime::scalar::encode_hex(&b)),
        (ValueKind::Int, DfdlValue::Long(n)) => DfdlValue::Int(n as i32),
        (ValueKind::Int, DfdlValue::UnsignedInt(n)) => DfdlValue::Int(n as i32),
        (ValueKind::Int, DfdlValue::UnsignedLong(n)) => DfdlValue::Int(n as i32),
        (ValueKind::Long, DfdlValue::Int(n)) => DfdlValue::Long(n as i64),
        (ValueKind::Long, DfdlValue::UnsignedInt(n)) => DfdlValue::Long(n as i64),
        (ValueKind::Long, DfdlValue::UnsignedLong(n)) => DfdlValue::Long(n as i64),
        (ValueKind::UnsignedInt, DfdlValue::Int(n)) => DfdlValue::UnsignedInt(n as u32),
        (ValueKind::UnsignedInt, DfdlValue::Long(n)) => DfdlValue::UnsignedInt(n as u32),
        (ValueKind::Float, DfdlValue::Int(n)) => DfdlValue::Float(n as f32),
        (ValueKind::Float, DfdlValue::Long(n)) => DfdlValue::Float(n as f32),
        (ValueKind::Double, DfdlValue::Int(n)) => DfdlValue::Double(n as f64),
        (ValueKind::Double, DfdlValue::Long(n)) => DfdlValue::Double(n as f64),
        (ValueKind::Double, DfdlValue::String(s)) => {
            DfdlValue::Double(s.text.trim().parse().unwrap_or(0.0))
        }
        (ValueKind::Float, DfdlValue::String(s)) => {
            DfdlValue::Float(s.text.trim().parse().unwrap_or(0.0))
        }
        (ValueKind::UnsignedByte, DfdlValue::String(s)) => {
            DfdlValue::UnsignedByte(s.text.trim().parse().unwrap_or(0))
        }
        (ValueKind::DateTime | ValueKind::Time, DfdlValue::String(s)) => {
            let norm = crate::vm::calendar_binary::normalize_xs_date_lexical(&s.text)
                .unwrap_or_else(|_| s.text.clone());
            DfdlValue::DateTime(norm)
        }
        (_, v) => v,
    }
}

pub(crate) fn eval_fn_error_ovc(
    expr: &str,
    map: &BTreeMap<String, DfdlValue>,
    _strings: &crate::ir::StringPool,
) -> Result<DfdlValue> {
    let expr = expr.trim();
    let error_call = if let Some(rest) = expr.strip_prefix("fn:round-half-to-even(") {
        rest.strip_suffix(')').unwrap_or(expr).trim()
    } else {
        expr
    };
    if error_call == "fn:error()" {
        return Err(VmError::InvalidValue {
            message: "Unparse Error: xqt-errors#FOER0000".into(),
        }
        .into());
    }
    let args_body = error_call
        .strip_prefix("fn:error(")
        .and_then(|a| a.strip_suffix(')'))
        .ok_or_else(|| VmError::InvalidValue {
            message: "invalid fn:error in outputValueCalc".into(),
        })?;
    let mut parts = alloc::vec!["Unparse Error".to_string()];
    for arg in split_top_level_commas_ovc(args_body) {
        let arg = arg.trim();
        if let Some(lit) = ovc_error_unquote_literal(arg) {
            parts.push(lit);
        } else if let Some(path) = arg.strip_prefix("../") {
            let local = path.rsplit(':').next().unwrap_or(path).trim();
            let text = lookup_sibling_string_in_encode_map(map, local)?;
            parts.push(text);
        } else {
            return Err(VmError::InvalidValue {
                message: alloc::format!("unsupported fn:error argument `{arg}`"),
            }
            .into());
        }
    }
    Err(VmError::InvalidValue {
        message: parts.join(": "),
    }
    .into())
}

pub(crate) fn eval_ovc_concat_segments(
    enc: &Encoder<'_>,
    segments: &[crate::ir::IrInputValueCalcSegment],
    map: &BTreeMap<String, DfdlValue>,
    children: &[u32],
) -> Result<String> {
    use crate::ir::IrInputValueCalcSegment;
    let strings = enc.ctx.strings();
    let mut out = String::new();
    for seg in segments {
        match seg {
            IrInputValueCalcSegment::Sibling(id) => {
                let name = strings.get(*id)?;
                let local = crate::xml_util::local_name_str(name);
                let text = lookup_sibling_string_in_encode_map(map, local)?;
                out.push_str(&text);
            }
            IrInputValueCalcSegment::Literal(id) => out.push_str(strings.get(*id)?),
            IrInputValueCalcSegment::Substring {
                sibling,
                start,
                length,
            } => {
                let name = strings.get(*sibling)?;
                let local = crate::xml_util::local_name_str(name);
                let text = lookup_sibling_string_in_encode_map(map, local)?;
                let start0 = (*start as usize).saturating_sub(1);
                for ch in text.chars().skip(start0).take(*length as usize) {
                    out.push(ch);
                }
            }
            IrInputValueCalcSegment::InfosetPath(steps) => {
                let v = resolve_output_value_calc_path_value(enc, steps, children, map)?;
                out.push_str(&dfdl_value_to_string_fragment(&v));
            }
            IrInputValueCalcSegment::ValueLength { sibling, units } => {
                let sib_name = strings.get(*sibling)?;
                let sib = sibling_from_map(Some(*sibling), map, strings)?;
                let len =
                    if let Some(child_id) = find_child_element_by_name(enc, children, sib_name)? {
                        measure_value_length(enc, child_id, sib, *units, Some(map))?
                    } else {
                        length_in_units(value_byte_length(sib)?, *units)?
                    };
                out.push_str(&len.to_string());
            }
        }
    }
    Ok(out)
}

pub(crate) fn eval_xpath_string_value(
    enc: &Encoder<'_>,
    expr: &str,
    map: &BTreeMap<String, DfdlValue>,
    children: &[u32],
) -> Result<String> {
    let expr = expr.trim();
    if let Some(body) = expr
        .strip_prefix("fn:concat(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut out = String::new();
        for part in split_top_level_commas_ovc(body) {
            out.push_str(&eval_xpath_string_value(enc, part.trim(), map, children)?);
        }
        return Ok(out);
    }
    if let Some(body) = expr
        .strip_prefix("fn:substring-before(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts = split_top_level_commas_ovc(body);
        let a = eval_xpath_string_value(
            enc,
            parts.first().ok_or_else(|| VmError::InvalidValue {
                message: "fn:substring-before missing arguments".into(),
            })?,
            map,
            children,
        )?;
        let b = eval_xpath_string_value(
            enc,
            parts.get(1).ok_or_else(|| VmError::InvalidValue {
                message: "fn:substring-before missing second argument".into(),
            })?,
            map,
            children,
        )?;
        if let Some(idx) = a.find(&b) {
            return Ok(a[..idx].to_string());
        }
        return Ok(a);
    }
    if let Some(body) = expr
        .strip_prefix("fn:substring-after(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts = split_top_level_commas_ovc(body);
        let a = eval_xpath_string_value(
            enc,
            parts.first().ok_or_else(|| VmError::InvalidValue {
                message: "fn:substring-after missing arguments".into(),
            })?,
            map,
            children,
        )?;
        let b = eval_xpath_string_value(
            enc,
            parts.get(1).ok_or_else(|| VmError::InvalidValue {
                message: "fn:substring-after missing second argument".into(),
            })?,
            map,
            children,
        )?;
        if let Some(idx) = a.find(&b) {
            return Ok(a[idx + b.len()..].to_string());
        }
        return Ok(String::new());
    }
    if let Some(body) = expr
        .strip_prefix("fn:substring(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts = split_top_level_commas_ovc(body);
        let src = eval_xpath_string_value(
            enc,
            parts.first().ok_or_else(|| VmError::InvalidValue {
                message: "fn:substring missing source argument".into(),
            })?,
            map,
            children,
        )?;
        let start_str = parts.get(1).ok_or_else(|| VmError::InvalidValue {
            message: "fn:substring missing start argument".into(),
        })?;
        let start_num: usize = start_str.trim().parse().unwrap_or(1);
        let start0 = start_num.saturating_sub(1);
        if let Some(len_str) = parts.get(2) {
            let len_num: usize = len_str.trim().parse().unwrap_or(src.len());
            let res: String = src.chars().skip(start0).take(len_num).collect();
            return Ok(res);
        } else {
            let res: String = src.chars().skip(start0).collect();
            return Ok(res);
        }
    }
    if let Some(path) = expr.strip_prefix("../") {
        let local = path.rsplit(':').next().unwrap_or(path).trim();
        return lookup_sibling_string_in_encode_map(map, local);
    }
    if let Some(lit) = ovc_error_unquote_literal(expr) {
        return Ok(lit);
    }
    let _ = (enc, children);
    Err(VmError::InvalidValue {
        message: alloc::format!("unsupported xpath outputValueCalc `{expr}`"),
    }
    .into())
}

pub(crate) fn eval_xpath_output_value_calc(
    enc: &Encoder<'_>,
    expr: &str,
    map: &BTreeMap<String, DfdlValue>,
    children: &[u32],
) -> Result<DfdlValue> {
    let expr = expr.trim();
    if let Some(arg) = xpath_cast_inner(expr, "xs:date") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let norm = crate::vm::calendar_binary::normalize_xs_date_lexical(&s).unwrap_or(s);
        return Ok(DfdlValue::DateTime(norm));
    }
    if let Some(arg) =
        xpath_cast_inner(expr, "xs:dateTime").or_else(|| xpath_cast_inner(expr, "xs:time"))
    {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        return Ok(DfdlValue::DateTime(s));
    }
    if let Some(arg) =
        xpath_cast_inner(expr, "xs:integer").or_else(|| xpath_cast_inner(expr, "xs:long"))
    {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: i64 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:integer `{s}`"),
        })?;
        return Ok(DfdlValue::Long(n));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:short") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: i16 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:short `{s}`"),
        })?;
        return Ok(DfdlValue::Int(i32::from(n)));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:byte") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: i8 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:byte `{s}`"),
        })?;
        return Ok(DfdlValue::Int(i32::from(n)));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:unsignedInt") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: u32 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:unsignedInt `{s}`"),
        })?;
        return Ok(DfdlValue::Long(i64::from(n)));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:unsignedShort") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: u16 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:unsignedShort `{s}`"),
        })?;
        return Ok(DfdlValue::Int(i32::from(n)));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:unsignedLong") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: u64 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:unsignedLong `{s}`"),
        })?;
        return Ok(DfdlValue::Long(n as i64));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:unsignedByte") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: u8 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:unsignedByte `{s}`"),
        })?;
        return Ok(DfdlValue::UnsignedByte(n));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:decimal") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        return Ok(DfdlValue::Decimal(s));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:float") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: f32 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:float `{s}`"),
        })?;
        return Ok(DfdlValue::Float(n));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:double") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: f64 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:double `{s}`"),
        })?;
        return Ok(DfdlValue::Double(n));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:string") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        return Ok(DfdlValue::string(s));
    }
    if let Some(arg) = xpath_cast_inner(expr, "xs:int") {
        let s = eval_xpath_string_value(enc, arg, map, children)?;
        let n: i32 = s.trim().parse().map_err(|_| VmError::InvalidValue {
            message: alloc::format!("invalid xs:int `{s}`"),
        })?;
        return Ok(DfdlValue::Int(n));
    }
    eval_xpath_string_value(enc, expr, map, children).map(DfdlValue::string)
}

pub(crate) fn eval_output_value_calc(
    enc: &Encoder<'_>,
    props: &IrProps,
    map: &BTreeMap<String, DfdlValue>,
    children: &[u32],
    _parent_props: &IrProps,
) -> Result<DfdlValue> {
    use super::super::runtime::{
        decode_hex_binary, hex_binary_from_integer, int_bytes,
    };

    let strings = enc.ctx.strings();
    if props.output_value_calc_conditional {
        let lit_id = props
            .output_value_calc_literal
            .ok_or_else(|| VmError::InvalidValue {
                message: "missing xpath outputValueCalc".into(),
            })?;
        let expr = strings.get(lit_id)?;
        return eval_xpath_output_value_calc(enc, expr, map, children);
    }
    let calc = props
        .output_value_calc
        .ok_or_else(|| VmError::InvalidValue {
            message: "missing outputValueCalc".into(),
        })?;
    match calc {
        OutputValueCalc::FnError => {
            let lit_id = props
                .output_value_calc_literal
                .ok_or_else(|| VmError::InvalidValue {
                    message: "missing outputValueCalc fn:error".into(),
                })?;
            let expr = strings.get(lit_id)?;
            return eval_fn_error_ovc(expr, map, strings);
        }
        OutputValueCalc::FnConcat => {
            let segments =
                props
                    .output_value_calc_segments
                    .as_ref()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "missing outputValueCalc fn:concat segments".into(),
                    })?;
            let text = eval_ovc_concat_segments(enc, segments, map, children)?;
            return Ok(DfdlValue::string(text));
        }
        OutputValueCalc::HexBinaryFromLexical => {
            let lit_id = props
                .output_value_calc_literal
                .ok_or_else(|| VmError::InvalidValue {
                    message: "missing outputValueCalc hex literal".into(),
                })?;
            let text = strings.get(lit_id)?;
            let bytes = decode_hex_binary(text)?;
            return Ok(DfdlValue::HexBinary(bytes));
        }
        OutputValueCalc::HexBinaryFromInteger(n) => {
            let width = props.length.map(|l| l as usize);
            let bytes = if n >= 0 {
                decode_hex_binary(&dfdl_hex_binary_lexical_from_integer(n))?
            } else if let Some(w) = width {
                let min_w = minimal_signed_byte_width(n);
                if w == min_w {
                    hex_binary_from_integer(n, Some(w))
                } else {
                    decode_hex_binary(&dfdl_hex_binary_lexical_from_integer(n))?
                }
            } else {
                decode_hex_binary(&dfdl_hex_binary_lexical_from_integer(n))?
            };
            return Ok(DfdlValue::HexBinary(bytes));
        }
        OutputValueCalc::HexBinaryFromShort(v) => {
            return Ok(DfdlValue::HexBinary(int_bytes(i64::from(v), 2, false)));
        }
        OutputValueCalc::InfosetPathAddend => {
            let steps =
                props
                    .output_value_calc_path
                    .as_ref()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "missing outputValueCalc path".into(),
                    })?;
            let addend = props.output_value_calc_path_addend.unwrap_or(0);
            if addend == 0 {
                return resolve_output_value_calc_path_value(enc, steps, children, map);
            }
            let base = eval_output_infoset_path(enc, steps, children, map)?;
            let len = base.saturating_add(addend);
            return Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| {
                VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
                }
            })?));
        }
        OutputValueCalc::FnCountPath => {
            let steps =
                props
                    .output_value_calc_path
                    .as_ref()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "missing outputValueCalc fn:count path".into(),
                    })?;
            let value = resolve_output_value_calc_path_value(enc, steps, children, map)?;
            let n = count_dfdl_value_for_fn_count(&value);
            return Ok(DfdlValue::Int(i32::try_from(n).map_err(|_| {
                VmError::InvalidValue {
                    message: alloc::format!("fn:count result `{n}` out of range for int"),
                }
            })?));
        }
        OutputValueCalc::OccursIndexPath { multiply } => {
            let (idx, _) =
                enc.array_occurrence_for_ovc
                    .get()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "occursIndex outputValueCalc missing occurrence context".into(),
                    })?;
            let idx = idx as i64;
            let addend = props.output_value_calc_path_addend.unwrap_or(0);
            let steps = props.output_value_calc_path.as_deref().unwrap_or(&[]);
            let v = if steps.is_empty() {
                if multiply {
                    idx
                } else {
                    idx.saturating_add(addend)
                }
            } else {
                let path_val = eval_output_infoset_path(enc, steps, children, map)?;
                if multiply {
                    idx.saturating_mul(path_val)
                } else {
                    idx.saturating_add(path_val).saturating_add(addend)
                }
            };
            return Ok(DfdlValue::Int(i32::try_from(v).map_err(|_| {
                VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc result `{v}` out of range for int"),
                }
            })?));
        }
        OutputValueCalc::ValueLengthInfosetPath(units, addend) => {
            let steps =
                props
                    .output_value_calc_path
                    .as_ref()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "missing outputValueCalc path".into(),
                    })?;
            let val = resolve_output_value_calc_path_value(enc, steps, children, map)?;
            let last = steps.last().ok_or_else(|| VmError::InvalidValue {
                message: "missing outputValueCalc path".into(),
            })?;
            let child_id = find_particle_for_ovc_path(enc, children, steps)?.ok_or_else(|| {
                let last_local = strings.get(last.local).unwrap_or("?");
                VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc sibling `{last_local}` not available"),
                }
            })?;
            let len = apply_output_value_calc_scale(
                props,
                measure_value_length(enc, child_id, &val, units, Some(map))? as i64 + addend,
            );
            return Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| {
                VmError::InvalidValue {
                    message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
                }
            })?));
        }
        OutputValueCalc::HexBinaryFromByteSibling => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let byte = match sib {
                DfdlValue::Byte(v) => *v,
                DfdlValue::Int(v) => i8::try_from(*v).map_err(|_| VmError::InvalidValue {
                    message: alloc::format!("value `{v}` out of range for byte"),
                })?,
                DfdlValue::String(s) => {
                    s.text.parse::<i8>().map_err(|_| VmError::InvalidValue {
                        message: alloc::format!("invalid byte `{text}`", text = s.text),
                    })?
                }
                other => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("dfdl:hexBinary(xs:byte(...)) on `{other:?}`"),
                    }
                    .into());
                }
            };
            return Ok(DfdlValue::HexBinary(alloc::vec![byte as u8]));
        }
        _ => {}
    }

    let len = match calc {
        OutputValueCalc::Constant(v) => {
            if let Some(lit_id) = props.output_value_calc_literal {
                let text = strings.get(lit_id)?;
                return Ok(DfdlValue::string(text));
            }
            v
        }
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
        OutputValueCalc::StringLengthSibling => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let len = match sib {
                DfdlValue::String(s) => s.text.chars().count(),
                DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.chars().count(),
                other => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("string-length on unsupported value `{other:?}`"),
                    }
                    .into());
                }
            } as i64;
            len
        }
        OutputValueCalc::ValueLengthSibling(units, addend) => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let sib_name = strings.get(props.output_value_calc_sibling.ok_or_else(|| {
                VmError::InvalidValue {
                    message: "outputValueCalc sibling missing".into(),
                }
            })?)?;
            if let Some(child_id) = find_child_element_by_name(enc, children, sib_name)? {
                apply_output_value_calc_scale(
                    props,
                    measure_value_length(enc, child_id, sib, units, Some(map))? as i64 + addend,
                )
            } else {
                apply_output_value_calc_scale(
                    props,
                    length_in_units(value_byte_length(sib)?, units)? as i64 + addend,
                )
            }
        }
        OutputValueCalc::Substring { start, length } => {
            let sib = sibling_from_map(props.output_value_calc_sibling, map, strings)?;
            let text = match sib {
                DfdlValue::String(s) => s.text.as_str(),
                DfdlValue::Decimal(s) | DfdlValue::DateTime(s) => s.as_str(),
                other => {
                    return Err(VmError::InvalidValue {
                        message: alloc::format!("substring on unsupported value `{other:?}`"),
                    }
                    .into());
                }
            };
            let start0 = start.saturating_sub(1);
            let slice: String = text.chars().skip(start0).take(length).collect();
            return Ok(DfdlValue::string(slice));
        }
        OutputValueCalc::RepeatIndicatorFromParentCount => {
            let (idx, total) =
                enc.array_occurrence_for_ovc
                    .get()
                    .ok_or_else(|| VmError::InvalidValue {
                        message: "repeatIndicator outputValueCalc missing occurrence context"
                            .into(),
                    })?;
            let v = if idx < total { 1 } else { 0 };
            return Ok(DfdlValue::Int(v));
        }
        OutputValueCalc::HexBinaryFromLexical
        | OutputValueCalc::HexBinaryFromInteger(_)
        | OutputValueCalc::HexBinaryFromShort(_)
        | OutputValueCalc::HexBinaryFromByteSibling
        | OutputValueCalc::InfosetPathAddend
        | OutputValueCalc::OccursIndexPath { .. }
        | OutputValueCalc::FnCountPath
        | OutputValueCalc::FnConcat
        | OutputValueCalc::FnError
        | OutputValueCalc::ValueLengthInfosetPath(_, _) => {
            return Err(VmError::InvalidValue {
                message: "handled above".into(),
            }
            .into());
        }
    };
    Ok(DfdlValue::Int(i32::try_from(len).map_err(|_| {
        VmError::InvalidValue {
            message: alloc::format!("outputValueCalc result `{len}` out of range for int"),
        }
    })?))
}
