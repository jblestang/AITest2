use super::*;
use crate::error::{Result, VmError};
use crate::ir::{IrProps, ValueKind};
use crate::schema::InputValueCalc;
use crate::schema::LengthUnits;
use crate::value::{DfdlValue, StringValue};
use crate::vm::decoder::SiblingState;
use crate::vm::runtime::Cursor;
use alloc::collections::BTreeMap;
use alloc::string::ToString;

#[derive(Copy, Clone)]
pub(crate) struct IvcEvalCtx<'a> {
    pub siblings: Option<&'a BTreeMap<alloc::string::String, SiblingState>>,
    pub ancestor_frames: Option<&'a [BTreeMap<alloc::string::String, SiblingState>]>,
    pub root_element: &'a str,
    pub define_variables: &'a BTreeMap<alloc::string::String, alloc::string::String>,
    pub runtime_variables: &'a BTreeMap<alloc::string::String, alloc::string::String>,
    pub element_name: Option<&'a str>,
}

pub(crate) fn length_in_units(byte_len: usize, units: LengthUnits) -> Result<usize> {
    match units {
        LengthUnits::Bytes => Ok(byte_len),
        LengthUnits::Bits => Ok(byte_len.saturating_mul(8)),
        LengthUnits::Characters => Err(VmError::UnsupportedOperation {
            op: "inputValueCalc character units".into(),
        }
        .into()),
    }
}

pub(crate) fn value_byte_length(value: &DfdlValue) -> Result<usize> {
    match value {
        DfdlValue::String(s) => Ok(s.text.len()),
        DfdlValue::Decimal(s) | DfdlValue::DateTime(s) | DfdlValue::Integer(s) => Ok(s.len()),
        DfdlValue::HexBinary(v) => Ok(v.len()),
        DfdlValue::Byte(_) | DfdlValue::UnsignedByte(_) | DfdlValue::Boolean(_) => Ok(1),
        DfdlValue::Short(_) | DfdlValue::UnsignedShort(_) => Ok(2),
        DfdlValue::Int(_) | DfdlValue::UnsignedInt(_) | DfdlValue::Float(_) => Ok(4),
        DfdlValue::Long(_) | DfdlValue::Double(_) => Ok(8),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("valueLength on unsupported value `{other:?}`"),
        }
        .into()),
    }
}

pub(crate) fn resolve_ivc_variable(
    name: &str,
    ctx: IvcEvalCtx<'_>,
) -> Result<alloc::string::String> {
    resolve_ivc_variable_depth(name, ctx, 0)
}

fn resolve_ivc_variable_depth(
    name: &str,
    ctx: IvcEvalCtx<'_>,
    depth: usize,
) -> Result<alloc::string::String> {
    if depth > 8 {
        return Err(VmError::InvalidValue {
            message: ivc_sde_message(
                ctx.element_name,
                alloc::format!("circular variable reference for `{name}`"),
            ),
        }
        .into());
    }
    let local = name.rsplit(':').next().unwrap_or(name);
    let raw_text = if let Some(text) = ctx.runtime_variables.get(name) {
        text.clone()
    } else if let Some(text) = ctx.runtime_variables.get(local) {
        text.clone()
    } else if let Some(text) = ctx.define_variables.get(name) {
        text.clone()
    } else if let Some(text) = ctx.define_variables.get(local) {
        text.clone()
    } else if name == "dfdl:encoding" || (local == "encoding" && name.starts_with("dfdl:")) {
        "UTF-8".to_string()
    } else if name == "dfdl:byteOrder" || (local == "byteOrder" && name.starts_with("dfdl:")) {
        "bigEndian".to_string()
    } else if name == "dfdl:binaryFloatRep"
        || (local == "binaryFloatRep" && name.starts_with("dfdl:"))
    {
        "ieee".to_string()
    } else if name == "dfdl:outputNewLine"
        || (local == "outputNewLine" && name.starts_with("dfdl:"))
    {
        "%LF;".to_string()
    } else {
        return Err(VmError::InvalidValue {
            message: ivc_sde_message(
                ctx.element_name,
                alloc::format!("variable `{name}` has no value. It was not set"),
            ),
        }
        .into());
    };
    let clean = raw_text.trim();
    if clean.starts_with('{') && clean.ends_with('}') {
        let inner = clean[1..clean.len() - 1].trim();
        if let Ok(num) = inner.parse::<i64>() {
            return Ok(num.to_string());
        }
        if let Ok(f) = inner.parse::<f64>() {
            return Ok(f.to_string());
        }
        if inner.starts_with('$') {
            let ref_name = inner[1..].trim();
            let local_ref = ref_name.rsplit(':').next().unwrap_or(ref_name);
            let local_name = name.rsplit(':').next().unwrap_or(name);
            if local_ref != local_name {
                return resolve_ivc_variable_depth(ref_name, ctx, depth + 1);
            }
        }
    }
    Ok(raw_text)
}

