use crate::error::VmError;
use crate::ir::{IrNode, IrProgram, IrProps};
use crate::schema::{
    BuiltinType, ComplexContent, DfdlProps, ElementDecl, GroupDecl, Particle, SchemaDocument,
    TypeDef,
};
use crate::tdml::InfosetNode;
use crate::value::DfdlValue;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

fn group_local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

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
    validate_hidden_groups_unparse(schema, root)?;
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

/// Hidden group refs require every descendant to be optional, defaultable, or have OVC (Daffodil unparse compile).
pub fn validate_hidden_groups_unparse(schema: &SchemaDocument, root: &str) -> Result<(), String> {
    let Some(ge) = crate::schema::get_global_element(schema, root) else {
        return Ok(());
    };
    if BuiltinType::from_xsd(ge.type_name.as_str()).is_some() {
        return Ok(());
    }
    let Some(type_def) = schema.resolve_type(&ge.type_name) else {
        return Ok(());
    };
    let TypeDef::Complex { content, .. } = type_def else {
        return Ok(());
    };
    let mut hidden_targets = BTreeSet::new();
    let mut collect_visited = BTreeSet::new();
    collect_hidden_group_refs_in_content(schema, content, &mut hidden_targets, &mut collect_visited);
    for target in hidden_targets {
        let local = group_local_name(&target);
        if local.is_empty() {
            continue;
        }
        validate_hidden_group_model(schema, local, &target, &mut Vec::new())?;
    }
    Ok(())
}

fn collect_hidden_group_refs_in_content(
    schema: &SchemaDocument,
    content: &ComplexContent,
    out: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) {
    match content {
        ComplexContent::Sequence(seq) => {
            if let Some(ref href) = seq.props.hidden_group_ref {
                out.insert(href.clone());
                collect_hidden_group_particles(schema, group_local_name(href), out, visited);
            }
            for p in &seq.particles {
                collect_hidden_group_refs_in_particle(schema, p, out, visited);
            }
        }
        ComplexContent::Choice(ch) => {
            for p in &ch.branches {
                collect_hidden_group_refs_in_particle(schema, p, out, visited);
            }
        }
        ComplexContent::Empty => {}
    }
}

fn collect_hidden_group_particles(
    schema: &SchemaDocument,
    local: &str,
    out: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) {
    if local.is_empty() || !visited.insert(local.to_string()) {
        return;
    }
    if let Some(GroupDecl::Sequence(gseq)) = schema.groups.get(local) {
        for p in &gseq.particles {
            collect_hidden_group_refs_in_particle(schema, p, out, visited);
        }
    }
    visited.remove(local);
}

fn collect_hidden_group_refs_in_particle(
    schema: &SchemaDocument,
    particle: &Particle,
    out: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) {
    match particle {
        Particle::Element(el) => {
            if BuiltinType::from_xsd(el.type_name.as_str()).is_some() {
                return;
            }
            if let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&el.type_name) {
                collect_hidden_group_refs_in_content(schema, content, out, visited);
            }
        }
        Particle::Sequence(s) => {
            if let Some(ref href) = s.props.hidden_group_ref {
                out.insert(href.clone());
                collect_hidden_group_particles(schema, group_local_name(href), out, visited);
            }
            for p in &s.particles {
                collect_hidden_group_refs_in_particle(schema, p, out, visited);
            }
        }
        Particle::Choice(c) => {
            for p in &c.branches {
                collect_hidden_group_refs_in_particle(schema, p, out, visited);
            }
        }
        Particle::GroupRef(gr) => {
            let local = gr.name.rsplit(':').next().unwrap_or(gr.name.as_str());
            if let Some(group) = schema.groups.get(local) {
                match group {
                    GroupDecl::Sequence(s) => {
                        for p in &s.particles {
                            collect_hidden_group_refs_in_particle(schema, p, out, visited);
                        }
                    }
                    GroupDecl::Choice(c) => {
                        for p in &c.branches {
                            collect_hidden_group_refs_in_particle(schema, p, out, visited);
                        }
                    }
                }
            }
        }
    }
}

fn validate_hidden_group_model(
    schema: &SchemaDocument,
    group_name: &str,
    group_ref_qname: &str,
    stack: &mut Vec<String>,
) -> Result<(), String> {
    if stack.iter().any(|s| s == group_name) {
        return Err(
            "Schema Definition Error: Model group circular definitions. Group references, or hidden group references form a loop.".into(),
        );
    }
    stack.push(group_name.to_string());
    let group = schema.groups.get(group_name).ok_or_else(|| {
        format!(
            "Schema Definition Error: Referenced group definition not found: {group_ref_qname}\nSchema context: group reference\n{group_name}"
        )
    })?;
    let particles = match group {
        GroupDecl::Sequence(s) => s.particles.as_slice(),
        GroupDecl::Choice(c) => {
            let any_ok = c
                .branches
                .iter()
                .any(|p| particle_can_unparse_if_no_events(schema, p));
            stack.pop();
            if any_ok {
                return Ok(());
            }
            let labels: Vec<String> = c
                .branches
                .iter()
                .filter_map(|p| particle_display(schema, p))
                .collect();
            return Err(format!(
                "Schema Definition Error: At least one branch of hidden choice must be fully defaultable or define dfdl:outputValueCalc:\n{}",
                labels.join("\n")
            ));
        }
    };
    let mut failures = Vec::new();
    for particle in particles {
        if let Particle::Sequence(s) = particle {
            if let Some(ref href) = s.props.hidden_group_ref {
                let nested = group_local_name(href);
                validate_hidden_group_model(schema, &nested, href, stack)?;
                continue;
            }
        }
        if !particle_can_unparse_if_no_events(schema, particle) {
            if let Some(label) = particle_display(schema, particle) {
                failures.push(label);
            }
        }
    }
    stack.pop();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Schema Definition Error: Element(s) of hidden group must define dfdl:outputValueCalc, be defaultable or be optional:\n{}",
            failures.join("\n")
        ))
    }
}

