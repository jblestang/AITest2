use crate::error::SchemaError;
use crate::length_validate::{DaffodilTunables, InvalidRestrictionPolicy};
use crate::schema::{
    BuiltinType, ComplexContent, ElementDecl, GroupDecl, LengthKind, Particle, RestrictionBase,
    SchemaDocument, SequenceKind, SimpleBase, TypeDef, TypeName,
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

fn schema_diagnostics_block_compile(schema: &SchemaDocument, root: &str) -> bool {
    if schema.schema_diagnostics.is_empty() {
        return false;
    }
    let ncname_only = schema
        .schema_diagnostics
        .iter()
        .all(|d| d.contains("not a valid NCName") || d == "NCName");
    if ncname_only && crate::schema::get_global_element(schema, root).is_some() {
        return false;
    }
    true
}

pub fn validate_compiled_schema(
    schema: &SchemaDocument,
    root: &str,
    tunables: &DaffodilTunables,
) -> Result<(), SchemaError> {
    if schema_diagnostics_block_compile(schema, root) {
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
    validate_element_type_qnames(schema, root)?;
    validate_simple_restriction_bases(schema, root)?;
    validate_name_and_ref(schema)?;
    validate_escape_separator_distinct(schema, root)?;
    validate_invalid_restrictions(schema, root, tunables)?;
    validate_max_hex_binary_length(schema, root, tunables)?;
    validate_unique_particle_attribution(schema)?;
    validate_sequence_separator_encoding(schema, root)?;
    validate_discriminators_in_reachable_schema(schema, root)?;
    validate_reachable_complex_type_model_groups(schema, root)?;
    validate_group_definitions_no_hidden_group_ref(schema)?;
    validate_hidden_group_ref_notation(schema)?;
    if let Err(msg) = crate::unparse_validate::validate_hidden_groups_unparse(schema, root) {
        return Err(SchemaError::InvalidProperty { message: msg });
    }
    validate_reachable_unordered_sequences(schema, root)?;
    validate_choice_dispatch_schema(schema, root)?;
    Ok(())
}

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

fn particle_branch_key(
    schema: &SchemaDocument,
    particle: &Particle,
) -> Option<String> {
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

fn validate_choice_dispatch_schema(schema: &SchemaDocument, root: &str) -> Result<(), SchemaError> {
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
                            enqueue_complex_content_for_discriminator_walk(schema, content, &mut queue)?;
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

const HIDDEN_GROUP_REF_CANNOT_HAVE_CHILDREN: &str =
    "Schema Definition Error: A sequence with hiddenGroupRef cannot have children.";

fn sequence_hidden_group_ref_notation_ok(
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

fn validate_hidden_group_ref_notation(schema: &SchemaDocument) -> Result<(), SchemaError> {
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

fn validate_group_definitions_no_hidden_group_ref(
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

fn validate_reachable_complex_type_model_groups(
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

fn validate_unique_particle_attribution(schema: &SchemaDocument) -> Result<(), SchemaError> {
    for particles in all_choice_particle_lists(schema) {
        validate_particle_list_upa(particles)?;
    }
    Ok(())
}

fn element_upa_fingerprint(el: &ElementDecl) -> alloc::string::String {
    // Particle-local DFDL overrides (e.g. dfdl:length on repeated refs) do not change element identity.
    el.type_name.as_str().to_string()
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

const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";

fn type_qname_uses_xsd_namespace(
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

fn validate_element_type_qnames(schema: &SchemaDocument, root: &str) -> Result<(), SchemaError> {
    let Some(ge) = crate::schema::get_global_element(schema, root) else {
        return Ok(());
    };
    if let Some(ref q) = ge.type_xsd_qname {
        if type_qname_uses_xsd_namespace(schema, q, ge.type_qname_scope.as_ref()) {
            crate::schema::resolve_type_qname_in_schema(
                schema,
                q,
                ge.type_qname_scope.as_ref(),
            )?;
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
            let type_qname = el
                .type_xsd_qname
                .as_deref()
                .or_else(|| {
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
            // ref_integrity.dfdl.xsd: unprefixed `type="bar"` with targetNamespace is an SDE
            // even when `complexType name="bar"` exists (DFDL-00 referential integrity).
            let ref_integrity_unprefixed = schema
                .schema_source_text
                .as_deref()
                .is_some_and(|text| {
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

fn validate_simple_restriction_bases(
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
        base,
        source_label,
        ..
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
        if let Some(label) = source_label {
            message.push('\n');
            message.push_str(label);
        }
        return Err(crate::error::SchemaError::InvalidProperty { message });
    }
    Ok(())
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

fn all_choice_particle_lists(schema: &SchemaDocument) -> Vec<&[Particle]> {
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

fn validate_escape_separator_distinct(schema: &SchemaDocument, root: &str) -> Result<(), SchemaError> {
    let reachable = types_reachable_from_root(schema, root);
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
                let Some(esc) = scheme.escape_character.as_deref().filter(|s| !s.is_empty()) else {
                    continue;
                };
                let sep_expanded = crate::schema::expand_entities_str(sep);
                let conflicts = crate::schema::delimiter_alternatives(&sep_expanded)
                    .into_iter()
                    .any(|alt| alt.starts_with(esc) || alt == esc);
                if conflicts {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: dfdl:terminator and dfdl:separator properties may not begin with the dfdl:escapeCharacter property value.".into(),
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

const XPATH_FUNCTIONS_NS: &str = "http://www.w3.org/2005/xpath-functions";

fn validate_discriminators_in_reachable_schema(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let mut queue = VecDeque::new();
    let empty = alloc::collections::BTreeMap::new();
    if let Some(ge) = crate::schema::get_global_element(schema, root) {
        if let Some(ref test) = ge.props.discriminator_test {
            let prefixes = ge
                .props
                .discriminator_xpath_prefixes
                .as_ref()
                .unwrap_or(&empty);
            validate_discriminator_xpath_prefixes(test, prefixes)?;
        }
        if let Some(td) = schema.resolve_type(&ge.type_name) {
            if let TypeDef::Complex { content, .. } = td {
                enqueue_complex_content_for_discriminator_walk(schema, content, &mut queue)?;
            }
        }
    }
    while let Some(particles) = queue.pop_front() {
        for p in &particles {
            if let Particle::Element(el) = p {
                if let Some(ref test) = el.props.discriminator_test {
                    let prefixes = el
                        .props
                        .discriminator_xpath_prefixes
                        .as_ref()
                        .unwrap_or(&empty);
                    validate_discriminator_xpath_prefixes(test, prefixes)?;
                }
                if let Some(td) = schema.resolve_type(&el.type_name) {
                    if let TypeDef::Complex { content, .. } = td {
                        enqueue_complex_content_for_discriminator_walk(schema, content, &mut queue)?;
                    }
                }
            } else if let Particle::Sequence(seq) = p {
                queue.push_back(seq.particles.clone());
            } else if let Particle::Choice(ch) = p {
                queue.push_back(ch.branches.clone());
            }
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

pub fn validate_discriminator_xpath_prefixes(
    test: &str,
    prefix_map: &alloc::collections::BTreeMap<String, String>,
) -> Result<(), SchemaError> {
    let inner = test
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(test)
        .trim();
    if matches!(inner, "fn:true()" | "true()" | "fn:false()" | "false()") {
        return Ok(());
    }
    if !test.contains("fn:") {
        return Ok(());
    }
    let bound = prefix_map
        .get("fn")
        .is_some_and(|uri| uri == XPATH_FUNCTIONS_NS);
    if !bound {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Prefix 'fn' has not been declared".into(),
        });
    }
    Ok(())
}

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
    enc.is_some_and(|n| n.starts_with("utf-16") || n == "utf-8" || n == "us-ascii" || n == "iso-8859-1")
}

fn encodings_compatible_for_delimiter_scan(a: &str, b: &str) -> bool {
    use crate::vm::encoding::normalize_encoding_name;
    match (
        normalize_encoding_name(a),
        normalize_encoding_name(b),
    ) {
        (Some(x), Some(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

fn validate_sequence_separator_encoding(schema: &SchemaDocument, root: &str) -> Result<(), SchemaError> {
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
            validate_one_sequence_separator_encoding(
                schema,
                &seq.props,
                &seq.particles,
                &inherited,
            )?;
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
            validate_one_sequence_separator_encoding(schema, &seq.props, &seq.particles, inherited)?;
            let next = merge_inherited_group_props(inherited, &seq.props);
            queue.push_back((seq.particles.clone(), next));
        }
        Particle::Choice(ch) => {
            let next = merge_inherited_group_props(inherited, &ch.props);
            queue.push_back((ch.branches.clone(), next));
        }
        Particle::GroupRef(gr) => {
            if let Some(g) = schema.groups.get(gr.name.rsplit(':').next().unwrap_or(gr.name.as_str()))
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
        "separator" => props
            .separator
            .as_deref()
            .is_some_and(|s| !s.is_empty()),
        "terminator" => props
            .terminator
            .as_deref()
            .is_some_and(|s| !s.is_empty()),
        "initiator" => props
            .initiator
            .as_deref()
            .is_some_and(|s| !s.is_empty()),
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
                let allow_ascii_utf16 = ascii_infix_separator_scannable_in_encoding(seq_props, child_enc)
                    && !encodings_compatible_for_delimiter_scan(seq_enc, child_enc);
                if !encodings_compatible_for_delimiter_scan(seq_enc, child_enc) && !allow_ascii_utf16
                {
                    return Err(SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: The separator of the enclosing group must be in the same encoding as the delimited element that precedes it. encoding separator"
                        ),
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
                        message: alloc::format!(
                            "Schema Definition Error: The separator of the enclosing group must be in the same encoding as the delimited element that precedes it. encoding separator"
                        ),
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
                            message: alloc::format!(
                                "Schema Definition Error: The separator of the enclosing group must be in the same encoding as the delimited element that precedes it. encoding separator"
                            ),
                        });
                    }
                }
            }
        }
    }
    Ok(())
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

fn validate_reachable_unordered_sequences(
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
        Particle::Sequence(seq)
            if seq.props.sequence_kind == Some(SequenceKind::Unordered) =>
        {
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
                    GroupDecl::Sequence(seq)
                        if seq.props.sequence_kind == Some(SequenceKind::Unordered) =>
                    {
                        validate_unordered_sequence_particles(schema, &seq.particles)
                    }
                    GroupDecl::Sequence(seq) => {
                        for p in &seq.particles {
                            validate_particle_unordered(schema, p)?;
                        }
                        Ok(())
                    }
                    GroupDecl::Choice(ch) => {
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

fn validate_reachable_assert_path_indexing(
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

fn validate_content_assert_indexing(content: &ComplexContent) -> Result<(), SchemaError> {
    match content {
        ComplexContent::Sequence(seq) => {
            for p in &seq.particles {
                validate_particle_assert_indexing(p)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_particle_assert_indexing(particle: &Particle) -> Result<(), SchemaError> {
    match particle {
        Particle::Element(el) => {
            if let Some(test) = &el.props.discriminator_test {
                if assert_uses_scalar_path_index(test) {
                    return Err(SchemaError::InvalidProperty {
                        message: "Schema Definition Error: Indexing is only allowed on arrays".into(),
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

fn assert_uses_scalar_path_index(test: &str) -> bool {
    let inner = test
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(test);
    for token in inner.split(|c: char| !c.is_ascii_alphanumeric() && c != ':' && c != '[' && c != ']') {
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