pub(crate) fn ivc_target_uses_float_math(kind: ValueKind) -> bool {
    matches!(kind, ValueKind::Float | ValueKind::Double)
}

pub(crate) fn parse_ivc_lexical_for_kind(
    text: &str,
    kind: ValueKind,
    props: &IrProps,
    strings: &crate::ir::StringPool,
) -> Result<DfdlValue> {
    if kind == ValueKind::String {
        return Ok(DfdlValue::String(StringValue::new(text.to_string())));
    }
    let trimmed = text.trim();
    if kind == ValueKind::Double {
        if let Ok(f) = crate::vm::runtime::parse_float(trimmed) {
            return Ok(DfdlValue::Double(f));
        }
    }
    if kind == ValueKind::Float {
        if let Ok(f) = crate::vm::runtime::parse_float(trimmed) {
            return Ok(DfdlValue::Float(f as f32));
        }
    }
    let mut lex_props = props.clone();
    lex_props.length_kind = crate::schema::LengthKind::Explicit;
    lex_props.length_units = crate::schema::LengthUnits::Bytes;
    lex_props.length = Some(trimmed.len() as u64);
    lex_props.custom_text_number_pattern = false;
    lex_props.text_number_pattern = None;
    lex_props.text_number_check_policy = crate::schema::BinaryNumberCheckPolicy::Lax;
    let mut sub = Cursor::new(trimmed.as_bytes());
    let target_name = match kind {
        ValueKind::Float => "float",
        ValueKind::Double => "double",
        ValueKind::Boolean => "boolean",
        ValueKind::DateTime => {
            if props.calendar_date_only {
                "date"
            } else {
                "dateTime"
            }
        }
        ValueKind::Time => "time",
        ValueKind::Int => "int",
        ValueKind::Integer => "integer",
        ValueKind::Long => "long",
        ValueKind::Short => "short",
        ValueKind::Byte => "byte",
        ValueKind::UnsignedInt => "unsignedInt",
        ValueKind::UnsignedShort => "unsignedShort",
        ValueKind::UnsignedByte => "unsignedByte",
        ValueKind::HexBinary => "hexBinary",
        ValueKind::Decimal => "decimal",
        _ => "string",
    };
    crate::vm::runtime::read_text_scalar(
        &mut sub,
        kind,
        &lex_props,
        strings,
        false,
        &[],
        None,
        None,
        &crate::length_validate::DaffodilTunables::default(),
        None,
    )
    .map_err(|_| VmError::InvalidValue {
        message: alloc::format!(
            "Parse Error. Hex character must be 0-9, a-f, or A-F. Failed to parse xs:{target_name} from text: {trimmed}"
        ),
    })
    .map_err(Into::into)
}

