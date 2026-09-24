use crate::schema::{
    BuiltinType, ComplexContent, DfdlProps, ElementDecl, GroupDecl, Particle, SchemaDocument,
    TypeDef,
};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

fn group_local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
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
    collect_hidden_group_refs_in_content(
        schema,
        content,
        &mut hidden_targets,
        &mut collect_visited,
    );
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
                validate_hidden_group_model(schema, nested, href, stack)?;
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
        || props.default_value.as_ref().is_some_and(|s| !s.is_empty())
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
