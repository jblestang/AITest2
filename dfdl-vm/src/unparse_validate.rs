use crate::error::VmError;
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::SchemaDocument;
use crate::tdml::InfosetNode;
use crate::value::DfdlValue;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;

fn qname_for_error(local: &str, ns: Option<&str>) -> String {
    match ns {
        Some(uri) if !uri.is_empty() => format!("{{{uri}}}{local}"),
        _ => format!("{{}}{local}"),
    }
}

fn element_qname_in_errors(local: &str, tns: Option<&str>) -> String {
    qname_for_error(local, tns.filter(|u| !u.is_empty()))
}

fn parent_qname(root: &str, target_ns: Option<&str>, qualified: bool) -> String {
    if root.starts_with('{') {
        return root.to_string();
    }
    if qualified {
        qname_for_error(root, target_ns)
    } else if let Some(ns) = target_ns.filter(|u| !u.is_empty()) {
        qname_for_error(root, Some(ns))
    } else {
        root.to_string()
    }
}

pub fn validate_unparse_infoset_nodes(
    schema: &SchemaDocument,
    program: &IrProgram,
    root: &str,
    nodes: &[InfosetNode],
) -> Result<(), String> {
    validate_unparse_infoset_nodes_inner(schema, program, root, nodes, true)
}

pub fn validate_unparse_infoset_cardinality(
    schema: &SchemaDocument,
    program: &IrProgram,
    root: &str,
    nodes: &[InfosetNode],
) -> Result<(), String> {
    validate_unparse_infoset_nodes_inner(schema, program, root, nodes, false)
}

fn validate_unparse_infoset_nodes_inner(
    schema: &SchemaDocument,
    program: &IrProgram,
    root: &str,
    nodes: &[InfosetNode],
    enforce_element_form: bool,
) -> Result<(), String> {
    let enforce_element_form =
        enforce_element_form && schema.element_form_default_explicit;
    let qualified = schema.element_form_default_qualified;
    let tns = schema.target_namespace.as_deref();
    let root_node = nodes
        .iter()
        .find(|n| crate::xml_util::local_name_str(&n.name) == root)
        .ok_or_else(|| format!("infoset missing root `{root}`"))?;
    if let Some(ns) = root_node.namespace.as_deref().filter(|u| !u.is_empty()) {
        if tns.is_some_and(|expected| ns != expected) {
            return Err(format!(
                "Schema Definition Error: No global element {{{ns}}}{root}"
            ));
        }
    } else if let Some(ns) = tns.filter(|u| !u.is_empty()) {
        return Err(format!(
            "Unparse Error: expected element start\n{{{ns}}}{root}\n{{}}{root}"
        ));
    }
    let root_parent = parent_qname(root, tns, qualified);
    if enforce_element_form && crate::schema::get_global_element(schema, root).is_some() {
        let local = crate::xml_util::local_name_str(&root_node.name);
        let has_ns = root_node
            .namespace
            .as_deref()
            .is_some_and(|u| !u.is_empty());
        if !has_ns {
            return Err(format!(
                "Unparse Error: {} expected element start, but received start event for {} at {}",
                element_qname_in_errors(local, tns),
                qname_for_error(local, None),
                element_qname_in_errors(local, tns),
            ));
        }
    }
    if enforce_element_form {
        validate_element_form(root_node, qualified, tns, None, &root_parent, true)?;
    }
    let root_id = program.root;
    validate_infoset_particle(
        program,
        root_id,
        root_node,
        qualified,
        tns,
        &root_parent,
        enforce_element_form,
    )?;
    Ok(())
}

fn validate_element_form(
    node: &InfosetNode,
    qualified: bool,
    target_ns: Option<&str>,
    parent_qualified: Option<bool>,
    parent_name: &str,
    required: bool,
) -> Result<(), String> {
    let is_root = parent_qualified.is_none();
    if is_root {
        return Ok(());
    }
    let local = crate::xml_util::local_name_str(&node.name);
    let expect_qualified = parent_qualified.unwrap_or(qualified);
    let has_ns = node.namespace.as_deref().is_some_and(|u| !u.is_empty());
    if expect_qualified == has_ns {
        return Ok(());
    }
    let expected_qname = if expect_qualified {
        element_qname_in_errors(local, target_ns)
    } else {
        qname_for_error(local, None)
    };
    let received_qname = if has_ns {
        element_qname_in_errors(local, target_ns)
    } else {
        qname_for_error(local, None)
    };
    if required {
        return Err(format!(
            "Unparse Error: {expected_qname} expected element start, but received start event for {received_qname} at {parent_name}"
        ));
    }
    Err(format!(
        "Unparse Error: {parent_name} expected element end, but received start event for {received_qname} at {parent_name}"
    ))
}