pub(crate) fn ivc_cast_kind_to_value_kind(cast: crate::ir::IrIvcXsCast) -> ValueKind {
    use crate::ir::{IrIvcXsCast, ValueKind};
    match cast {
        IrIvcXsCast::Byte => ValueKind::Byte,
        IrIvcXsCast::Short => ValueKind::Short,
        IrIvcXsCast::Int => ValueKind::Int,
        IrIvcXsCast::Long => ValueKind::Long,
        IrIvcXsCast::UnsignedByte => ValueKind::UnsignedByte,
        IrIvcXsCast::UnsignedShort => ValueKind::UnsignedShort,
        IrIvcXsCast::UnsignedInt => ValueKind::UnsignedInt,
        IrIvcXsCast::UnsignedLong => ValueKind::Long,
        IrIvcXsCast::Float => ValueKind::Float,
        IrIvcXsCast::Double => ValueKind::Double,
        IrIvcXsCast::String => ValueKind::String,
        IrIvcXsCast::HexBinary => ValueKind::HexBinary,
    }
}

pub(crate) fn eval_input_value_calc_expression(
    expr: &crate::ir::IrInputValueCalcExpression,
    ctx: IvcEvalCtx<'_>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    target_kind: ValueKind,
    target_props: &IrProps,
) -> Result<DfdlValue> {
    use crate::ir::IrInputValueCalcExpression;
    match expr {
        IrInputValueCalcExpression::Add(terms) => {
            if ivc_target_uses_float_math(target_kind) {
                let mut sum = 0.0f64;
                for term in terms {
                    sum += eval_input_value_calc_to_f64(
                        term,
                        ctx,
                        strings,
                        tunables,
                        target_kind,
                        target_props,
                    )?;
                }
                return ivc_f64_to_value(sum, target_kind);
            }
            let mut sum = 0i64;
            for term in terms {
                sum = sum.saturating_add(eval_input_value_calc_to_i64(
                    term,
                    ctx,
                    strings,
                    tunables,
                    target_kind,
                    target_props,
                )?);
            }
            Ok(DfdlValue::Integer(sum.to_string()))
        }
        IrInputValueCalcExpression::Sub(items) => {
            if items.len() != 2 {
                return Err(VmError::InvalidValue {
                    message:
                        "Schema Definition Error: expression evaluation error: invalid subtraction"
                            .into(),
                }
                .into());
            }
            if ivc_target_uses_float_math(target_kind) {
                let a = eval_input_value_calc_to_f64(
                    &items[0],
                    ctx,
                    strings,
                    tunables,
                    target_kind,
                    target_props,
                )?;
                let b = eval_input_value_calc_to_f64(
                    &items[1],
                    ctx,
                    strings,
                    tunables,
                    target_kind,
                    target_props,
                )?;
                return ivc_f64_to_value(a - b, target_kind);
            }
            let a = eval_input_value_calc_to_i64(
                &items[0],
                ctx,
                strings,
                tunables,
                target_kind,
                target_props,
            )?;
            let b = eval_input_value_calc_to_i64(
                &items[1],
                ctx,
                strings,
                tunables,
                target_kind,
                target_props,
            )?;
            Ok(DfdlValue::Integer((a - b).to_string()))
        }
        IrInputValueCalcExpression::Mul(terms) => {
            if ivc_target_uses_float_math(target_kind) {
                let mut product = 1.0f64;
                for term in terms {
                    product *= eval_input_value_calc_to_f64(
                        term,
                        ctx,
                        strings,
                        tunables,
                        target_kind,
                        target_props,
                    )?;
                }
                return ivc_f64_to_value(product, target_kind);
            }
            let mut product = 1i64;
            for term in terms {
                product = product.saturating_mul(eval_input_value_calc_to_i64(
                    term,
                    ctx,
                    strings,
                    tunables,
                    target_kind,
                    target_props,
                )?);
            }
            Ok(DfdlValue::Integer(product.to_string()))
        }
        IrInputValueCalcExpression::Div(left, right) => {
            if ivc_target_uses_float_math(target_kind) {
                let a = eval_input_value_calc_to_f64(
                    left,
                    ctx,
                    strings,
                    tunables,
                    target_kind,
                    target_props,
                )?;
                let b = eval_input_value_calc_to_f64(
                    right,
                    ctx,
                    strings,
                    tunables,
                    target_kind,
                    target_props,
                )?;
                if b == 0.0 {
                    return Err(VmError::InvalidValue {
                        message: "divide by zero".into(),
                    }
                    .into());
                }
                return ivc_f64_to_value(a / b, target_kind);
            }
            let a = eval_input_value_calc_to_i64(
                left,
                ctx,
                strings,
                tunables,
                target_kind,
                target_props,
            )?;
            let b = eval_input_value_calc_to_i64(
                right,
                ctx,
                strings,
                tunables,
                target_kind,
                target_props,
            )?;
            if b == 0 {
                return Err(VmError::InvalidValue {
                    message: "divide by zero".into(),
                }
                .into());
            }
            Ok(DfdlValue::Float((a / b) as f32))
        }
        IrInputValueCalcExpression::Path { parent_root, steps } => {
            eval_ivc_path_steps(*parent_root, steps, ctx, strings, tunables)
        }
        IrInputValueCalcExpression::StringOf(inner) => {
            let value = eval_input_value_calc_expression(
                inner,
                ctx,
                strings,
                tunables,
                ValueKind::String,
                target_props,
            )?;
            let text = dfdl_value_to_string(&value);
            Ok(DfdlValue::String(StringValue::new(text)))
        }
        IrInputValueCalcExpression::Literal(v) => constant_input_value(target_kind, *v),
        IrInputValueCalcExpression::LiteralLexical(id) => {
            let text = strings.get(*id)?;
            parse_ivc_lexical_for_kind(text, target_kind, target_props, strings)
        }
        IrInputValueCalcExpression::Cast { kind, inner } => {
            let cast_kind = ivc_cast_kind_to_value_kind(*kind);
            let value = eval_input_value_calc_expression(
                inner,
                ctx,
                strings,
                tunables,
                cast_kind,
                target_props,
            )?;
            if cast_kind == ValueKind::String {
                let text = dfdl_value_to_string(&value);
                return Ok(DfdlValue::String(StringValue::new(text)));
            }
            let text = dfdl_value_to_string(&value);
            let mut cast_props = target_props.clone();
            if *kind == crate::ir::IrIvcXsCast::UnsignedLong {
                cast_props.unsigned_integer = true;
            }
            parse_ivc_lexical_for_kind(&text, cast_kind, &cast_props, strings)
        }
        IrInputValueCalcExpression::Variable(id) => {
            let name = strings.get(*id)?;
            let text = resolve_ivc_variable(name, ctx)?;
            parse_ivc_lexical_for_kind(&text, target_kind, target_props, strings)
        }
        IrInputValueCalcExpression::Ceiling(inner) => {
            let value = eval_input_value_calc_to_f64(
                inner,
                ctx,
                strings,
                tunables,
                ValueKind::Double,
                target_props,
            )?;
            ivc_f64_to_value(value.ceil(), target_kind)
        }
        IrInputValueCalcExpression::ValueLength { sibling, units } => {
            let sib_name = strings.get(*sibling)?;
            let sib = ctx.siblings.and_then(|m| m.get(sib_name)).ok_or_else(|| {
                VmError::InvalidValue {
                    message: alloc::format!("sibling `{sib_name}` not found for valueLength"),
                }
            })?;
            let byte_len = value_byte_length(&sib.value)?;
            let len = length_in_units(byte_len, *units)?;
            i32::try_from(len)
                .map(DfdlValue::Int)
                .map_err(|_| VmError::InvalidValue {
                    message: alloc::format!("valueLength result `{len}` out of range for int"),
                })
                .map_err(Into::into)
        }
    }
}

