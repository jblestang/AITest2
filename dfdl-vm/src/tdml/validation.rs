use crate::ir::{IrNode, IrProgram, ValueKind};
use crate::schema::{SchemaDocument, TypeName, validate_union_membership};
use crate::value::DfdlValue;
use crate::vm::facet_validate::{needs_facet_validation, validate_decoded_facets};
use alloc::vec::Vec;

/// Collect XSD facet validation messages for a successfully decoded value tree.
pub fn collect_post_decode_validation_errors(
    schema: &SchemaDocument,
    program: &IrProgram,
    root_value: &DfdlValue,
    full_xerces_style: bool,
) -> Vec<String> {
    let mut errors = Vec::new();
    let root_field = resolve_root_field_value(root_value, &program.root_element);
    let _ = walk_particle(schema, program, program.root, root_field, full_xerces_style, &mut errors);
    errors
}

fn walk_particle(
    schema: &SchemaDocument,
    program: &IrProgram,
    node_id: u32,
    value: &DfdlValue,
    full_xerces_style: bool,
    errors: &mut Vec<String>,
) -> Result<(), ()> {
    match program.node(node_id).map_err(|_| ())? {
        IrNode::Element {
            name,
            kind,
            props,
            child,
        } => {
            if *kind == ValueKind::Complex {
                if let Some(child_id) = child {
                    let inner = complex_element_value(value, program.strings.get(*name).ok())?;
                    walk_particle(schema, program, *child_id, inner, full_xerces_style, errors)?;
                }
                return Ok(());
            }
            if matches!(value, DfdlValue::Null) {
                return Ok(());
            }
            let ename = program.strings.get(*name).ok();
            if needs_facet_validation(props) {
                if let Err(e) = validate_decoded_facets(
                    value,
                    *kind,
                    props,
                    &program.strings,
                    &program.tunables,
                ) {
                    let detail = e.to_string();
                    if let Some(ename) = ename {
                        if detail.contains("facet pattern") {
                            if full_xerces_style || *kind == ValueKind::String {
                                errors.push(alloc::format!("Validation Error"));
                                errors.push(ename.to_string());
                                errors.push(alloc::format!("pattern"));
                            } else {
                                errors.push(alloc::format!("failed facet checks"));
                                errors.push(alloc::format!("pattern"));
                            }
                            if let Some(start) = detail.find('(') {
                                if let Some(end) = detail.rfind(')') {
                                    errors.push(detail[start + 1..end].to_string());
                                }
                            }
                            let lex = value_lexical(value, *kind).unwrap_or("");
                            if full_xerces_style && !lex.is_empty() {
                                errors.push(alloc::format!("'{lex}'"));
                                errors.push(alloc::format!("not facet-valid"));
                                errors.push(alloc::format!("pattern"));
                                if let Some(start) = detail.find('(') {
                                    if let Some(end) = detail.rfind(')') {
                                        errors.push(alloc::format!(
                                            "'{}'",
                                            &detail[start + 1..end]
                                        ));
                                    }
                                }
                            }
                        } else if detail.contains("failed facet checks") {
                            let rest = detail
                                .strip_prefix("failed facet checks due to: ")
                                .map(str::trim);
                            if full_xerces_style
                                && matches!(
                                    *kind,
                                    ValueKind::Int
                                        | ValueKind::Long
                                        | ValueKind::Short
                                        | ValueKind::Byte
                                        | ValueKind::UnsignedInt
                                        | ValueKind::UnsignedShort
                                        | ValueKind::UnsignedByte
                                )
                                && rest.is_some_and(|r| r.contains("maxInclusive"))
                            {
                                if let Some(lex) = value_lexical_any(value, *kind) {
                                    errors.push(alloc::format!("Validation Error"));
                                    errors.push(alloc::format!("Value '{lex}'"));
                                    errors.push(alloc::format!("not valid"));
                                    errors.push(alloc::format!("ex:{ename}"));
                                }
                            } else if full_xerces_style
                                && rest.is_some_and(|r| r.contains("minLength"))
                                && *kind == ValueKind::String
                            {
                                if let Some(lex) = value_lexical(value, *kind) {
                                    let len = lex.chars().count();
                                    if let Some(min) = props.min_length {
                                        errors.push(alloc::format!("Validation Error"));
                                        errors.push(alloc::format!(
                                            "Value '{lex}' with length = '{len}' is not facet-valid with respect to minLength '{min}'"
                                        ));
                                    }
                                }
                            } else if full_xerces_style && rest.is_some_and(|r| r.contains("totalDigits")) {
                                if let Some(max) = props.total_digits {
                                    errors.push(ename.to_string());
                                    errors.push(alloc::format!("not valid"));
                                    let has_range = props.value_min_inclusive.is_some()
                                        || props.value_max_inclusive.is_some()
                                        || props.value_min_exclusive.is_some()
                                        || props.value_max_exclusive.is_some();
                                    if has_range {
                                        if let Some(lex) = value_lexical_any(value, *kind) {
                                            let digits = xsd_total_digit_count(&lex);
                                            errors.push(ename.to_string());
                                            errors.push(alloc::format!(
                                                "Value '{lex}' has {digits} total digits"
                                            ));
                                            errors.push(alloc::format!(
                                                "total digits has been limited to {max}."
                                            ));
                                        }
                                    } else {
                                        errors.push(alloc::format!(
                                            "total digits has been limited to {max}"
                                        ));
                                    }
                                }
                            } else if full_xerces_style && rest == Some("enumeration") {
                                errors.push(ename.to_string());
                                errors.push(alloc::format!("not valid"));
                                if let Some(lex) = value_lexical(value, *kind) {
                                    errors.push(lex.to_string());
                                }
                                errors.push(alloc::format!("not facet-valid"));
                                let allowed = props
                                    .facet_enumeration
                                    .iter()
                                    .filter_map(|id| program.strings.get(*id).ok())
                                    .collect::<alloc::vec::Vec<_>>()
                                    .join(", ");
                                if !allowed.is_empty() {
                                    errors.push(allowed);
                                }
                            } else if full_xerces_style
                                && rest.is_some_and(|r| {
                                    r.contains("Inclusive") || r.contains("Exclusive")
                                })
                            {
                                errors.push(alloc::format!("Validation Error"));
                                if detail.contains("maxInclusive") {
                                    errors.push(alloc::format!("maxInclusive"));
                                    if let Some(max) = props.value_max_inclusive {
                                        errors.push(format_xerces_float_bound(max as f64));
                                    }
                                } else if detail.contains("minInclusive") {
                                    errors.push(alloc::format!("minInclusive"));
                                    if let Some(min) = props.value_min_inclusive {
                                        errors.push(format_xerces_float_bound(min as f64));
                                    }
                                } else if detail.contains("maxExclusive") {
                                    errors.push(alloc::format!("maxExclusive"));
                                    if let Some(max) = props.value_max_exclusive {
                                        errors.push(format_xerces_float_bound(max as f64));
                                    }
                                } else if detail.contains("minExclusive") {
                                    errors.push(alloc::format!("minExclusive"));
                                    if let Some(min) = props.value_min_exclusive {
                                        errors.push(format_xerces_float_bound(min as f64));
                                    }
                                }
                            } else if !full_xerces_style {
                                errors.push(ename.to_string());
                                errors.push(alloc::format!("failed facet checks"));
                                if let Some(r) = rest {
                                    if r.contains("enumeration") {
                                        errors.push(alloc::format!("facet enumeration(s)"));
                                        let allowed = props
                                            .facet_enumeration
                                            .iter()
                                            .filter_map(|id| program.strings.get(*id).ok())
                                            .collect::<alloc::vec::Vec<_>>()
                                            .join("|");
                                        if !allowed.is_empty() {
                                            errors.push(allowed);
                                        }
                                    } else if r.contains("maxLength") {
                                        errors.push(alloc::format!("due to: facet {r}"));
                                    } else if r.starts_with("facet ") {
                                        errors.push(r.to_string());
                                    } else {
                                        errors.push(alloc::format!("facet {r}"));
                                    }
                                }
                            } else {
                                errors.push(ename.to_string());
                                errors.push(alloc::format!("failed facet checks"));
                                if let Some(r) = rest {
                                    if r.contains("enumeration") {
                                        errors.push(alloc::format!("facet enumeration(s)"));
                                        let allowed = props
                                            .facet_enumeration
                                            .iter()
                                            .filter_map(|id| program.strings.get(*id).ok())
                                            .collect::<alloc::vec::Vec<_>>()
                                            .join("|");
                                        if !allowed.is_empty() {
                                            errors.push(allowed);
                                        }
                                    } else if r.starts_with("facet ") {
                                        errors.push(r.to_string());
                                    } else {
                                        errors.push(alloc::format!("facet {r}"));
                                    }
                                }
                                errors.push(detail.clone());
                            }
                        } else {
                            errors.push(alloc::format!("Validation Error"));
                            errors.push(ename.to_string());
                            errors.push(alloc::format!("not valid"));
                            errors.push(detail.clone());
                            errors.push(alloc::format!("ex:{ename} {detail}"));
                        }
                    } else {
                        errors.push(detail);
                    }
                }
            }
            if let (Some(type_id), Some(text)) = (props.xsd_type, value_lexical(value, *kind)) {
                let type_name = TypeName::new(program.strings.get(type_id).map_err(|_| ())?);
                if !validate_union_membership(schema, &type_name, text) {
                    errors.push(text.to_string());
                    errors.push(alloc::format!("not one of the union members"));
                    if let Some(ename) = ename {
                        errors.push(alloc::format!("ex:{ename}"));
                    }
                }
            }
            Ok(())
        }
        IrNode::Sequence { children, .. } => {
            let DfdlValue::Sequence(seq) = value else {
                return Ok(());
            };
            for &child_id in children {
                let IrNode::Element { name, props, .. } = program.node(child_id).map_err(|_| ())?
                else {
                    continue;
                };
                let key = program.strings.get(*name).map_err(|_| ())?;
                let Some(field_value) = seq.fields.get(key) else {
                    continue;
                };
                match field_value {
                    DfdlValue::Array(items) => {
                        let count = items.len() as u64;
                        let min = props.occurs_min;
                        let max = props.occurs_max.unwrap_or(u64::MAX);
                        if count < min || (props.occurs_max.is_some() && count > max) {
                            if full_xerces_style {
                                if props.occurs_max.is_some() && count > max {
                                    errors.push(key.to_string());
                                    errors.push(alloc::format!("{count} occur"));
                                    errors.push(alloc::format!("expected"));
                                    errors.push(alloc::format!("maximum of '{max}'"));
                                    errors.push(alloc::format!("exceeded"));
                                }
                            } else {
                                errors.push(key.to_string());
                                errors.push(alloc::format!("occurred"));
                                errors.push(alloc::format!("expected"));
                                errors.push(alloc::format!("minimum of '{min}'"));
                                if props.occurs_max.is_some() {
                                    errors.push(alloc::format!("maximum of '{max}'"));
                                }
                                errors.push(alloc::format!(
                                    "{key} occurred '{count}' times when it was expected to be a minimum of '{min}' and a maximum of '{max}' times."
                                ));
                            }
                        }
                        for item in items {
                            walk_particle(schema, program, child_id, item, full_xerces_style, errors)?;
                        }
                    }
                    other => walk_particle(schema, program, child_id, other, full_xerces_style, errors)?,
                }
            }
            Ok(())
        }
        IrNode::Choice { branches, .. } => {
            if let DfdlValue::Choice {
                discriminator,
                value: branch_value,
            } = value
            {
                for branch in branches {
                    let branch_name = program.strings.get(branch.name).map_err(|_| ())?;
                    if branch_name == discriminator.as_str() {
                        return walk_particle(
                            schema,
                            program,
                            branch.node,
                            branch_value,
                            full_xerces_style,
                            errors,
                        );
                    }
                }
                return Ok(());
            }
            let DfdlValue::Sequence(seq) = value else {
                return Ok(());
            };
            for branch in branches {
                let branch_name = program.strings.get(branch.name).map_err(|_| ())?;
                if let Some(v) = seq.fields.get(branch_name) {
                    return walk_particle(schema, program, branch.node, v, full_xerces_style, errors);
                }
            }
            Ok(())
        }
    }
}

