use crate::error::SchemaError;
use crate::length_validate::{DaffodilTunables, InvalidRestrictionPolicy};
use crate::schema::{BuiltinType, SchemaDocument, SimpleBase, TypeDef};

pub(crate) fn validate_invalid_restrictions(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if tunables.invalid_restriction_policy != InvalidRestrictionPolicy::Error {
        return Ok(());
    }
    let reachable = super::types::types_reachable_from_root(schema, root);
    for td in schema.types.values() {
        let TypeDef::Simple {
            base: simple_base,
            name,
            ..
        } = td
        else {
            continue;
        };
        if !reachable.contains(name) {
            continue;
        }
        let SimpleBase::Restriction { patterns, .. } = simple_base else {
            continue;
        };
        if patterns.is_empty() {
            continue;
        }
        let is_stringy = schema
            .builtin_for_simple_base(simple_base)
            .is_some_and(|b| b == BuiltinType::String);
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

pub(crate) fn validate_max_hex_binary_length(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    let Some(max) = tunables.max_hex_binary_length_in_bytes else {
        return Ok(());
    };
    let Some(ge) = crate::schema::get_global_element(schema, root) else {
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

pub(crate) fn validate_unique_particle_attribution(
    schema: &SchemaDocument,
) -> Result<(), SchemaError> {
    for particles in super::types::all_choice_particle_lists(schema) {
        validate_particle_list_upa(particles)?;
    }
    Ok(())
}

fn element_upa_fingerprint(el: &crate::schema::ElementDecl) -> alloc::string::String {
    el.type_name.as_str().to_string()
}

fn validate_particle_list_upa(particles: &[crate::schema::Particle]) -> Result<(), SchemaError> {
    use alloc::collections::BTreeMap;
    let mut seen: BTreeMap<&str, alloc::string::String> = BTreeMap::new();
    for p in particles {
        let crate::schema::Particle::Element(el) = p else {
            continue;
        };
        let fp = element_upa_fingerprint(el);
        let el_key = el.element_ref.as_deref().unwrap_or(el.name.as_str());
        if let Some(prev) = seen.get(el_key) {
            if prev != &fp {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: Multiple elements with name '{}', with different types, appear in the model group",
                        el_key
                    ),
                });
            }
        } else {
            seen.insert(el_key, fp);
        }
    }
    Ok(())
}