pub(crate) fn ivc_f64_to_value(value: f64, kind: ValueKind) -> Result<DfdlValue> {
    match kind {
        ValueKind::Float => Ok(DfdlValue::Float(value as f32)),
        ValueKind::Double => Ok(DfdlValue::Double(value)),
        _ => Ok(DfdlValue::Integer(value.to_string())),
    }
}

pub(crate) fn eval_input_value_calc_to_i64(
    expr: &crate::ir::IrInputValueCalcExpression,
    ctx: IvcEvalCtx<'_>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    target_kind: ValueKind,
    target_props: &IrProps,
) -> Result<i64> {
    let value =
        eval_input_value_calc_expression(expr, ctx, strings, tunables, target_kind, target_props)?;
    value.as_i64().ok_or_else(|| {
        VmError::InvalidValue {
            message: "Schema Definition Error: expression evaluation error: non-numeric value"
                .to_string(),
        }
        .into()
    })
}

pub(crate) fn eval_input_value_calc_to_f64(
    expr: &crate::ir::IrInputValueCalcExpression,
    ctx: IvcEvalCtx<'_>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    target_kind: ValueKind,
    target_props: &IrProps,
) -> Result<f64> {
    let value =
        eval_input_value_calc_expression(expr, ctx, strings, tunables, target_kind, target_props)?;
    match value {
        DfdlValue::Float(v) => Ok(f64::from(v)),
        DfdlValue::Double(v) => Ok(v),
        DfdlValue::String(s) => crate::vm::runtime::parse_float(&s.text).map_err(|_| {
            VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: expression evaluation error: Cannot convert string '{}' to double",
                    s.text
                ),
            }
            .into()
        }),
        other => other.as_i64().map(|n| n as f64).ok_or_else(|| {
            VmError::InvalidValue {
                message: "Schema Definition Error: expression evaluation error: non-numeric value".to_string(),
            }
            .into()
        }),
    }
}

