use crate::error::SchemaError;
use crate::length_validate::{DaffodilTunables, InvalidRestrictionPolicy};
use crate::schema::{
    BuiltinType, ComplexContent, ElementDecl, GroupDecl, Particle, RestrictionBase, SchemaDocument,
    SimpleBase, TypeDef, TypeName,
};
use alloc::collections::{BTreeSet, VecDeque};

pub fn length_not_defined_message(schema: &SchemaDocument, element_name: Option<&str>) -> String {
    let mut msg = "Schema Definition Error: Property length is not defined".to_string();
    let Some(text) = schema.schema_source_text.as_deref() else {
        return msg;
    };
    let Some(label) = schema.schema_source_label.as_deref() else {
        return msg;
    };
    let needle = element_name
        .map(|n| alloc::format!("name=\"{n}\""))
        .unwrap_or_else(|| "lengthKind=\"explicit\"".to_string());
    for (i, line) in text.lines().enumerate() {
        if line.contains(&needle) {
            msg.push('\n');
            msg.push_str(label);
            msg.push('\n');
            msg.push_str(&alloc::format!("line {}", i + 1));
            if let Some(col) = line.find("<xs:element").map(|c| c + 2) {
                msg.push('\n');
                msg.push_str(&alloc::format!("column {col}"));
            }
            break;
        }
    }
    msg
}

pub fn validate_compiled_schema(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if !schema.schema_diagnostics.is_empty() {
        return Err(SchemaError::InvalidProperty {
            message: schema.schema_diagnostics.join("\n"),
        });
    }
    if !schema.dfdl_annotations_seen {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Non-DFDL Schema file".into(),
        });
    }
    validate_type_references(schema)?;
    validate_name_and_ref(schema)?;
    validate_escape_separator_distinct(schema)?;
    validate_invalid_restrictions(schema, root, tunables)?;
    validate_max_hex_binary_length(schema, root, tunables)?;
    validate_unique_particle_attribution(schema)?;
    let _ = root;
    Ok(())
}

fn validate_unique_particle_attribution(schema: &SchemaDocument) -> Result<(), SchemaError> {
    for particles in all_particle_lists(schema) {
        validate_particle_list_upa(particles)?;
    }
    Ok(())
}

fn element_upa_fingerprint(el: &ElementDecl) -> alloc::string::String {
    alloc::format!(
        "{}|len={:?}|{}",
        el.type_name.as_str(),
        el.props.length_kind,
        el.props.length.unwrap_or(0)
    )
}

fn validate_particle_list_upa(particles: &[Particle]) -> Result<(), SchemaError> {
    use alloc::collections::BTreeMap;
    let mut seen: BTreeMap<&str, alloc::string::String> = BTreeMap::new();
    for p in particles {
        let Particle::Element(el) = p else {
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

fn validate_max_hex_binary_length(
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

fn types_reachable_from_root(schema: &SchemaDocument, root: &str) -> BTreeSet<TypeName> {
    let mut seen = BTreeSet::new();
    let Some(ge) = crate::schema::get_global_element(schema, root) else {
        return seen;
    };
    let mut queue = VecDeque::new();
    queue.push_back(ge.type_name.clone());
    while let Some(tn) = queue.pop_front() {
        if !seen.insert(tn.clone()) {
            continue;
        }
        if BuiltinType::from_xsd(tn.as_str()).is_some() {
            continue;
        }
        let Some(td) = schema.resolve_type(&tn) else {
            continue;
        };
        match td {
            TypeDef::Simple { base, .. } => enqueue_simple_base_types(schema, base, &mut queue),
            TypeDef::Complex { content, .. } => {
                enqueue_particle_types(schema, content, &mut queue);
            }
        }
    }
    seen
}

fn enqueue_simple_base_types(
    schema: &SchemaDocument,
    base: &SimpleBase,
    queue: &mut VecDeque<TypeName>,
) {
    match base {
        SimpleBase::Restriction {
            base: RestrictionBase::Named(parent),
            ..
        } => {
            if schema.resolve_type(parent).is_some() {
                queue.push_back(parent.clone());
            }
        }
        SimpleBase::Restriction {
            base: RestrictionBase::Builtin(_),
            ..
        }
        | SimpleBase::Builtin(_)
        | SimpleBase::Union { .. } => {}
    }
}

fn enqueue_particle_types(
    schema: &SchemaDocument,
    content: &ComplexContent,
    queue: &mut VecDeque<TypeName>,
) {
    let particles = match content {
        ComplexContent::Sequence(s) => &s.particles,
        ComplexContent::Choice(c) => &c.branches,
        ComplexContent::Empty => return,
    };
    for p in particles {
        enqueue_particle(schema, p, queue);
    }
}

fn enqueue_particle(schema: &SchemaDocument, particle: &Particle, queue: &mut VecDeque<TypeName>) {
    match particle {
        Particle::Element(el) => {
            if BuiltinType::from_xsd(el.type_name.as_str()).is_none() {
                queue.push_back(el.type_name.clone());
            }
        }
        Particle::Sequence(s) => {
            for p in &s.particles {
                enqueue_particle(schema, p, queue);
            }
        }
        Particle::Choice(c) => {
            for p in &c.branches {
                enqueue_particle(schema, p, queue);
            }
        }
        Particle::GroupRef(gr) => {
            let local = gr.name.rsplit(':').next().unwrap_or(gr.name.as_str());
            if let Some(group) = schema.groups.get(local) {
                match group {
                    GroupDecl::Sequence(s) => {
                        for p in &s.particles {
                            enqueue_particle(schema, p, queue);
                        }
                    }
                    GroupDecl::Choice(c) => {
                        for p in &c.branches {
                            enqueue_particle(schema, p, queue);
                        }
                    }
                }
            }
        }
    }
}

fn validate_invalid_restrictions(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if tunables.invalid_restriction_policy != InvalidRestrictionPolicy::Error {
        return Ok(());
    }
    let reachable = types_reachable_from_root(schema, root);
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