fn particle_display(_schema: &SchemaDocument, particle: &Particle) -> Option<String> {
    match particle {
        Particle::Element(el) => Some(format!("ex:{}", el.name)),
        Particle::GroupRef(gr) => Some(gr.name.clone()),
        _ => None,
    }
}

fn can_be_absent_from_unparse_infoset(props: &DfdlProps) -> bool {
    props.occurs_min.unwrap_or(1) == 0
        || props.output_value_calc.is_some()
        || props.output_value_calc_literal.is_some()
        || props.output_value_calc_sibling.is_some()
        || props.output_value_calc_conditional
        || props
            .default_value
            .as_ref()
            .is_some_and(|s| !s.is_empty())
}

fn particle_can_unparse_if_no_events(schema: &SchemaDocument, particle: &Particle) -> bool {
    match particle {
        Particle::Element(el) => element_can_unparse_if_no_events(schema, el),
        Particle::Sequence(s) => s
            .particles
            .iter()
            .all(|p| particle_can_unparse_if_no_events(schema, p)),
        Particle::Choice(c) => c
            .branches
            .iter()
            .any(|p| particle_can_unparse_if_no_events(schema, p)),
        Particle::GroupRef(gr) => {
            let local = gr.name.rsplit(':').next().unwrap_or(gr.name.as_str());
            schema.groups.get(local).is_some_and(|group| match group {
                GroupDecl::Sequence(s) => s
                    .particles
                    .iter()
                    .all(|p| particle_can_unparse_if_no_events(schema, p)),
                GroupDecl::Choice(c) => c
                    .branches
                    .iter()
                    .any(|p| particle_can_unparse_if_no_events(schema, p)),
            })
        }
    }
}

fn resolve_type_def<'a>(
    schema: &'a SchemaDocument,
    type_name: &crate::schema::TypeName,
) -> Option<&'a TypeDef> {
    if let Some(td) = schema.resolve_type(type_name) {
        return Some(td);
    }
    let s = type_name.as_str();
    let local = s.rsplit(':').next().unwrap_or(s);
    for cand in [local, &alloc::format!("ex:{local}"), s] {
        if cand == s {
            continue;
        }
        if let Some(td) = schema.resolve_type(&crate::schema::TypeName::new(cand)) {
            return Some(td);
        }
    }
    None
}

fn element_can_unparse_if_no_events(schema: &SchemaDocument, el: &ElementDecl) -> bool {
    let props = &el.props;
    if can_be_absent_from_unparse_infoset(props) {
        return true;
    }
    if BuiltinType::from_xsd(el.type_name.as_str()).is_some() {
        return false;
    }
    if let Some(TypeDef::Complex { content, .. }) = resolve_type_def(schema, &el.type_name) {
        return complex_content_can_unparse_if_no_events(schema, content);
    }
    false
}

fn complex_content_can_unparse_if_no_events(
    schema: &SchemaDocument,
    content: &ComplexContent,
) -> bool {
    match content {
        ComplexContent::Sequence(s) => s
            .particles
            .iter()
            .all(|p| particle_can_unparse_if_no_events(schema, p)),
        ComplexContent::Choice(c) => c
            .branches
            .iter()
            .any(|p| particle_can_unparse_if_no_events(schema, p)),
        ComplexContent::Empty => true,
    }
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
    let mut has_ns = node.namespace.as_deref().is_some_and(|u| !u.is_empty());
    if !expect_qualified {
        // Unqualified elementFormDefault: infoset may still attach the target namespace URI to locals.
        has_ns = false;
    }
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
                let branch_children = find_infoset_children(node, branch_name);
                if !branch_children.is_empty() {
                    for child_node in branch_children {
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
                if ir_particle_can_absent_from_unparse_infoset(program, branch.node).unwrap_or(false)
                {
                    continue;
                }
                if let Some(local) = choice_branch_infoset_local_key(program, branch.node) {
                    expected.push(local);
                } else {
                    let branch_name = program.strings.get(branch.name).map_err(|e| e.to_string())?;
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
        if props.hidden || props.output_value_calc.is_some() {
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
                Some(
                    crate::xml_util::local_name_str(program.strings.get(*name).ok()?).to_string(),
                )
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
            IrNode::Sequence { children: nested, .. } => {
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

fn ir_particle_can_absent_from_unparse_infoset(
    program: &IrProgram,
    node_id: u32,
) -> Result<bool, String> {
    match program.node(node_id).map_err(|e| e.to_string())? {
        IrNode::Element {
            props,
            child,
            kind,
            ..
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
