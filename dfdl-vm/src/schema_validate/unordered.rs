use super::types::enqueue_particle_types;
use crate::error::SchemaError;
use crate::schema::{ComplexContent, Particle, SchemaDocument, SequenceKind, TypeDef};
use alloc::collections::{BTreeSet, VecDeque};

pub(crate) fn validate_reachable_unordered_sequences(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let mut type_queue = VecDeque::new();
    let mut seen = BTreeSet::new();
    if let Some(ge) = crate::schema::get_global_element(schema, root) {
        if crate::schema::BuiltinType::from_xsd(ge.type_name.as_str()).is_none() {
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
        validate_complex_content_unordered(schema, content)?;
        enqueue_particle_types(schema, content, &mut type_queue);
    }
    Ok(())
}

fn validate_complex_content_unordered(
    schema: &SchemaDocument,
    content: &ComplexContent,
) -> Result<(), SchemaError> {
    match content {
        ComplexContent::Sequence(seq)
            if seq.props.sequence_kind == Some(SequenceKind::Unordered) =>
        {
            validate_unordered_sequence_particles(schema, &seq.particles)?;
        }
        ComplexContent::Sequence(seq) => {
            for p in &seq.particles {
                validate_particle_unordered(schema, p)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_particle_unordered(
    schema: &SchemaDocument,
    particle: &Particle,
) -> Result<(), SchemaError> {
    match particle {
        Particle::Sequence(seq) if seq.props.sequence_kind == Some(SequenceKind::Unordered) => {
            validate_unordered_sequence_particles(schema, &seq.particles)
        }
        Particle::Sequence(seq) => {
            for p in &seq.particles {
                validate_particle_unordered(schema, p)?;
            }
            Ok(())
        }
        Particle::Choice(ch) => {
            for b in &ch.branches {
                validate_particle_unordered(schema, b)?;
            }
            Ok(())
        }
        Particle::GroupRef(gr) => {
            let local = gr.name.rsplit(':').next().unwrap_or(gr.name.as_str());
            if let Some(group) = schema.groups.get(local) {
                match group {
                    crate::schema::GroupDecl::Sequence(seq)
                        if seq.props.sequence_kind == Some(SequenceKind::Unordered) =>
                    {
                        validate_unordered_sequence_particles(schema, &seq.particles)
                    }
                    crate::schema::GroupDecl::Sequence(seq) => {
                        for p in &seq.particles {
                            validate_particle_unordered(schema, p)?;
                        }
                        Ok(())
                    }
                    crate::schema::GroupDecl::Choice(ch) => {
                        for b in &ch.branches {
                            validate_particle_unordered(schema, b)?;
                        }
                        Ok(())
                    }
                }
            } else {
                Ok(())
            }
        }
        Particle::Element(_) => Ok(()),
    }
}

fn validate_unordered_sequence_particles(
    schema: &SchemaDocument,
    particles: &[Particle],
) -> Result<(), SchemaError> {
    use crate::schema::OccursCountKind;
    let mut names: alloc::collections::BTreeMap<alloc::string::String, usize> =
        alloc::collections::BTreeMap::new();
    for p in particles {
        let Particle::Element(el) = p else {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: Member of an unordered sequence must be an element declaration or element reference".into(),
            });
        };
        let min = el.props.occurs_min.unwrap_or(1);
        let max = el.props.occurs_max;
        let is_optional_or_array = min == 0 || max.map(|m| m > 1).unwrap_or(false);
        if is_optional_or_array {
            match el.props.occurs_count_kind {
                Some(OccursCountKind::Parsed) | None => {}
                _ => {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Member of an unordered sequence that is an optional or array element must have dfdl:occursCountKind='parsed'".into(),
                    });
                }
            }
        }
        let key = el.element_ref.clone().unwrap_or_else(|| el.name.clone());
        *names.entry(key).or_insert(0) += 1;
        let _ = schema;
    }
    for (name, count) in names {
        if count > 1 {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: Two or more members of an unordered sequence have the same name and the same namespace ({name})"
                ),
            });
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn validate_reachable_assert_path_indexing(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let mut type_queue = VecDeque::new();
    let mut seen = BTreeSet::new();
    if let Some(ge) = crate::schema::get_global_element(schema, root) {
        if crate::schema::BuiltinType::from_xsd(ge.type_name.as_str()).is_none() {
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
        validate_content_assert_indexing(content)?;
        enqueue_particle_types(schema, content, &mut type_queue);
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_content_assert_indexing(content: &ComplexContent) -> Result<(), SchemaError> {
    if let ComplexContent::Sequence(seq) = content {
        for p in &seq.particles {
            validate_particle_assert_indexing(p)?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_particle_assert_indexing(particle: &Particle) -> Result<(), SchemaError> {
    match particle {
        Particle::Element(el) => {
            if let Some(test) = &el.props.discriminator_test {
                if assert_uses_scalar_path_index(test) {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Indexing is only allowed on arrays"
                            .into(),
                    });
                }
            }
            Ok(())
        }
        Particle::Sequence(seq) => {
            for p in &seq.particles {
                validate_particle_assert_indexing(p)?;
            }
            Ok(())
        }
        Particle::Choice(ch) => {
            for b in &ch.branches {
                validate_particle_assert_indexing(b)?;
            }
            Ok(())
        }
        Particle::GroupRef(_) => Ok(()),
    }
}

#[allow(dead_code)]
fn assert_uses_scalar_path_index(test: &str) -> bool {
    let inner = test
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(test);
    for token in
        inner.split(|c: char| !c.is_ascii_alphanumeric() && c != ':' && c != '[' && c != ']')
    {
        if let Some(open) = token.find('[') {
            if token[open + 1..]
                .strip_suffix(']')
                .and_then(|s| s.parse::<u32>().ok())
                .is_some()
            {
                return true;
            }
        }
    }
    false
}
