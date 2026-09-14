use crate::error::SchemaError;
use crate::length_validate::{DaffodilTunables, InvalidRestrictionPolicy};
use crate::schema::{
    ComplexContent, ElementDecl, GroupDecl, Particle, SchemaDocument, SimpleBase, TypeDef, TypeName,
};

pub fn validate_compiled_schema(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if !schema.dfdl_annotations_seen {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Non-DFDL Schema file".into(),
        });
    }
    validate_type_references(schema)?;
    validate_name_and_ref(schema)?;
    validate_escape_separator_distinct(schema)?;
    validate_invalid_restrictions(schema, tunables)?;
    validate_max_hex_binary_length(schema, root, tunables)?;
    let _ = root;
    Ok(())
}

fn validate_max_hex_binary_length(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    let Some(max) = tunables.max_hex_binary_length_in_bytes else {
        return Ok(());
    };
    let Some(ge) = schema.global_elements.get(root) else {
        return Ok(());
    };
    let t = ge.type_name.as_str();
    if !matches!(t, "xs:hexBinary" | "hexBinary") {
        return Ok(());
    }
    if ge.props.length_kind != Some(crate::schema::LengthKind::Explicit) {
        return Ok(());
    }
    let Some(len) = ge.props.length else {
        return Ok(());
    };
    if len > u64::from(max) {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Parse Error: xs:hexBinary maximum {max} {len}"),
        });
    }
    Ok(())
}

fn validate_type_references(schema: &SchemaDocument) -> Result<(), SchemaError> {
    if schema.target_namespace.is_none() {
        return Ok(());
    }
    for ge in schema.global_elements.values() {
        let t = ge.type_name.as_str();
        if ge.type_qname_prefixed
            || t.contains(':')
            || builtin_type_name(t)
            || t.starts_with("__inline_")
        {
            continue;
        }
        if schema.types.contains_key(&TypeName::new(t)) {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!("Schema Definition Error: Error resolving component '{t}'"),
            });
        }
    }
    Ok(())
}

fn builtin_type_name(t: &str) -> bool {
    t.starts_with("xs:")
}

fn validate_name_and_ref(schema: &SchemaDocument) -> Result<(), SchemaError> {
    for particles in all_particle_lists(schema) {
        for p in particles {
            if let Particle::Element(el) = p {
                validate_element_name_ref(el)?;
            }
        }
    }
    Ok(())
}

fn validate_element_name_ref(el: &ElementDecl) -> Result<(), SchemaError> {
    if el.element_ref.is_some() && (el.has_element_name_attr || el.type_name.as_str() != "xs:string") {
        // Ref-only particles inherit type from global; `type="xs:string"` on ref+type is caught at parse.
        if el.has_element_name_attr {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: name and type attributes cannot appear together with ref attribute".into(),
            });
        }
    }
    Ok(())
}

fn all_particle_lists(schema: &SchemaDocument) -> Vec<&[Particle]> {
    let mut out = Vec::new();
    for td in schema.types.values() {
        if let TypeDef::Complex { content, .. } = td {
            match content {
                ComplexContent::Sequence(s) => out.push(s.particles.as_slice()),
                ComplexContent::Choice(c) => out.push(c.branches.as_slice()),
                ComplexContent::Empty => {}
            }
        }
    }
    for g in schema.groups.values() {
        match g {
            GroupDecl::Sequence(s) => out.push(s.particles.as_slice()),
            GroupDecl::Choice(c) => out.push(c.branches.as_slice()),
        }
    }
    out
}

fn validate_escape_separator_distinct(schema: &SchemaDocument) -> Result<(), SchemaError> {
    for td in schema.types.values() {
        let TypeDef::Complex { content, .. } = td else {
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
            let key = ref_name.rsplit(':').next().unwrap_or(ref_name);
            let Some(scheme) = schema.named_escape_schemes.get(key) else {
                continue;
            };
            if scheme.escape_kind == crate::schema::EscapeKind::EscapeCharacter {
                if scheme
                    .escape_character
                    .as_deref()
                    .is_some_and(|c| c == sep)
                {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: escape character `{sep}` cannot be the same as terminating markup `{sep}`"
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_invalid_restrictions(
    schema: &SchemaDocument,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if tunables.invalid_restriction_policy != InvalidRestrictionPolicy::Error {
        return Ok(());
    }
    for td in schema.types.values() {
        let TypeDef::Simple {
            base: SimpleBase::Restriction { patterns, base, .. },
            name,
            ..
        } = td
        else {
            continue;
        };
        if patterns.is_empty() {
            continue;
        }
        let is_stringy = matches!(
            base,
            crate::schema::RestrictionBase::Builtin(crate::schema::BuiltinType::String)
        );
        if !is_stringy {
            return Err(SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: Pattern restriction on type `{}` must be on a string",
                    name.as_str()
                ),
            });
        }
    }
    Ok(())
}
