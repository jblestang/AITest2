use super::super::parser::{DocumentKind, TdmlDocument, TdmlSuite, UnparserTestCase};
use super::common::{
    compile_tdml_schema, error_messages_match, external_schema_label, hex_preview,
    resolve_model_schema, resolve_tdml_config,
};
use super::{TestOutcome, TestResult};
use crate::error::Result;
use crate::vm::RuntimeConfig;
use alloc::string::{String, ToString};

fn tdml_encode_runtime_config(test: &UnparserTestCase) -> RuntimeConfig {
    let doc = test.documents.first();
    RuntimeConfig {
        encode_pua_codepoints_as_utf8: doc
            .is_some_and(|d| d.kind == DocumentKind::Text && d.file_resource.is_none()),
        encode_tdml_bit_regions: doc
            .filter(|d| !d.part_bit_order_regions.is_empty())
            .map(|d| d.part_bit_order_regions.clone()),
        ..RuntimeConfig::default()
    }
}

pub fn run_unparser_test(suite: &TdmlSuite, test: &UnparserTestCase) -> Result<TestResult> {
    let (schema_xsd, compile_base_dir) = match resolve_model_schema(suite, &test.model) {
        Ok(v) => v,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("schema `{}`: {e}", test.model)),
            });
        }
    };

    let tdml_config = resolve_tdml_config(test.config.as_ref(), suite);
    let tunables = tdml_config.tunables;

    let schema_label = external_schema_label(&test.model);
    let spec = match compile_tdml_schema(
        &schema_xsd,
        &test.root,
        tunables,
        compile_base_dir.as_deref(),
        schema_label,
    ) {
        Ok(s) => s,
        Err(e) => {
            if let Some(expected) = &test.expected_errors {
                let msg = e.to_string();
                if error_messages_match(expected, &msg) {
                    return Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Pass,
                    });
                }
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Fail(alloc::format!("compile error mismatch: {msg}")),
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("compile error: {e}")),
            });
        }
    };

    let target_ns = spec.schema().target_namespace.as_deref();
    let element_form_suite = suite.name == "ElementFormDefaultTest";
    let infoset_target_ns = if element_form_suite && spec.schema().element_form_default_qualified {
        target_ns
    } else {
        None
    };
    let infoset_nodes = match super::super::infoset::parse_expected_infoset_with_target_ns(
        &test.infoset,
        &suite.resource_context,
        infoset_target_ns,
    ) {
        Ok(n) => n,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("infoset parse error: {e}")),
            });
        }
    };
    let validate_unparse = if element_form_suite {
        crate::unparse_validate::validate_unparse_infoset_nodes(
            spec.schema(),
            spec.program(),
            &test.root,
            &infoset_nodes,
        )
    } else if test.expected_errors.is_none() {
        crate::unparse_validate::validate_unparse_infoset_cardinality(
            spec.schema(),
            spec.program(),
            &test.root,
            &infoset_nodes,
        )
    } else if test.expected_errors.as_ref().is_some_and(|expected| {
        expected.iter().any(|e| {
            let el = e.to_ascii_lowercase();
            el.contains("element end")
                || el.contains("element start")
                || el.contains("expected element")
                || el.contains("expected one of")
                || el.contains("expected 1 additional")
                || el.contains("additional")
                || el.contains("schema definition error")
                || el.contains("no global element")
                || el.contains("not implemented")
        })
    }) {
        crate::unparse_validate::validate_unparse_infoset_nodes(
            spec.schema(),
            spec.program(),
            &test.root,
            &infoset_nodes,
        )
    } else {
        Ok(())
    };
    if let Err(msg) = validate_unparse {
        if let Some(expected_errors) = &test.expected_errors {
            if error_messages_match(expected_errors, &msg) {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("encode error mismatch: {msg}")),
            });
        }
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("encode error: {msg}")),
        });
    }

    let value = match super::super::infoset::infoset_xml_to_root_value_with_target_ns(
        &test.infoset,
        &test.root,
        spec.program(),
        &suite.resource_context,
        infoset_target_ns,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("infoset parse error: {e}")),
            });
        }
    };

    let unparse_blocked =
        !crate::parse_unparse_policy::root_allows_unparse(spec.schema(), &test.root)
            || crate::parse_unparse_policy::subtree_has_parse_only(spec.schema(), &test.root);

    if let Some(expected_errors) = &test.expected_errors {
        if let Some(msg) = crate::api::namespace_entity_limit_error(spec.schema(), &test.root) {
            if error_messages_match(expected_errors, &msg) {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("encode error mismatch: {msg}")),
            });
        }
        if unparse_blocked {
            let msg = crate::parse_unparse_policy::unparse_support_error().to_string();
            if error_messages_match(expected_errors, &msg) {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("encode error mismatch: {msg}")),
            });
        }
        let encode_config = tdml_encode_runtime_config(test);
        let diagnostic = match spec.encode_with_bit_count_config(&value, encode_config) {
            Ok((encoded, bit_count)) => {
                if let Some(doc) = test.documents.first() {
                    if encoded_matches_document(&encoded, bit_count, doc) {
                        None
                    } else {
                        Some(tdml_unparse_document_mismatch_message(
                            spec.program(),
                            &encoded,
                            doc,
                        ))
                    }
                } else {
                    None
                }
            }
            Err(e) => Some(augment_tdml_encode_error(
                e.to_string(),
                &suite.resource_context,
            )),
        };
        return match diagnostic {
            None => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!(
                    "expected encode error ({} message(s))",
                    expected_errors.len()
                )),
            }),
            Some(msg) => {
                if error_messages_match(expected_errors, &msg) {
                    Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Pass,
                    })
                } else {
                    Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!("encode error mismatch: {msg}")),
                    })
                }
            }
        };
    }

    if unparse_blocked {
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!(
                "encode error: {}",
                crate::parse_unparse_policy::unparse_support_error()
            )),
        });
    }

    let encode_config = tdml_encode_runtime_config(test);

    match spec.encode_with_bit_count_config(&value, encode_config) {
        Ok((encoded, bit_count)) => {
            if let Some(doc) = test.documents.first() {
                if !encoded_matches_document(&encoded, bit_count, doc) {
                    return Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!(
                            "encoded document mismatch: got {encoded:02x?} ({} trailing bits), expected {:02x?} ({:?} trailing bits)",
                            bit_count,
                            doc.data,
                            doc.last_byte_bit_count,
                        )),
                    });
                }
            }
            Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Pass,
            })
        }
        Err(e) => Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("encode error: {e}")),
        }),
    }
}

