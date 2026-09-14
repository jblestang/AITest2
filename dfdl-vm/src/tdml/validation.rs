use crate::ir::{IrNode, IrProgram, ValueKind};
use crate::value::DfdlValue;
use crate::vm::facet_validate::{needs_facet_validation, validate_decoded_facets};
use alloc::string::String;
use alloc::vec::Vec;

/// Collect XSD facet validation messages for a successfully decoded value tree.
pub fn collect_post_decode_validation_errors(
    program: &IrProgram,
    root_value: &DfdlValue,
) -> Vec<String> {
    let mut errors = Vec::new();
    let DfdlValue::Sequence(seq) = root_value else {
        return errors;
    };
    let Some(root_field) = seq.fields.get(&program.root_element) else {
        return errors;
    };
    let _ = walk_particle(program, program.root, root_field, &mut errors);
    errors
}

fn walk_particle(
    program: &IrProgram,
    node_id: u32,
    value: &DfdlValue,
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
                    walk_particle(program, *child_id, inner, errors)?;
                }
                return Ok(());
            }
            if matches!(value, DfdlValue::Null) {
                return Ok(());
            }
            if needs_facet_validation(props) {
                if let Err(e) = validate_decoded_facets(value, *kind, props, &program.strings) {
                    let detail = e.to_string();
                    if let Ok(ename) = program.strings.get(*name) {
                        errors.push(alloc::format!("Validation Error"));
                        errors.push(ename.to_string());
                        errors.push(alloc::format!("not valid"));
                        errors.push(detail.clone());
                        errors.push(alloc::format!("ex:{ename} {detail}"));
                    } else {
                        errors.push(detail);
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
                let IrNode::Element { name, .. } = program.node(child_id).map_err(|_| ())? else {
                    continue;
                };
                let key = program.strings.get(*name).map_err(|_| ())?;
                let Some(field_value) = seq.fields.get(key) else {
                    continue;
                };
                match field_value {
                    DfdlValue::Array(items) => {
                        for item in items {
                            walk_particle(program, child_id, item, errors)?;
                        }
                    }
                    other => walk_particle(program, child_id, other, errors)?,
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
                        return walk_particle(program, branch.node, branch_value, errors);
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
                    return walk_particle(program, branch.node, v, errors);
                }
            }
            Ok(())
        }
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
            return Ok(seq.fields.values().next().expect("one field"));
        }
    }
    Ok(value)
}
