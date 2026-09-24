use crate::error::SchemaError;
use crate::schema::{ComplexContent, GroupDecl, Particle, SchemaDocument, TypeDef};
use alloc::collections::{BTreeSet, VecDeque};
use alloc::string::String;

fn group_local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

fn choice_dispatch_active(props: &crate::schema::DfdlProps) -> bool {
    props.choice_dispatch_sibling.is_some()
        || props.choice_dispatch_path.is_some()
        || props.choice_dispatch_literal.is_some()
        || props.choice_dispatch_sibling_int.is_some()
}

fn particle_element_name(particle: &Particle) -> Option<String> {
    match particle {
        Particle::Element(e) => Some(e.name.clone()),
        Particle::GroupRef(gr) => Some(gr.name.clone()),
        Particle::Sequence(_) | Particle::Choice(_) => None,
    }
}

fn particle_branch_key(schema: &SchemaDocument, particle: &Particle) -> Option<String> {
    match particle {
        Particle::Element(e) => e.props.choice_branch_key.clone(),
        Particle::Sequence(s) => s.props.choice_branch_key.clone(),
        Particle::Choice(c) => c.props.choice_branch_key.clone(),
        Particle::GroupRef(gr) => gr
            .props
            .choice_branch_key
            .clone()
            .or_else(|| group_declaration_branch_key(schema, &gr.name)),
    }
}

fn group_declaration_branch_key(schema: &SchemaDocument, qname: &str) -> Option<String> {
    let group = schema.groups.get(group_local_name(qname))?;
    match group {
        GroupDecl::Sequence(s) => s.props.choice_branch_key.clone(),
        GroupDecl::Choice(c) => c.props.choice_branch_key.clone(),
    }
}

pub(crate) fn validate_choice_dispatch_schema(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let mut queue = VecDeque::new();
    if let Some(ge) = crate::schema::get_global_element(schema, root) {
        if let Some(td) = schema.resolve_type(&ge.type_name) {
            if let TypeDef::Complex { content, .. } = td {
                enqueue_complex_content_for_discriminator_walk(schema, content, &mut queue)?;
            }
        }
    }
    while let Some(particles) = queue.pop_front() {
        for p in &particles {
            match p {
                Particle::Choice(ch) => validate_dispatch_choice(schema, ch)?,
                Particle::Sequence(seq) => queue.push_back(seq.particles.clone()),
                Particle::Element(el) => {
                    if let Some(td) = schema.resolve_type(&el.type_name) {
                        if let TypeDef::Complex { content, .. } = td {
                            enqueue_complex_content_for_discriminator_walk(
                                schema, content, &mut queue,
                            )?;
                        }
                    }
                }
                Particle::GroupRef(gr) => {
                    if let Some(group) = schema.groups.get(group_local_name(&gr.name)) {
                        match group {
                            GroupDecl::Sequence(s) => queue.push_back(s.particles.clone()),
                            GroupDecl::Choice(c) => queue.push_back(c.branches.clone()),
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_dispatch_branch_particle(
    schema: &SchemaDocument,
    particle: &Particle,
) -> Result<(), SchemaError> {
    if let Particle::Element(el) = particle {
        if let Some(ref er) = el.element_ref {
            if el.props.choice_branch_key.is_none() {
                if let Some(g) = crate::schema::get_global_element(schema, er) {
                    if g.props.choice_branch_key.is_some() {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: The dfdl:choiceBranchKey property must not be defined on a global element."
                                .into(),
                        });
                    }
                }
            }
        }
    }
    if let Particle::GroupRef(gr) = particle {
        if gr.props.choice_branch_key.is_none() {
            validate_group_ref_definition_branch_keys(schema, &gr.name)?;
        }
    }
    Ok(())
}

fn validate_group_ref_definition_branch_keys(
    schema: &SchemaDocument,
    qname: &str,
) -> Result<(), SchemaError> {
    let Some(group) = schema.groups.get(group_local_name(qname)) else {
        return Ok(());
    };
    match group {
        GroupDecl::Sequence(seq) => {
            if seq.props.choice_branch_key.is_some() {
                return Err(global_group_branch_key_sde());
            }
            for p in &seq.particles {
                validate_group_def_particle_branch_keys(p)?;
            }
        }
        GroupDecl::Choice(ch) => {
            if ch.props.choice_branch_key.is_some() {
                return Err(global_group_branch_key_sde());
            }
            for p in &ch.branches {
                validate_group_def_particle_branch_keys(p)?;
            }
        }
    }
    Ok(())
}

fn validate_group_def_particle_branch_keys(particle: &Particle) -> Result<(), SchemaError> {
    match particle {
        Particle::Element(el) => {
            if el.props.choice_branch_key.is_some() {
                return Err(global_group_branch_key_sde());
            }
            Ok(())
        }
        Particle::Sequence(seq) => {
            if seq.props.choice_branch_key.is_some() {
                return Err(global_group_branch_key_sde());
            }
            for p in &seq.particles {
                validate_group_def_particle_branch_keys(p)?;
            }
            Ok(())
        }
        Particle::Choice(ch) => {
            if ch.props.choice_branch_key.is_some() {
                return Err(global_group_branch_key_sde());
            }
            for p in &ch.branches {
                validate_group_def_particle_branch_keys(p)?;
            }
            Ok(())
        }
        Particle::GroupRef(_) => Ok(()),
    }
}

fn global_group_branch_key_sde() -> SchemaError {
    SchemaError::InvalidProperty {
        message: "Schema Definition Error: The dfdl:choiceBranchKey property must not be defined on the child of a global group definition's sequence or choice."
            .into(),
    }
}

fn validate_dispatch_choice(
    schema: &SchemaDocument,
    ch: &crate::schema::ChoiceDecl,
) -> Result<(), SchemaError> {
    let props = &ch.props;
    if choice_dispatch_active(props) && props.initiated_content == Some(true) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: The dfdl:initiatedContent and dfdl:choiceDispatchKey properties must not both be defined on the same choice."
                .into(),
        });
    }
    if !choice_dispatch_active(props) {
        return Ok(());
    }
    let mut seen = BTreeSet::new();
    for branch in &ch.branches {
        validate_dispatch_branch_particle(schema, branch)?;
        let Some(key) = particle_branch_key(schema, branch) else {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: When dfdl:choiceDispatchKey is defined, every branch must have a dfdl:choiceBranchKey or dfdl:choiceBranchKeyRanges property defined."
                    .into(),
            });
        };
        if !seen.insert(key.clone()) {
            let branch_names: alloc::vec::Vec<String> = ch
                .branches
                .iter()
                .filter_map(particle_element_name)
                .collect();
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: dfdl:choiceBranchKey value `{key}` is not unique among the branches of the choice. Branches: {}",
                    branch_names.join(", ")
                ),
            });
        }
    }
    Ok(())
}

fn enqueue_complex_content_for_discriminator_walk(
    _schema: &SchemaDocument,
    content: &ComplexContent,
    queue: &mut VecDeque<alloc::vec::Vec<Particle>>,
) -> Result<(), SchemaError> {
    match content {
        ComplexContent::Empty => Ok(()),
        ComplexContent::Sequence(s) => {
            queue.push_back(s.particles.clone());
            Ok(())
        }
        ComplexContent::Choice(c) => {
            queue.push_back(c.branches.clone());
            Ok(())
        }
    }
}