pub(crate) fn eval_ivc_path_steps(
    parent_root: bool,
    steps: &[crate::ir::IrInputPathStep],
    ctx: IvcEvalCtx<'_>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<DfdlValue> {
    let mut steps = steps;
    if parent_root {
        if let Some(first) = steps.first() {
            let local = strings.get(first.local)?;
            if local == ctx.root_element {
                steps = &steps[1..];
            }
        }
    } else if let Some(first) = steps.first() {
        let local = strings.get(first.local)?;
        if local == ctx.root_element {
            steps = &steps[1..];
        }
    }
    if steps.is_empty() {
        return Err(VmError::InvalidValue {
            message: "Schema Definition Error: expression evaluation error: empty path".into(),
        }
        .into());
    }
    eval_infoset_path_steps(steps, ctx.siblings, strings, tunables, ctx.element_name)
}

pub(crate) fn eval_input_value_calc_path(
    props: &IrProps,
    siblings: Option<&BTreeMap<alloc::string::String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    element_name: Option<&str>,
) -> Result<DfdlValue> {
    let steps = props
        .input_value_calc_path
        .as_ref()
        .ok_or_else(|| VmError::InvalidValue {
            message: "missing inputValueCalc path".into(),
        })?;
    let value = eval_infoset_path_steps(steps, siblings, strings, tunables, element_name)?;
    let text = dfdl_value_to_string(&value);
    Ok(DfdlValue::String(StringValue::new(text)))
}

