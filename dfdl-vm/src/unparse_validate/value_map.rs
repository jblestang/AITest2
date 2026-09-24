use crate::error::VmError;
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};

fn ir_props_computed_at_unparse(props: &IrProps) -> bool {
    props.hidden
        || props.output_value_calc.is_some()
        || props.output_value_calc_literal.is_some()
        || props.output_value_calc_sibling.is_some()
        || props.output_value_calc_conditional
}

pub fn validate_unparse_value_map(
    program: &IrProgram,
    node_id: u32,
    map: &BTreeMap<String, DfdlValue>,
    parent_name: &str,
) -> Result<(), VmError> {
    match program.node(node_id).map_err(|e| VmError::InvalidValue {
        message: e.to_string(),
    })? {
        IrNode::Sequence { children, .. } => {
            for &child_id in children {
                let IrNode::Element { name, props, .. } =
                    program.node(child_id).map_err(|e| VmError::InvalidValue {
                        message: e.to_string(),
                    })?
                else {
                    continue;
                };
                if ir_props_computed_at_unparse(props) {
                    continue;
                }
                let key = program
                    .strings
                    .get(*name)
                    .map_err(|e| VmError::InvalidValue {
                        message: e.to_string(),
                    })?;
                let max = props.occurs_max.unwrap_or(u64::MAX);
                let min = props.occurs_min;
                let local = crate::xml_util::local_name_str(key);
                let count = match map.get(key).or_else(|| map.get(local)).or_else(|| {
                    map.iter()
                        .find(|(k, _)| crate::xml_util::local_name_str(k) == local)
                        .map(|(_, v)| v)
                }) {
                    Some(DfdlValue::Array(items)) => items.len() as u64,
                    Some(_) => 1,
                    None => 0,
                };
                if count < min {
                    return Err(VmError::InvalidValue {
                        message: format!(
                            "Unparse Error: Expected element start event for {key}, but received element end event for {parent_name}"
                        ),
                    });
                }
                if count > max {
                    return Err(VmError::InvalidValue {
                        message: format!(
                            "Unparse Error: {key} expected element end, but received extra occurrences at {parent_name}"
                        ),
                    });
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
