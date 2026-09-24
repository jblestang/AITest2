use crate::error::SchemaError;
use crate::length_validate::DaffodilTunables;
use crate::schema::{
    ComplexContent, ElementDecl, GlobalElement, GroupDecl, Particle, SchemaDocument, TypeDef,
};
fn check_format_props(
    props: &crate::schema::DfdlProps,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if tunables.require_text_bidi_property == Some(true) && props.text_bidi.is_none() {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property textBidi is not defined.".into(),
        });
    }
    if props.text_bidi == Some(true) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property value textBidi='yes' is not supported."
                .into(),
        });
    }
    if tunables.require_floating_property == Some(true) && props.floating.is_none() {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property floating is not defined.".into(),
        });
    }
    if props.floating == Some(true) {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property value floating='yes' is not supported."
                .into(),
        });
    }
    Ok(())
}

fn walk_particles(
    schema: &SchemaDocument,
    particles: &[Particle],
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    for p in particles {
        match p {
            Particle::Element(el) => walk_element(schema, el, tunables)?,
            Particle::Sequence(seq) => walk_particles(schema, &seq.particles, tunables)?,
            Particle::Choice(ch) => walk_particles(schema, &ch.branches, tunables)?,
            Particle::GroupRef(gr) => {
                let local = gr.name.rsplit(':').next().unwrap_or(&gr.name);
                if let Some(group) = schema.groups.get(local) {
                    match group {
                        GroupDecl::Sequence(seq) => {
                            walk_particles(schema, &seq.particles, tunables)?
                        }
                        GroupDecl::Choice(ch) => walk_particles(schema, &ch.branches, tunables)?,
                    }
                }
            }
        }
    }
    Ok(())
}

fn walk_element(
    schema: &SchemaDocument,
    el: &ElementDecl,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    check_format_props(&el.props, tunables)?;
    if let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&el.type_name) {
        walk_complex(schema, content, tunables)?;
    }
    Ok(())
}

fn walk_complex(
    schema: &SchemaDocument,
    content: &ComplexContent,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    match content {
        ComplexContent::Sequence(seq) => {
            check_format_props(&seq.props, tunables)?;
            walk_particles(schema, &seq.particles, tunables)
        }
        ComplexContent::Choice(ch) => {
            check_format_props(&ch.props, tunables)?;
            walk_particles(schema, &ch.branches, tunables)
        }
        ComplexContent::Empty => Ok(()),
    }
}

fn walk_global(
    schema: &SchemaDocument,
    g: &GlobalElement,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    check_format_props(&schema.format_defaults.props, tunables)?;
    check_format_props(&g.props, tunables)?;
    if let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&g.type_name) {
        walk_complex(schema, content, tunables)?;
    }
    Ok(())
}

pub fn validate_tunable_schema_requirements(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if tunables.require_encoding_error_policy == Some(true)
        && !schema.explicit_encoding_error_policy_on_format
    {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Property encodingErrorPolicy is not defined.".into(),
        });
    }
    check_format_props(&schema.format_defaults.props, tunables)?;
    let Some(g) = crate::schema::get_global_element(schema, root) else {
        return Ok(());
    };
    walk_global(schema, g, tunables)
}
