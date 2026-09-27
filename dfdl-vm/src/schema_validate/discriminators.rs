use crate::error::SchemaError;
use crate::schema::{ComplexContent, Particle, SchemaDocument, TypeDef};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;

const XPATH_FUNCTIONS_NS: &str = "http://www.w3.org/2005/xpath-functions";

pub fn validate_discriminator_xpath_prefixes(
    test: &str,
    prefix_map: &BTreeMap<String, String>,
    schema_prefixes: &BTreeMap<String, String>,
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
    if !prefix_map.contains_key("fn") && !schema_prefixes.contains_key("fn") {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: Prefix 'fn' has no corresponding namespace declaration in the schema.".into(),
        });
    }
    Ok(())
}

fn validate_test_pattern_syntax(pat: &str) -> Result<(), SchemaError> {
    use regex_automata::meta::Regex;
    let pat = pat.trim();
    if pat.is_empty() {
        return Ok(());
    }
    let anchored = alloc::format!(r"\A(?:{pat})");
    if Regex::new(&anchored).is_err() {
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: The pattern contained invalid syntax `{pat}`"
            ),
        });
    }
    Ok(())
}

fn validate_props_discriminator_flags(props: &crate::schema::DfdlProps) -> Result<(), SchemaError> {
    if let Some(ref pat) = props.test_pattern {
        validate_test_pattern_syntax(pat)?;
    }
    if props.has_test_attr_and_body {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: You may not specify both test attribute and a body expression".into(),
        });
    }
    if props.has_test_pat_attr_and_body {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: You may not specify both testPattern attribute and a body expression".into(),
        });
    }
    if props.has_test_and_test_pattern {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: It is a schema definition error if both a test expression and a test pattern are specified".into(),
        });
    }
    if props.has_multiple_discriminators {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: A component cannot have more than one discriminator statement.".into(),
        });
    }
    if props.has_discriminator_and_assert {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: A component cannot have both a discriminator statement and an assert statement.".into(),
        });
    }
    if props.has_empty_test_pattern {
        return Err(SchemaError::InvalidProperty {
            message: "Schema Definition Error: The attribute testPattern must not be empty for testKind='pattern'".into(),
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
        validate_props_discriminator_flags(&ge.props)?;
        if let Some(ref test) = ge.props.discriminator_test {
            let prefixes = ge
                .props
                .discriminator_xpath_prefixes
                .as_ref()
                .unwrap_or(&empty);
            validate_discriminator_xpath_prefixes(test, prefixes, &schema.namespace_prefixes)?;
        }
        let td_opt = schema.resolve_type(&ge.type_name);
        if let Some(td) = td_opt {
            validate_props_discriminator_flags(td.props())?;
            if let TypeDef::Complex { content, .. } = td {
                enqueue_complex_content_for_discriminator_walk(schema, content, &mut queue)?;
            }
        }
    }
    while let Some(particles) = queue.pop_front() {
        for p in &particles {
            if let Particle::Element(el) = p {
                validate_props_discriminator_flags(&el.props)?;
                if let Some(ref test) = el.props.discriminator_test {
                    let prefixes = el
                        .props
                        .discriminator_xpath_prefixes
                        .as_ref()
                        .unwrap_or(&empty);
                    validate_discriminator_xpath_prefixes(test, prefixes, &schema.namespace_prefixes)?;
                }
                if let Some(ref child_p) = el.particle {
                    queue.push_back(alloc::vec![(**child_p).clone()]);
                }
                if let Some(td) = schema.resolve_type(&el.type_name) {
                    validate_props_discriminator_flags(td.props())?;
                    if let TypeDef::Complex { content, .. } = td {
                        enqueue_complex_content_for_discriminator_walk(
                            schema, content, &mut queue,
                        )?;
                    }
                }
            } else if let Particle::Sequence(seq) = p {
                validate_props_discriminator_flags(&seq.props)?;
                queue.push_back(seq.particles.clone());
            } else if let Particle::Choice(ch) = p {
                validate_props_discriminator_flags(&ch.props)?;
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