fn validate_infoset_particle(
    program: &IrProgram,
    node_id: u32,
    node: &InfosetNode,
    qualified: bool,
    tns: Option<&str>,
    parent_name: &str,
    enforce_element_form: bool,
) -> Result<(), String> {
    match program.node(node_id).map_err(|e| e.to_string())? {
        IrNode::Element { name, kind, child, props, .. } => {
            let elem_name = program.strings.get(*name).map_err(|e| e.to_string())?;
            let local = crate::xml_util::local_name_str(elem_name);
            if local != crate::xml_util::local_name_str(&node.name) {
                return Err(format!(
                    "Unparse Error: Expected element start event for {elem_name}, but received element end event for {parent_name}"
                ));
            }
            if node.nil && props.nillable {
                return Ok(());
            }
            if *kind != crate::ir::ValueKind::Complex {
                return Ok(());
            }
            let Some(child_id) = child else {
                return Ok(());
            };
            let parent_label = parent_qname(
                crate::xml_util::local_name_str(elem_name),
                tns,
                qualified,
            );
            validate_infoset_particle(
                program,
                *child_id,
                node,
                qualified,
                tns,
                &parent_label,
                enforce_element_form,
            )?;
            let _ = props;
            Ok(())
        }
        IrNode::Sequence { children, .. } => {
            validate_sequence_children(
                program,
                children,
                node,
                qualified,
                tns,
                parent_name,
                enforce_element_form,
            )?;
            Ok(())
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                let branch_name = program.strings.get(branch.name).map_err(|e| e.to_string())?;
                if !find_infoset_children(node, branch_name).is_empty() {
                    return validate_infoset_particle(
                        program,
                        branch.node,
                        node,
                        qualified,
                        tns,
                        branch_name,
                        enforce_element_form,
                    );
                }
            }
            Err(format!(
                "Unparse Error: infoset does not match any choice branch under `{parent_name}`"
            ))
        }
    }
}

fn validate_sequence_children(
    program: &IrProgram,
    children: &[u32],
    node: &InfosetNode,
    qualified: bool,
    tns: Option<&str>,
    parent_name: &str,
    enforce_element_form: bool,
) -> Result<(), String> {
    for &child_id in children {
        match program.node(child_id).map_err(|e| e.to_string())? {
            IrNode::Sequence { .. } | IrNode::Choice { .. } => {
                validate_infoset_particle(
                    program,
                    child_id,
                    node,
                    qualified,
                    tns,
                    parent_name,
                    enforce_element_form,
                )?;
                continue;
            }
            _ => {}
        }
        let IrNode::Element { name, props, .. } = program.node(child_id).map_err(|e| e.to_string())?
        else {
            continue;
        };
        if props.output_value_calc.is_some() {
            continue;
        }
        let elem_name = program.strings.get(*name).map_err(|e| e.to_string())?;
        let local = crate::xml_util::local_name_str(elem_name);
        let max = props.occurs_max.unwrap_or(u64::MAX);
        let min = props.occurs_min;
        let infoset_children = find_infoset_children(node, local);
        let count = infoset_children.len() as u64;
        if count < min {
            return Err(format!(
                "Unparse Error: Expected element start event for {elem_name}, but received element end event for {parent_name}"
            ));
        }
        if count > max {
            let elem_qname = element_qname_in_errors(local, tns);
            let parent_q = if parent_name.starts_with('{') {
                parent_name.to_string()
            } else {
                element_qname_in_errors(parent_name, tns)
            };
            return Err(format!(
                "Unparse Error: {elem_qname} expected element end, but received start event for {elem_qname} at {parent_q}"
            ));
        }
        for child_node in &infoset_children {
            if enforce_element_form {
                validate_element_form(
                    child_node,
                    qualified,
                    tns,
                    Some(qualified),
                    parent_name,
                    min > 0,
                )?;
            }
            let child_parent = parent_qname(local, tns, qualified);
            validate_infoset_particle(
                program,
                child_id,
                child_node,
                qualified,
                tns,
                &child_parent,
                enforce_element_form,
            )?;
        }
    }
    if enforce_element_form {
        for (key, extra) in &node.children {
            let local = crate::xml_util::local_name_str(key);
            if sequence_allows_child_local(program, children, local) {
                continue;
            }
            if !extra.is_empty() {
                return Err(format!(
                    "Unparse Error: {} expected element end, but received start event for {key} at {parent_name}",
                    element_qname_in_errors(local, if qualified { tns } else { None }),
                ));
            }
        }
    }
    Ok(())
}

fn sequence_allows_child_local(program: &IrProgram, children: &[u32], local: &str) -> bool {
    for &cid in children {
        match program.node(cid).ok() {
            Some(IrNode::Element { name, .. }) => {
                if program
                    .strings
                    .get(*name)
                    .ok()
                    .is_some_and(|n| crate::xml_util::local_name_str(n) == local)
                {
                    return true;
                }
            }
            Some(IrNode::Sequence { children: nested, .. }) => {
                if sequence_allows_child_local(program, nested, local) {
                    return true;
                }
            }
            Some(IrNode::Choice { branches, .. }) => {
                for branch in branches {
                    if sequence_allows_child_local(program, core::slice::from_ref(&branch.node), local)
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn find_infoset_children<'a>(node: &'a InfosetNode, local: &str) -> Vec<&'a InfosetNode> {
    node.children
        .iter()
        .filter(|(k, _)| crate::xml_util::local_name_str(k) == local)
        .flat_map(|(_, v)| v.iter())
        .collect()
}

pub fn validate_unparse_value_map(
    program: &IrProgram,
    node_id: u32,
    map: &BTreeMap<String, DfdlValue>,
    parent_name: &str,
) -> Result<(), VmError> {
    match program.node(node_id).map_err(|e| VmError::InvalidValue { message: e.to_string() })? {
        IrNode::Sequence { children, .. } => {
            for &child_id in children {
                let IrNode::Element { name, props, .. } =
                    program.node(child_id).map_err(|e| VmError::InvalidValue {
                        message: e.to_string(),
                    })?
                else {
                    continue;
                };
                let key = program
                    .strings
                    .get(*name)
                    .map_err(|e| VmError::InvalidValue { message: e.to_string() })?;
                let max = props.occurs_max.unwrap_or(u64::MAX);
                let min = props.occurs_min;
                let count = match map.get(key) {
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
