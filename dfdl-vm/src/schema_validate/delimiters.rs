use crate::error::SchemaError;
use crate::schema::{ComplexContent, ElementDecl, GroupDecl, Particle, SchemaDocument, TypeDef};
use alloc::collections::VecDeque;

fn effective_encoding_name(props: &crate::schema::DfdlProps) -> Option<&str> {
    props.encoding.as_deref()
}

fn effective_length_kind(
    props: &crate::schema::DfdlProps,
    inherited: &crate::schema::DfdlProps,
) -> crate::schema::LengthKind {
    props
        .length_kind
        .or(inherited.length_kind)
        .unwrap_or(crate::schema::LengthKind::Delimited)
}

fn ascii_infix_separator_scannable_in_encoding(
    seq_props: &crate::schema::DfdlProps,
    field_enc: &str,
) -> bool {
    let Some(sep) = seq_props.separator.as_deref() else {
        return false;
    };
    if sep.is_empty() || !sep.is_ascii() {
        return false;
    }
    use crate::vm::encoding::normalize_encoding_name;
    let enc = normalize_encoding_name(field_enc);
    enc.is_some_and(|n| {
        n.starts_with("utf-16") || n == "utf-8" || n == "us-ascii" || n == "iso-8859-1"
    })
}

fn encodings_compatible_for_delimiter_scan(a: &str, b: &str) -> bool {
    use crate::vm::encoding::normalize_encoding_name;
    match (normalize_encoding_name(a), normalize_encoding_name(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

pub(crate) fn validate_sequence_separator_encoding(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let Some(ge) = crate::schema::get_global_element(schema, root) else {
        return Ok(());
    };
    let mut queue = VecDeque::new();
    if let Some(td) = schema.resolve_type(&ge.type_name) {
        if let TypeDef::Complex { content, .. } = td {
            let inherited = schema.format_defaults.props.clone();
            enqueue_complex_content(schema, content, &inherited, &mut queue)?;
        }
    }
    while let Some((particles, inherited)) = queue.pop_front() {
        for seq in sequence_groups_in_particles(&particles) {
            validate_one_sequence_separator_encoding(schema, seq.props, seq.particles, &inherited)?;
        }
        for p in &particles {
            enqueue_particle_with_inherited(schema, p, &inherited, &mut queue)?;
        }
    }
    Ok(())
}

struct SeqView<'a> {
    props: &'a crate::schema::DfdlProps,
    particles: &'a [Particle],
}

fn sequence_groups_in_particles(particles: &[Particle]) -> alloc::vec::Vec<SeqView<'_>> {
    let mut out = alloc::vec::Vec::new();
    for p in particles {
        if let Particle::Sequence(seq) = p {
            out.push(SeqView {
                props: &seq.props,
                particles: &seq.particles,
            });
        }
    }
    out
}

fn merge_inherited_group_props(
    inherited: &crate::schema::DfdlProps,
    group: &crate::schema::DfdlProps,
) -> crate::schema::DfdlProps {
    crate::schema::merge_dfdl_props(inherited.clone(), group.clone())
}

fn enqueue_complex_content(
    schema: &SchemaDocument,
    content: &ComplexContent,
    inherited: &crate::schema::DfdlProps,
    queue: &mut VecDeque<(alloc::vec::Vec<Particle>, crate::schema::DfdlProps)>,
) -> Result<(), SchemaError> {
    match content {
        ComplexContent::Empty => Ok(()),
        ComplexContent::Sequence(s) => {
            validate_one_sequence_separator_encoding(schema, &s.props, &s.particles, inherited)?;
            let group_inherited = merge_inherited_group_props(inherited, &s.props);
            queue.push_back((s.particles.clone(), group_inherited));
            Ok(())
        }
        ComplexContent::Choice(c) => {
            queue.push_back((c.branches.clone(), inherited.clone()));
            Ok(())
        }
    }
}

fn enqueue_particle_with_inherited(
    schema: &SchemaDocument,
    p: &Particle,
    inherited: &crate::schema::DfdlProps,
    queue: &mut VecDeque<(alloc::vec::Vec<Particle>, crate::schema::DfdlProps)>,
) -> Result<(), SchemaError> {
    match p {
        Particle::Element(el) => {
            if let Some(td) = schema.resolve_type(&el.type_name) {
                if let TypeDef::Complex { content, props, .. } = td {
                    let next = merge_inherited_group_props(inherited, &el.props);
                    let next = crate::schema::merge_dfdl_props(next, props.clone());
                    enqueue_complex_content(schema, content, &next, queue)?;
                }
            }
        }
        Particle::Sequence(seq) => {
            validate_one_sequence_separator_encoding(
                schema,
                &seq.props,
                &seq.particles,
                inherited,
            )?;
            let next = merge_inherited_group_props(inherited, &seq.props);
            queue.push_back((seq.particles.clone(), next));
        }
        Particle::Choice(ch) => {
            let next = merge_inherited_group_props(inherited, &ch.props);
            queue.push_back((ch.branches.clone(), next));
        }
        Particle::GroupRef(gr) => {
            if let Some(g) = schema
                .groups
                .get(gr.name.rsplit(':').next().unwrap_or(gr.name.as_str()))
            {
                match g {
                    GroupDecl::Sequence(s) => {
                        queue.push_back((s.particles.clone(), inherited.clone()));
                    }
                    GroupDecl::Choice(c) => {
                        queue.push_back((c.branches.clone(), inherited.clone()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn element_effective_props(schema: &SchemaDocument, el: &ElementDecl) -> crate::schema::DfdlProps {
    let mut props = el.props.clone();
    if let Some(ref er) = el.element_ref {
        if let Some(g) = crate::schema::get_global_element(schema, er) {
            props = crate::schema::merge_dfdl_props(g.props.clone(), props);
        }
    }
    props
}

fn has_non_empty_delimiter(props: &crate::schema::DfdlProps, key: &str) -> bool {
    match key {
        "separator" => props.separator.as_deref().is_some_and(|s| !s.is_empty()),
        "terminator" => props.terminator.as_deref().is_some_and(|s| !s.is_empty()),
        "initiator" => props.initiator.as_deref().is_some_and(|s| !s.is_empty()),
        _ => false,
    }
}

fn validate_one_sequence_separator_encoding(
    schema: &SchemaDocument,
    seq_props: &crate::schema::DfdlProps,
    particles: &[Particle],
    inherited: &crate::schema::DfdlProps,
) -> Result<(), SchemaError> {
    if !has_non_empty_delimiter(seq_props, "separator") {
        return Ok(());
    }
    let group_props = merge_inherited_group_props(inherited, seq_props);
    let Some(seq_enc) = effective_encoding_name(&group_props) else {
        return Ok(());
    };
    let elems: alloc::vec::Vec<_> = particles
        .iter()
        .filter_map(|p| {
            if let Particle::Element(el) = p {
                Some(el)
            } else {
                None
            }
        })
        .collect();
    for pair in elems.windows(2) {
        let prev = element_effective_props(schema, pair[0]);
        let next = element_effective_props(schema, pair[1]);
        if effective_length_kind(&prev, &group_props) != crate::schema::LengthKind::Delimited {
            continue;
        }
        if !has_non_empty_delimiter(&prev, "terminator") {
            if let Some(child_enc) = effective_encoding_name(&prev) {
                let allow_ascii_utf16 =
                    ascii_infix_separator_scannable_in_encoding(seq_props, child_enc)
                        && !encodings_compatible_for_delimiter_scan(seq_enc, child_enc);
                if !encodings_compatible_for_delimiter_scan(seq_enc, child_enc)
                    && !allow_ascii_utf16
                {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: The separator of the enclosing group must be in the same encoding as the delimited element that precedes it. encoding separator".to_string(),
                    });
                }
            }
        }
        if effective_length_kind(&next, &group_props) == crate::schema::LengthKind::Delimited
            && effective_length_kind(&prev, &group_props) == crate::schema::LengthKind::Delimited
            && !has_non_empty_delimiter(&prev, "terminator")
        {
            if let Some(next_enc) = effective_encoding_name(&next) {
                if !encodings_compatible_for_delimiter_scan(seq_enc, next_enc) {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: The separator of the enclosing group must be in the same encoding as the delimited element that precedes it. encoding separator".to_string(),
                    });
                }
            }
        }
        if !has_non_empty_delimiter(&prev, "terminator")
            && pair[1].element_ref.is_some()
            && effective_length_kind(&prev, &group_props) == crate::schema::LengthKind::Delimited
        {
            if let (Some(prev_enc), Some(next_enc)) = (
                effective_encoding_name(&prev),
                effective_encoding_name(&next),
            ) {
                if !encodings_compatible_for_delimiter_scan(prev_enc, next_enc) {
                    let seq_encoding_explicit = seq_props.encoding.is_some();
                    if !seq_encoding_explicit
                        || !encodings_compatible_for_delimiter_scan(seq_enc, prev_enc)
                    {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: The separator of the enclosing group must be in the same encoding as the delimited element that precedes it. encoding separator".to_string(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_escape_separator_distinct(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let reachable = super::types::types_reachable_from_root(schema, root);
    for tn in reachable {
        let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&tn) else {
            continue;
        };
        let ComplexContent::Sequence(seq) = content else {
            continue;
        };
        let Some(sep) = seq.props.separator.as_deref() else {
            continue;
        };
        for p in &seq.particles {
            let Particle::Element(el) = p else {
                continue;
            };
            let Some(ref_name) = el.props.escape_scheme_ref.as_deref() else {
                continue;
            };
            let Some(scheme) = schema
                .named_escape_schemes
                .get(ref_name)
                .cloned()
                .or_else(|| {
                    crate::schema::lookup_named_escape_scheme_in_document(schema, ref_name)
                })
            else {
                continue;
            };
            if scheme.escape_kind == crate::schema::EscapeKind::EscapeCharacter {
                let sep_expanded = crate::schema::expand_entities_str(sep);
                let alts = crate::schema::delimiter_alternatives(&sep_expanded);
                if let Some(esc) = scheme.escape_character.as_deref().filter(|s| !s.is_empty()) {
                    let conflicts = alts.iter().any(|alt| alt.starts_with(esc) || alt == esc);
                    if conflicts {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: The escape character cannot be the same as terminating markup for dfdl:separator or dfdl:terminator.".into(),
                        });
                    }
                }
                if let Some(ee) = scheme
                    .escape_escape_character
                    .as_deref()
                    .filter(|s| !s.is_empty())
                {
                    let conflicts = alts.iter().any(|alt| alt.starts_with(ee) || alt == ee);
                    if conflicts {
                        return Err(SchemaError::InvalidProperty {
                            message: "Schema Definition Error: dfdl:terminator and dfdl:separator properties may not begin with the dfdl:escapeEscapeCharacter property value.".into(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}