pub(crate) fn eval_occurs_count_expression(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<alloc::string::String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<u64> {
    let value = eval_infoset_path_steps(steps, siblings, strings, tunables, None)?;
    if let Some(n) = value.as_i64() {
        if n >= 0 {
            return Ok(n as u64);
        }
    }
    Ok(count_dfdl_value_nodes(&value))
}

#[allow(dead_code)]
pub(crate) fn eval_fn_count_path(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<alloc::string::String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<u64> {
    eval_occurs_count_expression(steps, siblings, strings, tunables)
}

pub(crate) fn count_dfdl_value_nodes(value: &DfdlValue) -> u64 {
    match value {
        DfdlValue::Array(items) => items.len() as u64,
        DfdlValue::Null => 0,
        _ => 1,
    }
}

pub(crate) fn eval_input_value_calc_concat(
    props: &IrProps,
    siblings: Option<&BTreeMap<alloc::string::String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    root_element: &str,
) -> Result<DfdlValue> {
    use crate::ir::IrInputValueCalcSegment;
    let segments =
        props
            .input_value_calc_segments
            .as_ref()
            .ok_or_else(|| VmError::InvalidValue {
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
                let value = eval_ivc_path_steps(
                    false,
                    steps,
                    IvcEvalCtx {
                        siblings,
                        ancestor_frames: None,
                        root_element,
                        define_variables: &BTreeMap::new(),
                        runtime_variables: &BTreeMap::new(),
                        element_name: None,
                    },
                    strings,
                    tunables,
                )?;
                out.push_str(&dfdl_value_to_string(&value));
            }
            IrInputValueCalcSegment::ValueLength { sibling, units } => {
                let sib_name = strings.get(*sibling)?;
                let sib = siblings.and_then(|m| m.get(sib_name)).ok_or_else(|| {
                    VmError::InvalidValue {
                        message: alloc::format!("sibling `{sib_name}` not found for valueLength"),
                    }
                })?;
                let byte_len = value_byte_length(&sib.value)?;
                let len = length_in_units(byte_len, *units)?;
                out.push_str(&len.to_string());
            }
        }
    }
    Ok(DfdlValue::String(StringValue::new(out)))
}

pub(crate) fn eval_input_value_calc(
    props: &IrProps,
    kind: ValueKind,
    cursor: &Cursor<'_>,
    siblings: Option<&BTreeMap<alloc::string::String, SiblingState>>,
    strings: &crate::ir::StringPool,
    content_scope_bytes: Option<usize>,
    runtime_variables: &BTreeMap<alloc::string::String, alloc::string::String>,
    element_name: Option<&str>,
) -> Result<DfdlValue> {
    let calc = props
        .input_value_calc
        .ok_or_else(|| VmError::InvalidValue {
            message: "missing inputValueCalc".into(),
        })?;
    if calc == InputValueCalc::SchemaVariable {
        let name_id = props
            .input_value_calc_literal
            .ok_or_else(|| VmError::InvalidValue {
                message: "missing inputValueCalc variable name".into(),
            })?;
        let name = strings.get(name_id)?;
        let text = runtime_variables
            .get(name)
            .cloned()
            .ok_or_else(|| VmError::InvalidValue {
                message: ivc_sde_message(
                    element_name,
                    alloc::format!("variable `{name}` is not defined"),
                ),
            })?;
        if kind == ValueKind::String {
            return Ok(DfdlValue::String(StringValue::new(text)));
        }
        if kind == ValueKind::HexBinary {
            let bytes = crate::vm::runtime::decode_hex_binary(&text)?;
            return Ok(crate::value::DfdlValue::HexBinary(bytes));
        }
        let mut sub = Cursor::new(text.as_bytes());
        return crate::vm::runtime::read_text_scalar(
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
        let lit_id = props
            .input_value_calc_literal
            .ok_or_else(|| VmError::InvalidValue {
                message: "missing inputValueCalc string literal".into(),
            })?;
        let text = strings.get(lit_id)?;
        if kind == ValueKind::HexBinary {
            let bytes = crate::vm::runtime::decode_hex_binary(text)?;
            return Ok(crate::value::DfdlValue::HexBinary(bytes));
        }
        if kind == ValueKind::String {
            return Ok(DfdlValue::String(StringValue::new(text)));
        }
        if matches!(kind, ValueKind::DateTime | ValueKind::Time) {
            let parsed = crate::vm::calendar_binary::parse_xs_calendar_lexical(
                kind,
                props.calendar_date_only,
                text,
            )?;
            return Ok(DfdlValue::DateTime(parsed));
        }
        let mut sub = Cursor::new(text.as_bytes());
        return crate::vm::runtime::read_text_scalar(
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
                message: alloc::format!("Error Cannot convert {v} to NonNegativeInteger"),
            }
            .into());
        }
        return constant_input_value(kind, v);
    }
    if calc == InputValueCalc::ConstantLexical {
        let lit_id = props
            .input_value_calc_literal
            .ok_or_else(|| VmError::InvalidValue {
                message: "missing inputValueCalc lexical constant".into(),
            })?;
        let text = strings.get(lit_id)?;
        return parse_ivc_lexical_for_kind(text, kind, props, strings);
    }
    if calc == InputValueCalc::BooleanFromSibling {
        if kind != ValueKind::Boolean {
            return Err(VmError::InvalidValue {
                message: "xs:boolean inputValueCalc requires xs:boolean element".into(),
            }
            .into());
        }
        let sib = crate::vm::decoder::sibling_state(props, siblings, strings)?;
        let text = dfdl_value_text(&sib.value);
        return crate::vm::runtime::parse_xs_boolean_lexical(text)
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
        let sib = crate::vm::decoder::sibling_state(props, siblings, strings)?;
        let text = dfdl_value_text(&sib.value);
        let bytes = crate::vm::runtime::decode_hex_binary(text)?;
        return Ok(crate::value::DfdlValue::HexBinary(bytes));
    }
    let len = match calc {
        InputValueCalc::Constant(_)
        | InputValueCalc::ConstantLexical
        | InputValueCalc::StringLiteral
        | InputValueCalc::SchemaVariable
        | InputValueCalc::BooleanFromSibling
        | InputValueCalc::HexBinaryFromSibling => {
            return Err(VmError::InvalidValue {
                message: "handled above".into(),
            }
            .into())
        }
        InputValueCalc::ContentLengthSelf(units) | InputValueCalc::ValueLengthSelf(units) => {
            let byte_len = content_scope_bytes.unwrap_or_else(|| cursor.remaining());
            length_in_units(byte_len, units)?
        }
        InputValueCalc::ContentLengthSibling(units) => {
            let sib = crate::vm::decoder::sibling_state(props, siblings, strings)?;
            length_in_units(sib.content_bytes, units)?
        }
        InputValueCalc::ValueLengthSibling(_) => {
            let sib = crate::vm::decoder::sibling_state(props, siblings, strings)?;
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

pub(crate) fn constant_input_value(kind: ValueKind, value: i64) -> Result<DfdlValue> {
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
                message: alloc::format!("inputValueCalc constant `{value}` out of range for short"),
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
        Float => Ok(DfdlValue::Float(value as f32)),
        Double => Ok(DfdlValue::Double(value as f64)),
        String => Ok(DfdlValue::String(StringValue::new(value.to_string()))),
        Decimal => Ok(DfdlValue::Decimal(value.to_string())),
        Boolean => Ok(DfdlValue::Boolean(value != 0)),
        other => Err(VmError::InvalidValue {
            message: alloc::format!("inputValueCalc constant unsupported for `{other:?}`"),
        }),
    }
    .map_err(Into::into)
}

impl<'a> Decoder<'a> {
    pub(crate) fn finalize_ivc_value(
        &self,
        value: DfdlValue,
        kind: ValueKind,
        props: &IrProps,
    ) -> Result<DfdlValue> {
        let value = if kind != ValueKind::String {
            if let DfdlValue::String(s) = &value {
                parse_ivc_lexical_for_kind(s.text.as_str(), kind, props, self.ctx.strings())?
            } else {
                value
            }
        } else {
            value
        };
        if props.non_negative_integer {
            let is_neg = match &value {
                DfdlValue::Integer(s) => s.trim().starts_with('-'),
                DfdlValue::Long(n) => *n < 0,
                DfdlValue::Int(n) => *n < 0,
                DfdlValue::Short(n) => *n < 0,
                DfdlValue::Byte(n) => *n < 0,
                _ => false,
            };
            if is_neg {
                let s = dfdl_value_dispatch_string(&value);
                return Err(
                    crate::vm::runtime::parse_out_of_range("xs:nonNegativeInteger", &s).into(),
                );
            }
        }
        crate::vm::runtime::finalize_simple_value(
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
}
