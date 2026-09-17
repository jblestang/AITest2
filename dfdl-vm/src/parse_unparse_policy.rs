use crate::error::SchemaError;
use crate::schema::{
    get_global_element, ComplexContent, ElementDecl, GlobalElement, ParseUnparsePolicy, Particle,
    SchemaDocument, TypeDef,
};
use alloc::collections::BTreeMap;
use alloc::format;

fn policy_label(p: ParseUnparsePolicy) -> &'static str {
    match p {
        ParseUnparsePolicy::Both => "both",
        ParseUnparsePolicy::ParseOnly => "parseOnly",
        ParseUnparsePolicy::UnparseOnly => "unparseOnly",
    }
}

fn element_policy(el: &ElementDecl, schema: &SchemaDocument) -> ParseUnparsePolicy {
    el.props
        .parse_unparse_policy
        .or_else(|| {
            get_global_element(
                schema,
                el.element_ref.as_deref().unwrap_or(el.name.as_str()),
            )
            .and_then(|g| g.props.parse_unparse_policy)
        })
        .unwrap_or(ParseUnparsePolicy::Both)
}

fn global_policy(g: &GlobalElement) -> ParseUnparsePolicy {
    g.props
        .parse_unparse_policy
        .unwrap_or(ParseUnparsePolicy::Both)
}

fn check_compatible(
    root_policy: ParseUnparsePolicy,
    child_name: &str,
    child_policy: ParseUnparsePolicy,
) -> Result<(), SchemaError> {
    let compatible =
        root_policy == child_policy || child_policy == ParseUnparsePolicy::Both;
    if compatible {
        return Ok(());
    }
    Err(SchemaError::InvalidProperty {
        message: format!(
            "Schema Definition Error: Child element '{child_name}' with dfdlx:parseUnparsePolicy='{}' is not compatible with root elements dfdlx:parseUnparsePolicy='{}'",
            policy_label(child_policy),
            policy_label(root_policy)
        ),
    })
}

fn walk_particles(
    schema: &SchemaDocument,
    particles: &[Particle],
    root_policy: ParseUnparsePolicy,
) -> Result<(), SchemaError> {
    for p in particles {
        match p {
            Particle::Element(el) => {
                let cp = element_policy(el, schema);
                check_compatible(root_policy, &el.name, cp)?;
                if let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&el.type_name) {
                    walk_complex(schema, content, root_policy)?;
                }
            }
            Particle::Sequence(seq) => walk_particles(schema, &seq.particles, root_policy)?,
            Particle::Choice(ch) => walk_particles(schema, &ch.branches, root_policy)?,
            Particle::GroupRef(gr) => {
                let local = gr.name.rsplit(':').next().unwrap_or(&gr.name);
                if let Some(group) = schema.groups.get(local) {
                    match group {
                        crate::schema::GroupDecl::Sequence(seq) => {
                            walk_particles(schema, &seq.particles, root_policy)?
                        }
                        crate::schema::GroupDecl::Choice(ch) => {
                            walk_particles(schema, &ch.branches, root_policy)?
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn walk_complex(
    schema: &SchemaDocument,
    content: &ComplexContent,
    root_policy: ParseUnparsePolicy,
) -> Result<(), SchemaError> {
    match content {
        ComplexContent::Sequence(seq) => walk_particles(schema, &seq.particles, root_policy),
        ComplexContent::Choice(ch) => walk_particles(schema, &ch.branches, root_policy),
        ComplexContent::Empty => Ok(()),
    }
}

pub fn validate_parse_unparse_policy(schema: &SchemaDocument, root: &str) -> Result<(), SchemaError> {
    let Some(root_el) = get_global_element(schema, root) else {
        return Ok(());
    };
    let root_policy = global_policy(root_el);
    if let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&root_el.type_name) {
        walk_complex(schema, content, root_policy)?;
    }
    Ok(())
}

pub fn parse_support_error() -> SchemaError {
    SchemaError::InvalidProperty {
        message: "Schema Definition Error: This schema was compiled without parse support. Check the parseUnparsePolicy tunable or dfdlx:parseUnparsePolicy property."
            .into(),
    }
}

pub fn unparse_support_error() -> SchemaError {
    SchemaError::InvalidProperty {
        message: "Schema Definition Error: This schema was compiled without unparse support. Check the parseUnparsePolicy tunable or dfdlx:parseUnparsePolicy property."
            .into(),
    }
}

pub fn root_allows_parse(schema: &SchemaDocument, root: &str) -> bool {
    get_global_element(schema, root)
        .map(global_policy)
        .map(|p| matches!(p, ParseUnparsePolicy::Both | ParseUnparsePolicy::ParseOnly))
        .unwrap_or(true)
}

pub fn root_allows_unparse(schema: &SchemaDocument, root: &str) -> bool {
    get_global_element(schema, root)
        .map(global_policy)
        .map(|p| matches!(p, ParseUnparsePolicy::Both | ParseUnparsePolicy::UnparseOnly))
        .unwrap_or(true)
}

pub fn subtree_has_parse_only(schema: &SchemaDocument, root: &str) -> bool {
    fn walk_el(schema: &SchemaDocument, el: &ElementDecl, seen: &mut BTreeMap<String, ()>) -> bool {
        if element_policy(el, schema) == ParseUnparsePolicy::ParseOnly {
            return true;
        }
        if seen.contains_key(&el.name) {
            return false;
        }
        seen.insert(el.name.clone(), ());
        let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&el.type_name) else {
            return false;
        };
        walk_complex_any(schema, content, seen)
    }

    fn walk_complex_any(
        schema: &SchemaDocument,
        content: &ComplexContent,
        seen: &mut BTreeMap<String, ()>,
    ) -> bool {
        let particles = match content {
            ComplexContent::Sequence(s) => &s.particles,
            ComplexContent::Choice(c) => &c.branches,
            ComplexContent::Empty => return false,
        };
        for p in particles {
            if let Particle::Element(el) = p {
                if walk_el(schema, el, seen) {
                    return true;
                }
            }
        }
        false
    }

    let Some(root_el) = get_global_element(schema, root) else {
        return false;
    };
    if global_policy(root_el) == ParseUnparsePolicy::ParseOnly {
        return true;
    }
    let Some(TypeDef::Complex { content, .. }) = schema.resolve_type(&root_el.type_name) else {
        return false;
    };
    let mut seen = BTreeMap::new();
    walk_complex_any(schema, content, &mut seen)
}
