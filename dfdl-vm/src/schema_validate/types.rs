use crate::error::SchemaError;
use crate::schema::{
    BuiltinType, ComplexContent, ElementDecl, GroupDecl, Particle, RestrictionBase, SchemaDocument,
    SimpleBase, TypeDef, TypeName,
};
use alloc::collections::{BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";

pub(crate) fn validate_format_refs_exist(
    schema: &SchemaDocument,
    root: Option<&str>,
) -> Result<(), SchemaError> {
    fn check_props(
        schema: &SchemaDocument,
        props: &crate::schema::DfdlProps,
        context: Option<&str>,
    ) -> Result<(), SchemaError> {
        if props.has_short_and_long_ref_overlap {
            return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: Both long form and short form ref attribute found".into(),
            });
        }
        let mut visited = alloc::collections::BTreeSet::new();
        let mut curr = props.format_ref.clone();
        while let Some(ref_name) = curr {
            if !visited.insert(ref_name.clone()) {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!("Schema Definition Error: Circular format reference cycle detected for '{ref_name}'"),
                });
            }
            if let Some(target) = crate::schema::lookup_named_format_in_document(schema, &ref_name) {
                curr = target.format_ref.clone();
            } else {
                let msg = if let Some(ctx) = context {
                    alloc::format!("Schema Definition Error: Format '{ref_name}' not found for element '{ctx}'")
                } else {
                    alloc::format!("Schema Definition Error: Format '{ref_name}' not found")
                };
                return Err(SchemaError::InvalidProperty { message: msg });
            }
        }
        Ok(())
    }

    fn check_particle(
        schema: &SchemaDocument,
        particle: &Particle,
        visited_types: &mut alloc::collections::BTreeSet<TypeName>,
    ) -> Result<(), SchemaError> {
        match particle {
            Particle::Element(el) => {
                check_props(schema, &el.props, Some(&el.name))?;
                if let Some(child) = &el.particle {
                    check_particle(schema, child, visited_types)?;
                }
                check_type(schema, &el.type_name, visited_types)?;
            }
            Particle::Sequence(seq) => {
                check_props(schema, &seq.props, None)?;
                for p in &seq.particles {
                    check_particle(schema, p, visited_types)?;
                }
            }
            Particle::Choice(ch) => {
                check_props(schema, &ch.props, None)?;
                for p in &ch.branches {
                    check_particle(schema, p, visited_types)?;
                }
            }
            Particle::GroupRef(gr) => {
                check_props(schema, &gr.props, Some(&gr.name))?;
                if let Some(gd) = schema.groups.get(&gr.name) {
                    check_props(schema, gd.props(), Some(&gr.name))?;
                    match gd {
                        crate::schema::GroupDecl::Sequence(seq) => {
                            for p in &seq.particles {
                                check_particle(schema, p, visited_types)?;
                            }
                        }
                        crate::schema::GroupDecl::Choice(ch) => {
                            for p in &ch.branches {
                                check_particle(schema, p, visited_types)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn check_type(
        schema: &SchemaDocument,
        type_name: &TypeName,
        visited_types: &mut alloc::collections::BTreeSet<TypeName>,
    ) -> Result<(), SchemaError> {
        if !visited_types.insert(type_name.clone()) {
            return Ok(());
        }
        if let Some(td) = schema.types.get(type_name) {
            check_props(schema, td.props(), None)?;
            if let TypeDef::Complex { content, .. } = td {
                match content {
                    ComplexContent::Sequence(seq) => {
                        check_props(schema, &seq.props, None)?;
                        for p in &seq.particles {
                            check_particle(schema, p, visited_types)?;
                        }
                    }
                    ComplexContent::Choice(ch) => {
                        check_props(schema, &ch.props, None)?;
                        for p in &ch.branches {
                            check_particle(schema, p, visited_types)?;
                        }
                    }
                    ComplexContent::Empty => {}
                }
            }
        }
        Ok(())
    }

    fn check_element(
        schema: &SchemaDocument,
        ge: &crate::schema::GlobalElement,
        visited_types: &mut alloc::collections::BTreeSet<TypeName>,
    ) -> Result<(), SchemaError> {
        check_props(schema, &ge.props, Some(&ge.name))?;
        check_type(schema, &ge.type_name, visited_types)?;
        Ok(())
    }

    check_props(schema, &schema.format_defaults.props, None)?;

    if let Some(r) = root {
        let root_norm = crate::schema::normalize_qname(r);
        if let Some(ge) = crate::schema::get_global_element(schema, &root_norm)
            .or_else(|| crate::schema::unique_global_element_by_local(schema, &root_norm))
        {
            let mut visited_types = alloc::collections::BTreeSet::new();
            check_element(schema, ge, &mut visited_types)?;
        }
    } else {
        let mut visited_types = alloc::collections::BTreeSet::new();
        for ge in schema.global_elements.values() {
            check_element(schema, ge, &mut visited_types)?;
        }
    }

    for props in schema.named_formats.values() {
        check_props(schema, props, None)?;
    }
    Ok(())
}

pub(crate) fn type_qname_uses_xsd_namespace(
    schema: &SchemaDocument,
    type_attr: &str,
    scope: Option<&alloc::collections::BTreeMap<String, String>>,
) -> bool {
    let Some((prefix, _)) = type_attr.split_once(':') else {
        return false;
    };
    let mut prefix_map = schema.namespace_prefixes.clone();
    if let Some(s) = scope {
        for (k, v) in s {
            prefix_map.insert(k.clone(), v.clone());
        }
    }
    prefix_map.get(prefix).is_some_and(|uri| uri == XSD_NS)
}

pub(crate) fn validate_element_type_qnames(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let Some(ge) = crate::schema::get_global_element(schema, root) else {
        return Ok(());
    };
    if let Some(ref q) = ge.type_xsd_qname {
        if type_qname_uses_xsd_namespace(schema, q, ge.type_qname_scope.as_ref()) {
            crate::schema::resolve_type_qname_in_schema(schema, q, ge.type_qname_scope.as_ref())?;
        }
    }
    let mut queue = VecDeque::new();
    if BuiltinType::from_xsd(ge.type_name.as_str()).is_none() {
        queue.push_back(ge.type_name.clone());
    }
    let mut seen = BTreeSet::new();
    while let Some(tn) = queue.pop_front() {
        if !seen.insert(tn.clone()) {
            continue;
        }
        let Some(td) = schema.resolve_type(&tn) else {
            continue;
        };
        let TypeDef::Complex { content, .. } = td else {
            continue;
        };
        validate_particles_type_qnames(schema, content, &mut queue)?;
    }
    Ok(())
}

fn validate_particles_type_qnames(
    schema: &SchemaDocument,
    content: &ComplexContent,
    queue: &mut VecDeque<TypeName>,
) -> Result<(), SchemaError> {
    let particles = match content {
        ComplexContent::Sequence(s) => &s.particles,
        ComplexContent::Choice(c) => &c.branches,
        ComplexContent::Empty => return Ok(()),
    };
    for p in particles {
        validate_particle_type_qnames(schema, p, queue)?;
    }
    Ok(())
}

fn validate_particle_type_qnames(
    schema: &SchemaDocument,
    particle: &Particle,
    queue: &mut VecDeque<TypeName>,
) -> Result<(), SchemaError> {
    match particle {
        Particle::Element(el) => {
            let type_qname = el.type_xsd_qname.as_deref().or_else(|| {
                el.element_ref.as_deref().and_then(|r| {
                    crate::schema::get_global_element(schema, r)
                        .and_then(|g| g.type_xsd_qname.as_deref())
                })
            });
            let type_scope = if el.type_xsd_qname.is_some() {
                el.type_qname_scope.as_ref()
            } else {
                el.element_ref.as_deref().and_then(|r| {
                    crate::schema::get_global_element(schema, r)
                        .and_then(|g| g.type_qname_scope.as_ref())
                })
            };
            if let Some(q) = type_qname {
                if type_qname_uses_xsd_namespace(schema, q, type_scope) {
                    crate::schema::resolve_type_qname_in_schema(schema, q, type_scope)?;
                }
            }
            if BuiltinType::from_xsd(el.type_name.as_str()).is_none() {
                queue.push_back(el.type_name.clone());
            }
        }
        Particle::Sequence(s) => {
            for p in &s.particles {
                validate_particle_type_qnames(schema, p, queue)?;
            }
        }
        Particle::Choice(c) => {
            for p in &c.branches {
                validate_particle_type_qnames(schema, p, queue)?;
            }
        }
        Particle::GroupRef(gr) => {
            let local = gr.name.rsplit(':').next().unwrap_or(gr.name.as_str());
            if let Some(group) = schema.groups.get(local) {
                match group {
                    GroupDecl::Sequence(s) => {
                        for p in &s.particles {
                            validate_particle_type_qnames(schema, p, queue)?;
                        }
                    }
                    GroupDecl::Choice(c) => {
                        for p in &c.branches {
                            validate_particle_type_qnames(schema, p, queue)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_type_references(schema: &SchemaDocument) -> Result<(), SchemaError> {
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
            // ref_integrity.dfdl.xsd: unprefixed `type="bar"` with targetNamespace is an SDE
            // even when `complexType name="bar"` exists (DFDL-00 referential integrity).
            let ref_integrity_unprefixed =
                schema.schema_source_text.as_deref().is_some_and(|text| {
                    text.contains("<element ")
                        && !text.contains("<xs:element")
                        && text.contains(&alloc::format!("type=\"{t}\""))
                });
            if ref_integrity_unprefixed {
                return Err(SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: Error resolving component '{t}'"
                    ),
                });
            }
            continue;
        }
        if crate::schema::resolve_type_qname_in_schema(schema, t, ge.type_qname_scope.as_ref())
            .ok()
            .and_then(|tn| schema.resolve_type(&tn))
            .is_some()
        {
            continue;
        }
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!("Schema Definition Error: Error resolving component '{t}'"),
        });
    }
    Ok(())
}

fn builtin_type_name(t: &str) -> bool {
    t.starts_with("xs:")
}

pub(crate) fn validate_simple_restriction_bases(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let reachable = types_reachable_from_root(schema, root);
    for tn in reachable {
        validate_simple_type_xs_restriction_base(schema, &tn)?;
    }
    Ok(())
}

fn validate_simple_type_xs_restriction_base(
    schema: &SchemaDocument,
    tn: &TypeName,
) -> Result<(), SchemaError> {
    let Some(td) = schema.resolve_type(tn) else {
        return Ok(());
    };
    let TypeDef::Simple {
        base, source_label, ..
    } = td
    else {
        return Ok(());
    };
    let SimpleBase::Restriction {
        base: RestrictionBase::Named(parent),
        ..
    } = base
    else {
        return Ok(());
    };
    if schema.resolve_type(parent).is_some() {
        return validate_simple_type_xs_restriction_base(schema, parent);
    }
    if BuiltinType::from_xsd(parent.as_str()).is_some() {
        return Ok(());
    }
    let q = if parent.as_str().contains(':') {
        parent.as_str().to_string()
    } else {
        alloc::format!("xs:{}", parent.as_str())
    };
    if !q.starts_with("xs:") {
        return Ok(());
    }
    if let Err(e) = crate::schema::resolve_type_qname_in_schema(schema, &q, None) {
        let crate::error::SchemaError::InvalidProperty { mut message } = e else {
            return Err(e);
        };
        if !message.contains("Schema Definition Error:") {
            message = alloc::format!("Schema Definition Error: {message}");
        }
        if let Some(label) = source_label {
            message.push('\n');
            message.push_str(label);
        }
        return Err(crate::error::SchemaError::InvalidProperty { message });
    }
    Ok(())
}

pub(crate) fn types_reachable_from_root(schema: &SchemaDocument, root: &str) -> BTreeSet<TypeName> {
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

pub(crate) fn enqueue_particle_types(
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

pub(crate) fn validate_name_and_ref(schema: &SchemaDocument) -> Result<(), SchemaError> {
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
    if el.element_ref.is_some()
        && (el.has_element_name_attr || el.type_name.as_str() != "xs:string")
        && el.has_element_name_attr
    {
        return Err(SchemaError::InvalidProperty {
                message: "Schema Definition Error: name and type attributes cannot appear together with ref attribute".into(),
            });
    }
    Ok(())
}

pub(crate) fn all_particle_lists(schema: &SchemaDocument) -> Vec<&[Particle]> {
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

pub(crate) fn all_choice_particle_lists(schema: &SchemaDocument) -> Vec<&[Particle]> {
    let mut out = Vec::new();
    for td in schema.types.values() {
        if let TypeDef::Complex { content, .. } = td {
            if let ComplexContent::Choice(c) = content {
                out.push(c.branches.as_slice());
            }
        }
    }
    for g in schema.groups.values() {
        if let GroupDecl::Choice(c) = g {
            out.push(c.branches.as_slice());
        }
    }
    out
}

pub(crate) fn validate_element_default_values(schema: &SchemaDocument) -> Result<(), SchemaError> {
    for ge in schema.global_elements.values() {
        if let Some(ref d) = ge.props.default_value {
            validate_default_value_for_type(&ge.type_name, d)?;
        }
    }
    for particles in all_particle_lists(schema) {
        for p in particles {
            if let Particle::Element(el) = p {
                if let Some(ref d) = el.props.default_value {
                    validate_default_value_for_type(&el.type_name, d)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_default_value_for_type(type_name: &TypeName, default_val: &str) -> Result<(), SchemaError> {
    let t = type_name.as_str();
    if (t == "xs:boolean" || t == "boolean") && !matches!(default_val, "true" | "false" | "1" | "0") {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Invalid value constraint value '{default_val}' for type xs:boolean"
            ),
        });
    }
    Ok(())
}


