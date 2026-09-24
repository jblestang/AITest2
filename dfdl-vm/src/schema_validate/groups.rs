use super::types::enqueue_particle_types;
use crate::error::SchemaError;
use crate::schema::{
    BuiltinType, ComplexContent, GroupDecl, LengthKind, Particle, SchemaDocument, SequenceKind,
    TypeDef,
};
use alloc::collections::{BTreeSet, VecDeque};

const HIDDEN_GROUP_REF_CANNOT_HAVE_CHILDREN: &str =
    "Schema Definition Error: A sequence with hiddenGroupRef cannot have children.";

pub(crate) fn sequence_hidden_group_ref_notation_ok(
    schema: &SchemaDocument,
    seq: &crate::schema::SequenceDecl,
) -> Result<(), SchemaError> {
    if seq.props.hidden_group_ref.is_some()
        && (seq.props.hidden_group_ref_from_appinfo_sequence
            || !seq.particles.is_empty()
            || seq.had_markup_before_particles)
    {
        return Err(SchemaError::InvalidProperty {
            message: HIDDEN_GROUP_REF_CANNOT_HAVE_CHILDREN.into(),
        });
    }
    for particle in &seq.particles {
        validate_particle_hidden_group_ref_notation(schema, particle)?;
    }
    Ok(())
}

fn validate_particle_hidden_group_ref_notation(
    schema: &SchemaDocument,
    particle: &Particle,
) -> Result<(), SchemaError> {
    match particle {
        Particle::Sequence(seq) => sequence_hidden_group_ref_notation_ok(schema, seq),
        Particle::Choice(ch) => {
            for branch in &ch.branches {
                validate_particle_hidden_group_ref_notation(schema, branch)?;
            }
            Ok(())
        }
        Particle::GroupRef(gr) => {
            let local = gr.name.rsplit(':').next().unwrap_or(gr.name.as_str());
            match schema.groups.get(local) {
                Some(GroupDecl::Sequence(seq)) => {
                    for p in &seq.particles {
                        validate_particle_hidden_group_ref_notation(schema, p)?;
                    }
                }
                Some(GroupDecl::Choice(ch)) => {
                    for branch in &ch.branches {
                        validate_particle_hidden_group_ref_notation(schema, branch)?;
                    }
                }
                None => {}
            }
            Ok(())
        }
        Particle::Element(_) => Ok(()),
    }
}

pub(crate) fn validate_hidden_group_ref_notation(
    schema: &SchemaDocument,
) -> Result<(), SchemaError> {
    for td in schema.types.values() {
        if let TypeDef::Complex { content, .. } = td {
            if let ComplexContent::Sequence(seq) = content {
                sequence_hidden_group_ref_notation_ok(schema, seq)?;
            }
        }
    }
    for group in schema.groups.values() {
        match group {
            GroupDecl::Sequence(seq) => sequence_hidden_group_ref_notation_ok(schema, seq)?,
            GroupDecl::Choice(ch) => {
                for branch in &ch.branches {
                    validate_particle_hidden_group_ref_notation(schema, branch)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_group_definitions_no_hidden_group_ref(
    schema: &SchemaDocument,
) -> Result<(), SchemaError> {
    for group in schema.groups.values() {
        if let GroupDecl::Sequence(seq) = group {
            if seq.props.hidden_group_ref.is_some() {
                return Err(SchemaError::InvalidProperty {
                    message: "Schema Definition Error: the model group of a group definition cannot be a sequence with dfdl:hiddenGroupRef".into(),
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_reachable_complex_type_model_groups(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    const NO_MODEL_GROUP: &str = "Schema Definition Error: A complex type must have exactly one model-group element child which is a sequence, choice, or group reference.";
    let root_implicit_length = crate::schema::get_global_element(schema, root)
        .and_then(|ge| ge.props.length_kind)
        == Some(LengthKind::Implicit);
    let mut seen = BTreeSet::new();
    let mut type_queue = VecDeque::new();
    if let Some(ge) = crate::schema::get_global_element(schema, root) {
        if BuiltinType::from_xsd(ge.type_name.as_str()).is_none() {
            type_queue.push_back(ge.type_name.clone());
        }
    }
    while let Some(tn) = type_queue.pop_front() {
        if !seen.insert(tn.clone()) {
            continue;
        }
        let Some(td) = schema.resolve_type(&tn) else {
            continue;
        };
        let TypeDef::Complex { content, .. } = td else {
            continue;
        };
        match content {
            ComplexContent::Empty => {
                return Err(SchemaError::InvalidProperty {
                    message: NO_MODEL_GROUP.into(),
                });
            }
            ComplexContent::Sequence(seq)
                if seq.particles.is_empty() && seq.props.hidden_group_ref.is_none() =>
            {
                if seq.props.sequence_kind == Some(SequenceKind::Unordered) {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Unordered sequences must not be empty"
                            .into(),
                    });
                }
                if !tn.as_str().starts_with("__inline_") && !root_implicit_length {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error".into(),
                    });
                }
            }
            _ => {}
        }
        enqueue_particle_types(schema, content, &mut type_queue);
    }
    Ok(())
}