fn xsd_total_digit_count(lex: &str) -> usize {
    let mut s = lex.trim();
    if let Some(rest) = s.strip_prefix('-') {
        s = rest;
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    }
    let digits: alloc::string::String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        0
    } else {
        trimmed.len()
    }
}

fn format_xerces_float_bound(v: f64) -> alloc::string::String {
    if v.fract() == 0.0 && v.is_finite() {
        alloc::format!("{v:.1}")
    } else {
        alloc::format!("{v}")
    }
}

fn resolve_root_field_value<'a>(root_value: &'a DfdlValue, root_element: &str) -> &'a DfdlValue {
    if let Some(seq) = root_value.sequence_value() {
        if let Some(inner) = seq.fields.get(root_element) {
            return inner;
        }
        if seq.fields.len() == 1 {
            if let Some((name, inner)) = seq.fields.iter().next() {
                if name == root_element {
                    return inner;
                }
            }
        }
    }
    root_value
}

fn value_lexical<'a>(value: &'a DfdlValue, kind: ValueKind) -> Option<&'a str> {
    match (kind, value) {
        (ValueKind::String, DfdlValue::String(s)) => Some(s.text.as_str()),
        (_, DfdlValue::String(s)) => Some(s.text.as_str()),
        _ => None,
    }
}