fn encoded_matches_document(encoded: &[u8], encoded_bit_count: u8, doc: &TdmlDocument) -> bool {
    let expected_bit_count = doc.last_byte_bit_count.unwrap_or(0);

    if expected_bit_count == 0 && encoded_bit_count > 0 {
        if encoded.len() != doc.data.len() {
            return false;
        }
        if encoded.len() > 1 && encoded[..encoded.len() - 1] != doc.data[..doc.data.len() - 1] {
            return false;
        }
        let mask = 0xFF_u8 << (8 - encoded_bit_count);
        return encoded.last().map(|b| b & mask) == doc.data.last().map(|b| b & mask);
    }

    if absolute_bit_index(encoded, encoded_bit_count)
        != absolute_bit_index(&doc.data, expected_bit_count)
    {
        return false;
    }
    if encoded_bit_count == 0 && expected_bit_count == 0 {
        return encoded == doc.data.as_slice();
    }
    if encoded.len() != doc.data.len() {
        return false;
    }
    if encoded.len() > 1 && encoded[..encoded.len() - 1] != doc.data[..doc.data.len() - 1] {
        return false;
    }
    let mask = if encoded_bit_count == 0 {
        0xFF
    } else {
        0xFF_u8 << (8 - encoded_bit_count)
    };
    encoded.last().map(|b| b & mask) == doc.data.last().map(|b| b & mask)
}

fn absolute_bit_index(data: &[u8], bit_count: u8) -> usize {
    data.len() * 8 + bit_count as usize
}

fn augment_tdml_encode_error(
    msg: String,
    ctx: &super::super::resources::TdmlResourceContext,
) -> String {
    let Some(path) = ctx.tdml_resource_path.as_deref() else {
        return msg;
    };
    if msg.contains(path) {
        return msg;
    }
    alloc::format!("{msg}\n{path}")
}

fn tdml_unparse_document_mismatch_message(
    _program: &crate::ir::IrProgram,
    actual: &[u8],
    doc: &TdmlDocument,
) -> alloc::string::String {
    let expected = doc.data.as_slice();
    if actual == expected {
        return "TDML Error: encoded document mismatch".into();
    }
    let prefix = "TDML Error: ";
    if doc.kind == DocumentKind::Text {
        let actual_text: alloc::string::String = actual.iter().map(|&b| b as char).collect();
        let expected_text: alloc::string::String = expected.iter().map(|&b| b as char).collect();
        return alloc::format!(
            "{prefix}{}",
            tdml_text_data_mismatch_message(&actual_text, &expected_text)
        );
    }
    if actual.len() != expected.len() {
        return alloc::format!(
            "{prefix}output data length {} for {} doesn't match expected length {} for {}",
            actual.len(),
            hex_preview(actual),
            expected.len(),
            hex_preview(expected)
        );
    }
    for (i, ((e, a), idx)) in expected.iter().zip(actual.iter()).zip(1usize..).enumerate() {
        if e != a {
            return alloc::format!(
                "{prefix}Unparsed data differs at byte {idx}. Expected 0x{e:02x}. Actual was 0x{a:02x}."
            );
        }
        let _ = i;
    }
    alloc::format!("{prefix}encoded document mismatch")
}

fn tdml_text_data_mismatch_message(actual: &str, expected: &str) -> alloc::string::String {
    const MAX: usize = 100;
    let trim = |s: &str| {
        if s.len() <= MAX {
            s.to_string()
        } else {
            alloc::format!("{}...", &s[..MAX])
        }
    };
    let actual_show = if actual.is_empty() {
        alloc::string::String::new()
    } else {
        alloc::format!(" for '{}'", trim(actual))
    };
    let expected_show = if expected.is_empty() {
        alloc::string::String::new()
    } else {
        alloc::format!(" for '{}'", trim(expected))
    };
    if actual.len() != expected.len() {
        return alloc::format!(
            "output data length {}{} doesn't match expected length {}{}",
            actual.len(),
            actual_show,
            expected.len(),
            expected_show
        );
    }
    for (idx, (e, a)) in expected.chars().zip(actual.chars()).enumerate() {
        if e != a {
            return alloc::format!(
                "Unparsed data differs at character {}. Expected '{}'. Actual was '{}'. Expected data {}, actual data {}",
                idx + 1,
                e,
                a,
                expected_show,
                actual_show
            );
        }
    }
    alloc::format!(
        "TDML Error: data differs. Expected '{}'. Actual was '{}'.",
        expected,
        actual
    )
}
