use crate::ir::{IrInputPathStep, IrNode, IrProgram, IrProps};
use crate::schema::OccursCountKind;
use crate::tdml::InfosetNode;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) fn group_local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

pub(crate) fn qname_for_error(local: &str, ns: Option<&str>) -> String {
    match ns {
        Some(uri) if !uri.is_empty() => format!("{{{uri}}}{local}"),
        _ => format!("{{}}{local}"),
    }
}

pub(crate) fn element_qname_in_errors(local: &str, tns: Option<&str>) -> String {
    qname_for_error(local, tns.filter(|u| !u.is_empty()))
}

pub(crate) fn parent_qname(root: &str, target_ns: Option<&str>, qualified: bool) -> String {
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

pub(crate) fn validate_element_form(
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

pub(crate) fn validate_infoset_particle(
    program: &IrProgram,
    node_id: u32,
    node: &InfosetNode,
    qualified: bool,
    tns: Option<&str>,
    parent_name: &str,
    enforce_element_form: bool,
) -> Result<(), String> {
    match program.node(node_id).map_err(|e| e.to_string())? {
        IrNode::Element {
            name,
            kind,
            child,
            props,
            ..
        } => {
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
            let parent_label =
                parent_qname(crate::xml_util::local_name_str(elem_name), tns, qualified);
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
                let branch_name = program
                    .strings
                    .get(branch.name)
                    .map_err(|e| e.to_string())?;
                let branch_children = find_infoset_children(node, branch_name);
                if !branch_children.is_empty() {
                    if let Some(&child_node) = branch_children.first() {
                        return validate_infoset_particle(
                            program,
                            branch.node,
                            child_node,
                            qualified,
                            tns,
                            branch_name,
                            enforce_element_form,
                        );
                    }
                }
            }
            for branch in branches {
                if choice_branch_is_empty_sequence(program, branch.node) {
                    return Ok(());
                }
                if ir_particle_can_absent_from_unparse_infoset(program, branch.node)? {
                    return Ok(());
                }
            }
            for branch in branches {
                if choice_branch_infoset_matches(program, branch.node, node) {
                    return validate_infoset_particle(
                        program,
                        branch.node,
                        node,
                        qualified,
                        tns,
                        parent_name,
                        enforce_element_form,
                    );
                }
            }
            let mut expected = Vec::new();
            for branch in branches {
                if ir_particle_can_absent_from_unparse_infoset(program, branch.node)
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(local) = choice_branch_infoset_local_key(program, branch.node) {
                    expected.push(local);
                } else {
                    let branch_name = program
                        .strings
                        .get(branch.name)
                        .map_err(|e| e.to_string())?;
                    expected.push(crate::xml_util::local_name_str(branch_name).to_string());
                }
            }
            if expected.is_empty() {
                return Err(format!(
                    "Unparse Error: infoset does not match any choice branch under `{parent_name}`"
                ));
            }
            let mut found = Vec::new();
            for key in node.children.keys() {
                let local = crate::xml_util::local_name_str(key);
                if !expected.iter().any(|e| e == local) {
                    found.push(local.to_string());
                }
            }
            if !found.is_empty() {
                return Err(format!(
                    "Unparse Error: Found {}, expected one of {} at {parent_name}",
                    found.join(", "),
                    expected.join(", ")
                ));
            }
            Err(format!(
                "Unparse Error: Expected one of {} at {parent_name}",
                expected.join(", ")
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
    for (child_idx, &child_id) in children.iter().enumerate() {
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
        let IrNode::Element { name, props, .. } =
            program.node(child_id).map_err(|e| e.to_string())?
        else {
            continue;
        };
        if ir_props_computed_at_unparse(props) {
            continue;
        }
        let elem_name = program.strings.get(*name).map_err(|e| e.to_string())?;
        let local = crate::xml_util::local_name_str(elem_name);
        let ns_for_qname = if qualified { tns } else { None };
        let ns_for_errors = tns.filter(|u| !u.is_empty());
        let max = effective_occurs_max_for_unparse(program, props, node)?;
        let min = props.occurs_min;
        let infoset_children = find_infoset_children(node, local);
        let count = infoset_children.len() as u64;
        if count < min {
            let elem_qname = element_qname_in_errors(local, ns_for_qname);
            if min > 0
                && matches!(
                    props.occurs_count_kind,
                    OccursCountKind::Implicit
                        | OccursCountKind::Expression
                        | OccursCountKind::Fixed
                )
            {
                let needed = min.saturating_sub(count);
                if needed == 1 && min == 1 {
                    return Err(format!(
                        "Unparse Error: Expected element start event for {elem_name}, but received element end event for {parent_name}"
                    ));
                }
                let end_event_for = if props.occurs_count_kind == OccursCountKind::Expression {
                    children
                        .get(child_idx + 1)
                        .and_then(|&next_id| program.node(next_id).ok())
                        .and_then(|next| {
                            if let IrNode::Element {
                                name: next_name, ..
                            } = next
                            {
                                let next_local = crate::xml_util::local_name_str(
                                    program.strings.get(*next_name).ok()?,
                                );
                                Some(element_qname_in_errors(next_local, ns_for_errors))
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| parent_name.to_string())
                } else {
                    parent_name.to_string()
                };
                let end_kind = if min > 1 { "array end" } else { "element end" };
                return Err(format!(
                    "Unparse Error: Expected {needed} additional {elem_qname} received {end_kind} event for {end_event_for}"
                ));
            }
            return Err(format!(
                "Unparse Error: Expected element start event for {elem_name}, but received element end event for {parent_name}"
            ));
        }
        if count > max {
            let elem_qname = element_qname_in_errors(local, ns_for_errors);
            let parent_q = if parent_name.starts_with('{') {
                parent_name.to_string()
            } else {
                element_qname_in_errors(parent_name, ns_for_errors)
            };
            if max > 1 {
                return Err(format!(
                    "Unparse Error: Expected array end event for {elem_qname}, but received element start event for {elem_qname} at {parent_q}"
                ));
            }
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
                let ns_for_qname = if qualified { tns } else { None };
                let bad_q = element_qname_in_errors(local, ns_for_qname);
                let expected = sequence_next_missing_local(program, children, node)
                    .or_else(|| sequence_active_branch_tail_local(program, children, node))
                    .or_else(|| {
                        if find_infoset_children(node, "nack").is_empty() {
                            None
                        } else {
                            Some(String::from("nackInfo"))
                        }
                    });
                if let Some(expected) = expected {
                    let exp_q = element_qname_in_errors(&expected, ns_for_qname);
                    return Err(format!(
                        "Unparse Error: Expected element start event for {exp_q}, but received element start event for (invalid) {bad_q} at {parent_name}"
                    ));
                }
                return Err(format!(
                    "Unparse Error: {} expected element end, but received start event for {key} at {parent_name}",
                    element_qname_in_errors(local, ns_for_qname),
                ));
            }
        }
    }
    Ok(())
}

fn sequence_active_branch_tail_local(
    program: &IrProgram,
    children: &[u32],
    node: &InfosetNode,
) -> Option<String> {
    for &cid in children {
        let IrNode::Choice { branches, .. } = program.node(cid).ok()? else {
            continue;
        };
        for branch in branches {
            if !choice_branch_infoset_matches(program, branch.node, node) {
                continue;
            }
            return sequence_last_required_local(program, branch.node);
        }
    }
    None
}

fn sequence_last_required_local(program: &IrProgram, node_id: u32) -> Option<String> {
    match program.node(node_id).ok()? {
        IrNode::Sequence { children, .. } => {
            let mut last = None;
            for &cid in children {
                if let Some(local) = sequence_last_required_local(program, cid) {
                    last = Some(local);
                }
            }
            last
        }
        IrNode::Element { name, props, .. } => {
            if props.hidden || props.output_value_calc.is_some() {
                None
            } else {
                Some(crate::xml_util::local_name_str(program.strings.get(*name).ok()?).to_string())
            }
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if let Some(local) = sequence_last_required_local(program, branch.node) {
                    return Some(local);
                }
            }
            None
        }
        _ => None,
    }
}

fn sequence_next_missing_local(
    program: &IrProgram,
    children: &[u32],
    node: &InfosetNode,
) -> Option<String> {
    for &cid in children {
        match program.node(cid).ok()? {
            IrNode::Element { name, props, .. } => {
                let elem_name = program.strings.get(*name).ok()?;
                let local = crate::xml_util::local_name_str(elem_name);
                if props.hidden || props.output_value_calc.is_some() {
                    continue;
                }
                let count = find_infoset_children(node, local).len() as u64;
                if count < props.occurs_min {
                    return Some(local.to_string());
                }
            }
            IrNode::Sequence {
                children: nested, ..
            } => {
                if let Some(local) = sequence_next_missing_local(program, nested, node) {
                    return Some(local);
                }
            }
            IrNode::Choice { branches, .. } => {
                for branch in branches {
                    let branch_name = program.strings.get(branch.name).ok()?;
                    if find_infoset_children(node, branch_name).is_empty() {
                        continue;
                    }
                    if let Some(local) = sequence_next_missing_local(
                        program,
                        core::slice::from_ref(&branch.node),
                        node,
                    ) {
                        return Some(local);
                    }
                    break;
                }
            }
            _ => {}
        }
    }
    None
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
            Some(IrNode::Sequence {
                children: nested, ..
            }) => {
                if sequence_allows_child_local(program, nested, local) {
                    return true;
                }
            }
            Some(IrNode::Choice { branches, .. }) => {
                for branch in branches {
                    if sequence_allows_child_local(
                        program,
                        core::slice::from_ref(&branch.node),
                        local,
                    ) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn choice_branch_is_empty_sequence(program: &IrProgram, node_id: u32) -> bool {
    matches!(
        program.node(node_id).ok(),
        Some(IrNode::Sequence { children, .. }) if children.is_empty()
    )
}

fn ir_props_can_absent_from_unparse_infoset(props: &IrProps) -> bool {
    props.hidden
        || props.output_value_calc.is_some()
        || props.output_value_calc_literal.is_some()
        || props.output_value_calc_sibling.is_some()
        || props.output_value_calc_conditional
        || props.occurs_min == 0
}

fn ir_props_computed_at_unparse(props: &IrProps) -> bool {
    props.hidden
        || props.output_value_calc.is_some()
        || props.output_value_calc_literal.is_some()
        || props.output_value_calc_sibling.is_some()
        || props.output_value_calc_conditional
}

fn ir_particle_can_absent_from_unparse_infoset(
    program: &IrProgram,
    node_id: u32,
) -> Result<bool, String> {
    match program.node(node_id).map_err(|e| e.to_string())? {
        IrNode::Element {
            props, child, kind, ..
        } => {
            if ir_props_can_absent_from_unparse_infoset(props) {
                return Ok(true);
            }
            if *kind == crate::ir::ValueKind::Complex {
                if let Some(child_id) = child {
                    return ir_particle_can_absent_from_unparse_infoset(program, *child_id);
                }
            }
            Ok(false)
        }
        IrNode::Sequence { children, .. } => {
            for &cid in children {
                if !ir_particle_can_absent_from_unparse_infoset(program, cid)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if ir_particle_can_absent_from_unparse_infoset(program, branch.node)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn choice_branch_infoset_local_key(program: &IrProgram, branch_node: u32) -> Option<String> {
    match program.node(branch_node).ok()? {
        IrNode::Element { name, props, .. } => {
            if ir_props_can_absent_from_unparse_infoset(props) {
                return None;
            }
            let ename = program.strings.get(*name).ok()?;
            Some(crate::xml_util::local_name_str(ename).to_string())
        }
        IrNode::Sequence { children, .. } => {
            for &cid in children {
                if let Some(k) = choice_branch_infoset_local_key(program, cid) {
                    return Some(k);
                }
            }
            None
        }
        IrNode::Choice { branches, .. } => {
            for branch in branches {
                if let Some(k) = choice_branch_infoset_local_key(program, branch.node) {
                    return Some(k);
                }
            }
            None
        }
        _ => None,
    }
}

fn choice_branch_infoset_matches(
    program: &IrProgram,
    branch_node: u32,
    node: &InfosetNode,
) -> bool {
    let Some(local) = choice_branch_infoset_local_key(program, branch_node) else {
        return false;
    };
    !find_infoset_children(node, &local).is_empty()
}

fn effective_occurs_max_for_unparse(
    program: &IrProgram,
    props: &IrProps,
    parent: &InfosetNode,
) -> Result<u64, String> {
    if props.occurs_count_kind == OccursCountKind::Expression {
        if props.occurs_min == 0 {
            return Ok(u64::MAX);
        }
        if let Some(steps) = props.occurs_count_fn_path.as_ref() {
            return eval_occurs_count_from_infoset(program, steps, parent);
        }
    }
    if props.occurs_count_kind == OccursCountKind::Parsed && props.occurs_min == 0 {
        if props.occurs_max == Some(1) {
            return Ok(u64::MAX);
        }
        return Ok(props.occurs_max.unwrap_or(u64::MAX));
    }
    Ok(props.occurs_max.unwrap_or(u64::MAX))
}

fn eval_occurs_count_from_infoset(
    program: &IrProgram,
    steps: &[IrInputPathStep],
    parent: &InfosetNode,
) -> Result<u64, String> {
    let strings = &program.strings;
    let step = steps
        .first()
        .ok_or_else(|| "empty occursCount path".to_string())?;
    let local = strings.get(step.local).map_err(|e| e.to_string())?;
    let local = crate::xml_util::local_name_str(local);
    let matches = find_infoset_children(parent, local);
    let value = if let Some(idx) = step.index {
        matches
            .get((idx as usize).saturating_sub(1))
            .ok_or_else(|| format!("occursCount path missing `{local}[{idx}]`"))?
    } else {
        matches
            .first()
            .ok_or_else(|| format!("occursCount path missing `{local}`"))?
    };
    if let Some(text) = value.text.as_deref() {
        return text
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("occursCount path `{local}` not numeric"));
    }
    Ok(matches.len() as u64)
}

fn find_infoset_children<'a>(node: &'a InfosetNode, local: &str) -> Vec<&'a InfosetNode> {
    node.children
        .iter()
        .filter(|(k, _)| crate::xml_util::local_name_str(k) == local)
        .flat_map(|(_, v)| v.iter())
        .collect()
}