fn value_lexical_any(value: &DfdlValue, kind: ValueKind) -> Option<alloc::string::String> {
    if let Some(s) = value_lexical(value, kind) {
        return Some(s.to_string());
    }
    match (kind, value) {
        (ValueKind::Int, DfdlValue::Int(v)) => Some(v.to_string()),
        (ValueKind::Long, DfdlValue::Long(v)) => Some(v.to_string()),
        (ValueKind::Short, DfdlValue::Short(v)) => Some(v.to_string()),
        (ValueKind::Byte, DfdlValue::Byte(v)) => Some(v.to_string()),
        (ValueKind::UnsignedInt, DfdlValue::UnsignedInt(v)) => Some(v.to_string()),
        (ValueKind::UnsignedShort, DfdlValue::UnsignedShort(v)) => Some(v.to_string()),
        (ValueKind::UnsignedByte, DfdlValue::UnsignedByte(v)) => Some(v.to_string()),
        _ => None,
    }
}

fn complex_element_value<'a>(
    value: &'a DfdlValue,
    element_name: Option<&str>,
) -> Result<&'a DfdlValue, ()> {
    if let DfdlValue::Sequence(seq) = value {
        if let Some(name) = element_name {
            if let Some(inner) = seq.fields.get(name) {
                return Ok(inner);
            }
        }
        if seq.fields.len() == 1 {
            if let Some((key, inner)) = seq.fields.iter().next() {
                if element_name.is_none() || element_name == Some(key.as_str()) {
                    return Ok(inner);
                }
            }
        }
    }
    Ok(value)
}
