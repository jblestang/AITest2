use crate::error::SchemaError;
use crate::schema::{ComplexContent, Particle, SchemaDocument, TypeDef};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;

const XPATH_FUNCTIONS_NS: &str = "http://www.w3.org/2005/xpath-functions";

pub fn validate_discriminator_xpath_prefixes(
    test: &str,
    prefix_map: &BTreeMap<String, String>,
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
        .is_none_or(|uri| uri == XPATH_FUNCTIONS_NS);
    if !bound {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Prefix 'fn' has not been declared".into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_discriminators_in_reachable_schema(
    schema: &SchemaDocument,
    root: &str,
) -> Result<(), SchemaError> {
    let mut queue = VecDeque::new();
    let empty = BTreeMap::new();
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
                        enqueue_complex_content_for_discriminator_walk(
                            schema, content, &mut queue,
                        )?;
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
