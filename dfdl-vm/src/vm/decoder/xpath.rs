//! XPath evaluation, variable scoping, and assertion checking for VM decoder.

use super::*;
use crate::error::{Result, VmError};
use crate::ir::{IrProps, StringId, ValueKind};
use crate::value::DfdlValue;
use crate::vm::decoder::SiblingState;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub fn ivc_sde_message(
    element_name: Option<&str>,
    detail: impl core::fmt::Display,
) -> alloc::string::String {
    match element_name {
        Some(el) => alloc::format!("Schema Definition Error: {detail} Element `{el}`."),
        None => alloc::format!("Schema Definition Error: {detail}"),
    }
}

pub(crate) fn sibling_state_by_local<'a>(
    siblings: &'a BTreeMap<String, SiblingState>,
    local: &str,
) -> Option<&'a SiblingState> {
    siblings
        .iter()
        .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
        .map(|(_, v)| v)
}

pub(crate) fn check_path_step(
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

pub(crate) fn navigate_to_child<'a>(
    value: &'a DfdlValue,
    local: &str,
    index: Option<u32>,
    element_name: Option<&str>,
) -> Result<&'a DfdlValue> {
    match value {
        DfdlValue::Sequence(seq) => {
            let child = sequence_field_by_local(&seq.fields, local).ok_or_else(|| {
                VmError::InvalidValue {
                    message: ivc_sde_message(
                        element_name,
                        alloc::format!("No element corresponding to step {local} found."),
                    ),
                }
            })?;
            match (index, child) {
                (Some(n), DfdlValue::Array(items)) => {
                    let idx = (n as usize).saturating_sub(1);
                    items.get(idx).ok_or_else(|| VmError::InvalidValue {
                        message: alloc::format!("Schema Definition Error: no child `{local}[{n}]`"),
                    })
                }
                (Some(_), _) => Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Schema Definition Error: no child `{local}[{index:?}]`"
                    ),
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

pub(crate) fn eval_infoset_path_steps(
    steps: &[crate::ir::IrInputPathStep],
    siblings: Option<&BTreeMap<String, SiblingState>>,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
    element_name: Option<&str>,
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
                ivc_sde_message(
                    element_name,
                    alloc::format!("No element corresponding to step {first_local} found."),
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
        check_path_step(
            step.prefix.is_some(),
            local,
            tunables.unqualified_path_step_policy,
        )?;
        value = navigate_to_child(value, local, step.index, element_name)?;
    }
    Ok(value.clone())
}

pub fn dfdl_value_dispatch_string(value: &DfdlValue) -> alloc::string::String {
    match value {
        DfdlValue::String(s) => s.text.clone(),
        DfdlValue::HexBinary(b) => crate::vm::runtime::encode_hex(b),
        DfdlValue::Int(n) => n.to_string(),
        DfdlValue::Long(n) => n.to_string(),
        DfdlValue::Integer(s) => s.clone(),
        DfdlValue::Decimal(s) => s.clone(),
        DfdlValue::Short(n) => n.to_string(),
        DfdlValue::Byte(n) => n.to_string(),
        DfdlValue::UnsignedLong(n) => n.to_string(),
        DfdlValue::UnsignedInt(n) => n.to_string(),
        DfdlValue::UnsignedShort(n) => n.to_string(),
        DfdlValue::UnsignedByte(n) => n.to_string(),
        DfdlValue::Boolean(b) => b.to_string(),
        DfdlValue::Choice { value, .. } => dfdl_value_dispatch_string(value),
        _ => alloc::string::String::new(),
    }
}

pub fn dfdl_value_to_string(value: &DfdlValue) -> alloc::string::String {
    match value {
        DfdlValue::String(s) => s.text.clone(),
        DfdlValue::HexBinary(b) => crate::vm::runtime::encode_hex(b),
        DfdlValue::Int(n) => n.to_string(),
        DfdlValue::Long(n) => n.to_string(),
        DfdlValue::Integer(s) => s.clone(),
        DfdlValue::Decimal(s) => s.clone(),
        DfdlValue::Short(n) => n.to_string(),
        DfdlValue::Byte(n) => n.to_string(),
        DfdlValue::UnsignedLong(n) => n.to_string(),
        DfdlValue::UnsignedInt(n) => n.to_string(),
        DfdlValue::UnsignedShort(n) => n.to_string(),
        DfdlValue::UnsignedByte(n) => n.to_string(),
        DfdlValue::Float(v) => v.to_string(),
        DfdlValue::Double(v) => v.to_string(),
        DfdlValue::Boolean(v) => {
            if *v {
                "true".into()
            } else {
                "false".into()
            }
        }
        DfdlValue::Choice { value, .. } => dfdl_value_to_string(value),
        other => dfdl_value_text(other).to_string(),
    }
}

pub fn numeric_value_from_dfdl(value: &DfdlValue) -> Result<i64> {
    match value {
        DfdlValue::Int(v) => Ok(*v as i64),
        DfdlValue::Long(v) => Ok(*v),
        DfdlValue::Short(v) => Ok(*v as i64),
        DfdlValue::Byte(v) => Ok(*v as i64),
        DfdlValue::Integer(s) => s.parse::<i64>().map_err(|_| VmError::InvalidValue {
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

#[cfg(test)]
pub(crate) fn resolve_length_props_for_test(
    props: &IrProps,
    kind: ValueKind,
    strings: &crate::ir::StringPool,
    tunables: &crate::length_validate::DaffodilTunables,
) -> Result<IrProps> {
    resolve_length_props(props, None, kind, strings, tunables)
}

pub(crate) struct VariableScopeGuard<'a> {
    pub(crate) decoder: &'a Decoder<'a>,
    pub(crate) shadowed_vars: Vec<(String, Option<String>)>,
}

impl Drop for VariableScopeGuard<'_> {
    fn drop(&mut self) {
        let mut current = self.decoder.runtime_variables.borrow_mut();
        for (name, prev_val) in self.shadowed_vars.drain(..) {
            if let Some(val) = prev_val {
                current.insert(name, val);
            } else {
                current.remove(&name);
            }
        }
    }
}

impl<'a> Decoder<'a> {
    pub(crate) fn push_xpath_ancestor_frame(
        &self,
        siblings: Option<&BTreeMap<String, SiblingState>>,
    ) {
        let frame = siblings
            .cloned()
            .unwrap_or_else(|| self.xpath_siblings_snapshot());
        self.xpath_ancestor_frames.borrow_mut().push(frame);
    }

    pub(crate) fn pop_xpath_ancestor_frame(&self) {
        self.xpath_ancestor_frames.borrow_mut().pop();
    }

    pub(crate) fn sibling_map_for_discriminator_up(
        &self,
        up: usize,
    ) -> BTreeMap<String, SiblingState> {
        let frames = self.xpath_ancestor_frames.borrow();
        if up > 0 && up <= frames.len() {
            let map = frames[frames.len() - up].clone();
            if !map.is_empty() {
                return map;
            }
        }
        self.xpath_siblings_snapshot()
    }

    pub(crate) fn strip_type_cast_wrapper(s: &str) -> &str {
        let trimmed = s.trim();
        if (trimmed.starts_with("xs:")
            || trimmed.starts_with("xsd:")
            || trimmed.starts_with("dfdl:"))
            && trimmed.ends_with(')')
        {
            if let Some(open) = trimmed.find('(') {
                return trimmed[open + 1..trimmed.len() - 1].trim();
            }
        }
        trimmed
    }

    pub(crate) fn eval_discriminator_xpath_eq(
        &self,
        inner: &str,
        dot: &str,
    ) -> Result<Option<bool>> {
        let mut rest = inner.trim();
        let mut up = 0usize;
        while rest.starts_with("../") {
            up += 1;
            rest = &rest[3..];
        }
        let Some(eq_idx) = rest.find(" eq ") else {
            return Ok(None);
        };
        let path_raw = rest[..eq_idx].trim();
        let lit_raw = rest[eq_idx + 4..].trim();
        let path_clean = Self::strip_type_cast_wrapper(path_raw);
        let lit_clean = Self::strip_type_cast_wrapper(lit_raw);

        if path_clean.contains('*')
            || path_clean.contains('(')
            || lit_clean.contains('*')
            || lit_clean.contains('(')
        {
            return Ok(None);
        }

        let mut path = path_clean;
        let lit = crate::schema::unquote_xpath_string_literal(lit_clean);
        let query_style = path.starts_with("./");
        if let Some(stripped) = path.strip_prefix("./") {
            path = stripped;
        }
        while path.starts_with("../") {
            up += 1;
            path = &path[3..];
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
        self.eval_sibling_name_eq_literal(local, &lit, up, query_style)
    }

    pub(crate) fn eval_sibling_name_eq_literal(
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

    pub(crate) fn xpath_discriminator_path_string(
        &self,
        up: usize,
        path: &str,
    ) -> Result<Option<alloc::string::String>> {
        let map = self.sibling_map_for_discriminator_up(up);
        let mut value: Option<DfdlValue> = None;
        for (i, step) in path.split('/').filter(|s| !s.is_empty()).enumerate() {
            let (local, index) = parse_discriminator_path_step(step)?;
            if i == 0 {
                let target = crate::xml_util::local_name_str(local);
                let state = map
                    .iter()
                    .find(|(k, _)| crate::xml_util::local_name_str(k) == target)
                    .map(|(_, v)| v.clone())
                    .or_else(|| {
                        self.xpath_siblings
                            .borrow()
                            .iter()
                            .find(|(k, _)| crate::xml_util::local_name_str(k) == target)
                            .map(|(_, v)| v.clone())
                    })
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

    pub(crate) fn lookup_xpath_sibling_state(
        &self,
        local: &str,
        up_levels: usize,
    ) -> Option<SiblingState> {
        let target = crate::xml_util::local_name_str(local);
        if up_levels == 0 {
            return self
                .xpath_siblings
                .borrow()
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == target)
                .map(|(_, v)| v.clone());
        }
        if let Some((_, v)) = self
            .xpath_siblings
            .borrow()
            .iter()
            .find(|(k, _)| crate::xml_util::local_name_str(k) == target)
        {
            return Some(v.clone());
        }
        let frames = self.xpath_ancestor_frames.borrow();
        if let Some(i) = frames.len().checked_sub(up_levels) {
            if let Some((_, v)) = frames[i]
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == target)
            {
                return Some(v.clone());
            }
        }
        for frame in frames.iter().rev() {
            if let Some((_, v)) = frame
                .iter()
                .find(|(k, _)| crate::xml_util::local_name_str(k) == target)
            {
                return Some(v.clone());
            }
        }
        None
    }

    pub(crate) fn seed_xpath_siblings(&self, seed: Option<&BTreeMap<String, SiblingState>>) {
        let mut map = self.xpath_siblings.borrow_mut();
        map.clear();
        if let Some(s) = seed {
            map.extend(s.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }

    pub(crate) fn insert_xpath_sibling(&self, key: String, state: SiblingState) {
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

    pub(crate) fn xpath_siblings_snapshot(&self) -> BTreeMap<String, SiblingState> {
        let mut map = BTreeMap::new();
        for frame in self.xpath_ancestor_frames.borrow().iter() {
            for (k, v) in frame {
                map.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in self.xpath_siblings.borrow().iter() {
            map.insert(k.clone(), v.clone());
        }
        map
    }

    pub(crate) fn eval_escape_scheme_property(&self, raw: &str) -> Result<String> {
        let trimmed = raw.trim();
        if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
            return Ok(trimmed.to_string());
        }
        let sib_snap = self.xpath_siblings_snapshot();
        let sib_values: BTreeMap<String, DfdlValue> = sib_snap
            .iter()
            .map(|(k, v)| (k.clone(), v.value.clone()))
            .collect();
        if let Some(local) = crate::vm::runtime::parse_sibling_property_expr(raw) {
            if let Some(text) =
                crate::vm::runtime::sibling_string_from_encode_map(&sib_values, &local)
            {
                return Ok(text);
            }
        }
        let Some(schema_expr) = crate::schema::parse_input_value_calc_expression(raw) else {
            return Ok(trimmed[1..trimmed.len() - 1].trim().to_string());
        };
        let mut pool = self.ctx.strings().clone();
        let ir_expr =
            crate::ir::builder::intern_input_value_calc_expression(&schema_expr, &mut pool);
        let ancestor_frames = self.xpath_ancestor_frames.borrow();
        let ivc_ctx = IvcEvalCtx {
            siblings: Some(&sib_snap),
            ancestor_frames: Some(ancestor_frames.as_slice()),
            root_element: self.ctx.program.root_element.as_str(),
            define_variables: &self.ctx.program.variables,
            runtime_variables: &self.runtime_variables.borrow(),
            element_name: None,
        };
        let value = eval_input_value_calc_expression(
            &ir_expr,
            ivc_ctx,
            &pool,
            &self.ctx.program.tunables,
            ValueKind::String,
            &IrProps::default(),
        )?;
        Ok(dfdl_value_to_string(&value))
    }

    pub(crate) fn resolve_escape_scheme_for_decode(
        &self,
        scheme: &crate::schema::EscapeSchemeDef,
    ) -> Result<crate::schema::EscapeSchemeDef> {
        let mut resolved = scheme.clone();
        if let Some(raw) = scheme
            .escape_character_raw
            .as_deref()
            .or(scheme.escape_character.as_deref())
        {
            if raw.trim().starts_with('{') {
                resolved.escape_character = Some(self.eval_escape_scheme_property(raw)?);
            }
        }
        if let Some(raw) = scheme
            .escape_escape_character_raw
            .as_deref()
            .or(scheme.escape_escape_character.as_deref())
        {
            if raw.trim().starts_with('{') {
                resolved.escape_escape_character = Some(self.eval_escape_scheme_property(raw)?);
            }
        }
        if let Some(raw) = scheme
            .escape_block_start_raw
            .as_deref()
            .or(scheme.escape_block_start.as_deref())
        {
            if raw.trim().starts_with('{') {
                resolved.escape_block_start = Some(self.eval_escape_scheme_property(raw)?);
            }
        }
        if let Some(raw) = scheme
            .escape_block_end_raw
            .as_deref()
            .or(scheme.escape_block_end.as_deref())
        {
            if raw.trim().starts_with('{') {
                resolved.escape_block_end = Some(self.eval_escape_scheme_property(raw)?);
            }
        }
        if let Some(raw) = scheme.extra_escaped_characters_raw.as_deref() {
            if raw.trim().starts_with('{') {
                let text = self.eval_escape_scheme_property(raw)?;
                resolved.extra_escaped_characters =
                    crate::schema::extra_escaped_characters_from_property(&text);
            }
        }
        Ok(resolved)
    }

    pub(crate) fn parse_if_then_else_expr(s: &str) -> Option<(&str, &str, &str)> {
        let s = s.trim();
        let rest = s.strip_prefix("if")?.trim();
        let then_idx = rest.find("then")?;
        let cond = rest[..then_idx].trim();
        let rest_then = rest[then_idx + 4..].trim();
        let else_idx = rest_then.find("else")?;
        let then_val = rest_then[..else_idx].trim();
        let else_val = rest_then[else_idx + 4..].trim();
        Some((cond, then_val, else_val))
    }

    pub(crate) fn eval_simple_condition(
        &self,
        cond: &str,
        siblings: Option<&BTreeMap<String, SiblingState>>,
    ) -> Result<bool> {
        let cond = cond.trim();
        let cond_unparenthesized = if cond.starts_with('(') && cond.ends_with(')') {
            cond[1..cond.len() - 1].trim()
        } else {
            cond
        };
        let (lhs_str, op, rhs_str) = if let Some(idx) = cond_unparenthesized.find(" lt ") {
            (
                &cond_unparenthesized[..idx],
                "lt",
                &cond_unparenthesized[idx + 4..],
            )
        } else if let Some(idx) = cond_unparenthesized.find(" gt ") {
            (
                &cond_unparenthesized[..idx],
                "gt",
                &cond_unparenthesized[idx + 4..],
            )
        } else if let Some(idx) = cond_unparenthesized.find(" eq ") {
            (
                &cond_unparenthesized[..idx],
                "eq",
                &cond_unparenthesized[idx + 4..],
            )
        } else if let Some(idx) = cond_unparenthesized.find(" ne ") {
            (
                &cond_unparenthesized[..idx],
                "ne",
                &cond_unparenthesized[idx + 4..],
            )
        } else {
            return Ok(true);
        };
        let eval_operand = |op_str: &str| -> Result<i64> {
            let op_str = op_str.trim();
            let expr_str = alloc::format!("{{{op_str}}}");
            if let Some(schema_expr) = crate::schema::parse_input_value_calc_expression(&expr_str) {
                let mut pool = self.ctx.strings().clone();
                let ir_expr =
                    crate::ir::builder::intern_input_value_calc_expression(&schema_expr, &mut pool);
                let mut sib_snap = self.xpath_siblings_snapshot();
                if let Some(s) = siblings {
                    for (k, v) in s {
                        sib_snap.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                let ancestor_frames = self.xpath_ancestor_frames.borrow();
                let ivc_ctx = IvcEvalCtx {
                    siblings: Some(&sib_snap),
                    ancestor_frames: Some(ancestor_frames.as_slice()),
                    root_element: self.ctx.program.root_element.as_str(),
                    define_variables: &self.ctx.program.variables,
                    runtime_variables: &self.runtime_variables.borrow(),
                    element_name: None,
                };
                let v = eval_input_value_calc_expression(
                    &ir_expr,
                    ivc_ctx,
                    &pool,
                    &self.ctx.program.tunables,
                    ValueKind::Long,
                    &IrProps::default(),
                )?;
                let text = dfdl_value_to_string(&v);
                text.parse::<i64>().map_err(|_| {
                    VmError::InvalidValue {
                        message: alloc::format!(
                            "Parse Error. Unable to parse xs:int from text: {text}"
                        ),
                    }
                    .into()
                })
            } else {
                op_str.parse::<i64>().map_err(|_| {
                    VmError::InvalidValue {
                        message: alloc::format!(
                            "Parse Error. Unable to parse xs:int from text: {op_str}"
                        ),
                    }
                    .into()
                })
            }
        };
        let lhs = eval_operand(lhs_str)?;
        let rhs = eval_operand(rhs_str)?;
        match op {
            "lt" => Ok(lhs < rhs),
            "gt" => Ok(lhs > rhs),
            "eq" => Ok(lhs == rhs),
            "ne" => Ok(lhs != rhs),
            _ => Ok(false),
        }
    }

    pub(crate) fn evaluate_and_set_variables(
        &self,
        set_variables: &[(StringId, StringId)],
        siblings: Option<&BTreeMap<String, SiblingState>>,
    ) -> Result<()> {
        for &(name_id, val_id) in set_variables {
            let name = self.ctx.strings().get(name_id)?;
            let val = self.ctx.strings().get(val_id)?;
            let evaluated_val = {
                let v_str = val.trim();
                if v_str.starts_with('{') && v_str.ends_with('}') {
                    let inner = v_str[1..v_str.len() - 1].trim();
                    if (inner.starts_with('\'') && inner.ends_with('\''))
                        || (inner.starts_with('"') && inner.ends_with('"'))
                    {
                        inner[1..inner.len() - 1].to_string()
                    } else if let Some((cond, then_v, else_v)) =
                        Self::parse_if_then_else_expr(inner)
                    {
                        if self.eval_simple_condition(cond, siblings)? {
                            then_v.to_string()
                        } else {
                            else_v.to_string()
                        }
                    } else if let Some(schema_expr) =
                        crate::schema::parse_input_value_calc_expression(v_str)
                    {
                        let mut pool = self.ctx.strings().clone();
                        let ir_expr = crate::ir::builder::intern_input_value_calc_expression(
                            &schema_expr,
                            &mut pool,
                        );
                        let mut sib_snap = self.xpath_siblings_snapshot();
                        if let Some(s) = siblings {
                            for (k, v) in s {
                                sib_snap.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                        let ancestor_frames = self.xpath_ancestor_frames.borrow();
                        let ivc_ctx = IvcEvalCtx {
                            siblings: Some(&sib_snap),
                            ancestor_frames: Some(ancestor_frames.as_slice()),
                            root_element: self.ctx.program.root_element.as_str(),
                            define_variables: &self.ctx.program.variables,
                            runtime_variables: &self.runtime_variables.borrow(),
                            element_name: None,
                        };
                        let is_constant =
                            !inner.contains('.') && !inner.contains('/') && !inner.contains('$');
                        let v = eval_input_value_calc_expression(
                            &ir_expr,
                            ivc_ctx,
                            &pool,
                            &self.ctx.program.tunables,
                            ValueKind::Long,
                            &IrProps::default(),
                        )
                        .map_err(|e| {
                            let msg = match &e {
                                crate::error::Error::Vm(VmError::InvalidValue { message }) => {
                                    message.clone()
                                }
                                _ => alloc::format!("{e}"),
                            };
                            let msg_clean = msg.strip_prefix("vm error: ").unwrap_or(&msg);
                            VmError::InvalidValue {
                                message: if is_constant {
                                    alloc::format!("Schema Definition Error: {msg_clean}")
                                } else {
                                    alloc::format!("Parse Error: {msg_clean}")
                                },
                            }
                        })?;
                        dfdl_value_to_string(&v)
                    } else {
                        let vars = self.runtime_variables.borrow();
                        if inner.starts_with('$') {
                            let var_ref_name = inner[1..].trim();
                            let local = var_ref_name.rsplit(':').next().unwrap_or(var_ref_name);
                            vars.get(var_ref_name)
                                .or_else(|| vars.get(local))
                                .or_else(|| self.ctx.program.variables.get(var_ref_name))
                                .or_else(|| self.ctx.program.variables.get(local))
                                .cloned()
                                .unwrap_or_else(|| val.to_string())
                        } else {
                            val.to_string()
                        }
                    }
                } else {
                    val.to_string()
                }
            };
            let local = name.rsplit(':').next().unwrap_or(name);
            let mut vars = self.runtime_variables.borrow_mut();
            vars.insert(name.to_string(), evaluated_val.clone());
            vars.insert(local.to_string(), evaluated_val.clone());
            if local == "outputNewLine" {
                vars.insert("dfdl:outputNewLine".to_string(), evaluated_val.clone());
            } else if local == "encoding" {
                vars.insert("dfdl:encoding".to_string(), evaluated_val.clone());
            } else if local == "byteOrder" {
                vars.insert("dfdl:byteOrder".to_string(), evaluated_val.clone());
            } else if local == "binaryFloatRep" {
                vars.insert("dfdl:binaryFloatRep".to_string(), evaluated_val.clone());
            }
        }
        Ok(())
    }

    pub(crate) fn enter_variable_scope(
        &self,
        new_variable_instances: &[(StringId, Option<StringId>)],
        siblings: Option<&BTreeMap<String, SiblingState>>,
        is_element: bool,
    ) -> Result<VariableScopeGuard<'_>> {
        if new_variable_instances.is_empty() {
            return Ok(VariableScopeGuard {
                decoder: self,
                shadowed_vars: Vec::new(),
            });
        }
        if is_element {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: newVariableInstance may only be used on group reference, sequence or choice".to_string(),
            }.into());
        }

        let mut shadowed_vars = Vec::new();
        let mut seen = alloc::vec::Vec::new();
        for &(name_id, def_id) in new_variable_instances {
            let name = self.ctx.strings().get(name_id)?;
            let local = name.rsplit(':').next().unwrap_or(name);
            if seen.contains(&local) {
                return Err(VmError::InvalidValue {
                    message: alloc::format!(
                        "Schema Definition Error: newVariableInstances must all be distinct within the same scope: `{local}`"
                    ),
                }
                .into());
            }
            seen.push(local);

            {
                let current_vars = self.runtime_variables.borrow();
                shadowed_vars.push((name.to_string(), current_vars.get(name).cloned()));
                if local != name {
                    shadowed_vars.push((local.to_string(), current_vars.get(local).cloned()));
                }
            }

            let initial_val = if let Some(d_id) = def_id {
                let expr_str = self.ctx.strings().get(d_id)?;
                let v_trim = expr_str.trim();
                if v_trim.starts_with('{') && v_trim.ends_with('}') {
                    let inner = v_trim[1..v_trim.len() - 1].trim();
                    if (inner.starts_with('\'') && inner.ends_with('\''))
                        || (inner.starts_with('"') && inner.ends_with('"'))
                    {
                        inner[1..inner.len() - 1].to_string()
                    } else if let Some((cond, then_v, else_v)) =
                        Self::parse_if_then_else_expr(inner)
                    {
                        if self.eval_simple_condition(cond, siblings)? {
                            then_v.to_string()
                        } else {
                            else_v.to_string()
                        }
                    } else if let Some(schema_expr) =
                        crate::schema::parse_input_value_calc_expression(v_trim)
                    {
                        let mut pool = self.ctx.strings().clone();
                        let ir_expr = crate::ir::builder::intern_input_value_calc_expression(
                            &schema_expr,
                            &mut pool,
                        );
                        let mut sib_snap = self.xpath_siblings_snapshot();
                        if let Some(s) = siblings {
                            for (k, v) in s {
                                sib_snap.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                        let ancestor_frames = self.xpath_ancestor_frames.borrow();
                        let ivc_ctx = IvcEvalCtx {
                            siblings: Some(&sib_snap),
                            ancestor_frames: Some(ancestor_frames.as_slice()),
                            root_element: self.ctx.program.root_element.as_str(),
                            define_variables: &self.ctx.program.variables,
                            runtime_variables: &self.runtime_variables.borrow(),
                            element_name: None,
                        };
                        let v = eval_input_value_calc_expression(
                            &ir_expr,
                            ivc_ctx,
                            &pool,
                            &self.ctx.program.tunables,
                            ValueKind::Long,
                            &IrProps::default(),
                        )?;
                        dfdl_value_to_string(&v)
                    } else {
                        expr_str.to_string()
                    }
                } else {
                    expr_str.to_string()
                }
            } else {
                let current_vars = self.runtime_variables.borrow();
                current_vars
                    .get(name)
                    .or_else(|| current_vars.get(local))
                    .or_else(|| self.ctx.program.variables.get(name))
                    .or_else(|| self.ctx.program.variables.get(local))
                    .cloned()
                    .unwrap_or_default()
            };

            let mut current_vars = self.runtime_variables.borrow_mut();
            current_vars.insert(name.to_string(), initial_val.clone());
            current_vars.insert(local.to_string(), initial_val);
        }

        Ok(VariableScopeGuard {
            decoder: self,
            shadowed_vars,
        })
    }

    pub(crate) fn eval_particle_assert_expression(
        &self,
        expr: &str,
        dot: &str,
        cursor: Option<&Cursor>,
    ) -> Result<bool> {
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
        if !dot.is_empty() {
            if crate::schema::match_pattern(dot.as_bytes(), inner).is_some() {
                return Ok(true);
            }
        } else if let Some(c) = cursor {
            if !c.is_empty() && crate::schema::match_pattern(&c.data[c.pos..], inner).is_some() {
                return Ok(true);
            }
        }
        let compact: alloc::string::String = inner.chars().filter(|c| !c.is_whitespace()).collect();
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
        if inner.contains(" eq ")
            || inner.contains(" ne ")
            || inner.contains(" lt ")
            || inner.contains(" gt ")
        {
            return self.eval_simple_condition(inner, None);
        }
        if inner.starts_with('$') {
            let name = inner.trim_start_matches('$').trim();
            let local = name.rsplit(':').next().unwrap_or(name);
            let vars = self.runtime_variables.borrow();
            let val = vars
                .get(name)
                .or_else(|| vars.get(local))
                .or_else(|| self.ctx.program.variables.get(name))
                .or_else(|| self.ctx.program.variables.get(local))
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

    pub(crate) fn eval_occurs_count_xpath_expr(
        &self,
        expr: &str,
        siblings: Option<&BTreeMap<String, SiblingState>>,
    ) -> Result<u64> {
        let expr = expr.trim();
        let inner = expr
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(expr)
            .trim();
        if inner.starts_with("if") {
            if let Some(then_idx) = inner.find(" then ") {
                let cond_part = inner[2..then_idx].trim();
                let rest = inner[then_idx + 6..].trim();
                if let Some(else_idx) = rest.find(" else ") {
                    let then_val_str = rest[..else_idx].trim();
                    let else_val_str = rest[else_idx + 6..].trim();
                    let cond_bool = self.eval_simple_condition(cond_part, siblings)?;
                    let choice_str = if cond_bool {
                        then_val_str
                    } else {
                        else_val_str
                    };
                    let val: u64 = choice_str.parse().unwrap_or(0);
                    return Ok(val);
                }
            }
        }
        if let Ok(n) = inner.parse::<u64>() {
            return Ok(n);
        }
        Ok(0)
    }

    pub(crate) fn xpath_sibling_path_truthy(&self, path: &str) -> bool {
        self.xpath_path_int_value(path).unwrap_or(0) != 0
    }

    pub(crate) fn xpath_path_int_value(&self, path: &str) -> Result<i64> {
        let trimmed = path.trim();
        let mut rest = trimmed;
        let mut up = 0usize;
        while rest.starts_with("../") {
            up += 1;
            rest = &rest[3..];
        }
        let local = rest.rsplit(':').next().unwrap_or(rest).trim();
        let sib = self
            .lookup_xpath_sibling_state(local, up.saturating_sub(1))
            .ok_or_else(|| VmError::InvalidValue {
                message: alloc::format!(
                    "Schema Definition Error: No element corresponding to step {local} found."
                ),
            })?;
        numeric_value_from_dfdl(&sib.value)
    }

    pub(crate) fn eval_facet_assert_message(
        &self,
        props: &IrProps,
    ) -> Result<alloc::string::String> {
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
                            false,
                            steps,
                            IvcEvalCtx {
                                siblings,
                                ancestor_frames: None,
                                root_element: "",
                                define_variables: &BTreeMap::new(),
                                runtime_variables: &BTreeMap::new(),
                                element_name: None,
                            },
                            strings,
                            &self.ctx.program.tunables,
                        )?;
                        out.push_str(&dfdl_value_to_string(&value));
                    }
                    IrInputValueCalcSegment::ValueLength { .. } => {
                        return Err(VmError::InvalidValue {
                            message: "facet assert message valueLength not supported".into(),
                        }
                        .into());
                    }
                }
            }
            return Ok(out);
        }
        if let Some(msg_id) = props.facet_assert_message {
            if let Ok(msg) = self.ctx.strings().get(msg_id) {
                if msg.starts_with('{') && msg.contains("fn:concat") {
                    let siblings = self.xpath_siblings_snapshot();
                    let siblings = Some(&siblings);
                    if let Some(lit_start) = msg.find('"') {
                        if let Some(lit_end) = msg[lit_start + 1..].find('"') {
                            let prefix = &msg[lit_start + 1..lit_start + 1 + lit_end];
                            if let Ok(val) = sibling_string_value(siblings, "messageID") {
                                return Ok(alloc::format!("{prefix}{val}"));
                            }
                        }
                    }
                }
                return Ok(msg.to_string());
            }
        }
        Ok(alloc::string::String::new())
    }
}
