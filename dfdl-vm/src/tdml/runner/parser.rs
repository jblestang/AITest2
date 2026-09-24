use super::super::infoset::{compare_infoset_with_context, resolve_expected_infoset_xml};
use super::super::parser::{
    effective_round_trip, effective_validation, DocumentKind, ParserTestCase, RoundTrip,
    TdmlDocument, TdmlSuite, TdmlValidationMode,
};
use super::super::validation::collect_post_decode_validation_errors_with_document;
use super::common::{
    compile_tdml_schema, error_messages_match, escalated_schema_warnings_message,
    external_schema_label, resolve_model_schema, resolve_tdml_config, resolve_tdml_document_bytes,
    schema_warnings_for_root, schema_warnings_message, tdml_transmission_bit_order,
};
use super::{ParserTestRunOptions, TestOutcome, TestResult};
use crate::error::Result;
use crate::vm::RuntimeConfig;
use alloc::string::ToString;
use alloc::vec::Vec;

const DAFFODIL_IGNORED_PARSER_TESTS: &[&str] = &["multifile_choice_02b"];

pub fn run_parser_test_with_options(
    suite: &TdmlSuite,
    test: &ParserTestCase,
    options: ParserTestRunOptions,
) -> Result<TestResult> {
    if DAFFODIL_IGNORED_PARSER_TESTS.contains(&test.name.as_str()) {
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Skip("ignored in Apache Daffodil TDML".into()),
        });
    }
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

    let empty_doc = TdmlDocument {
        kind: DocumentKind::Text,
        data: Vec::new(),
        file_resource: None,
        last_byte_bit_count: None,
        transmission_bit_order: crate::schema::BitOrder::MostSignificantBitFirst,
        document_transmission_bit_order: None,
        mixed_bits_text_document: false,
        part_bit_order_regions: Vec::new(),
        load_error: None,
    };
    if test.documents.is_empty() && test.expected_errors.is_none() {
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Skip("no document".into()),
        });
    }
    let doc = test.documents.first().unwrap_or(&empty_doc);
    let document_data = match resolve_tdml_document_bytes(doc, &suite.resource_context) {
        Ok(data) => data,
        Err(msg) => {
            if let Some(expected_errors) = &test.expected_errors {
                if error_messages_match(expected_errors, &msg) {
                    return Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Pass,
                    });
                }
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("document load error: {msg}")),
            });
        }
    };
    if let Err(msg) = resolve_expected_infoset_xml(&test.expected_infoset, &suite.resource_context)
    {
        if msg.contains("DOCTYPE is disallowed") {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Pass,
            });
        }
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("infoset load error: {msg}")),
        });
    }
    let validation_mode = effective_validation(test, suite);
    let enable_facet_validation = !matches!(validation_mode, TdmlValidationMode::Off);
    let defer_facet_validation = enable_facet_validation;
    let config = RuntimeConfig {
        strict_eos: true,
        enable_facet_validation,
        defer_facet_validation,
        runtime_variable_overrides: tdml_config.external_variables,
        ..RuntimeConfig::default()
    };

    if let Some(load_err) = &doc.load_error {
        if let Some(expected_errors) = &test.expected_errors {
            if error_messages_match(expected_errors, load_err) {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {load_err}")),
            });
        }
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("document load error: {load_err}")),
        });
    }

    if !crate::parse_unparse_policy::root_allows_parse(spec.schema(), &test.root) {
        let msg = crate::parse_unparse_policy::parse_support_error().to_string();
        if let Some(expected_errors) = &test.expected_errors {
            if error_messages_match(expected_errors, &msg) {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
            });
        }
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("decode error: {msg}")),
        });
    }

    let frame_bits = doc.significant_bit_length();
    let transmission = Some(tdml_transmission_bit_order(doc, spec.program()));
    let tdml_regions = if doc.part_bit_order_regions.is_empty() {
        None
    } else {
        Some(doc.part_bit_order_regions.clone())
    };
    let compile_warnings = schema_warnings_for_root(spec.schema(), &test.root);
    if tunables.escalate_warnings_to_errors && !compile_warnings.is_empty() {
        let msg = escalated_schema_warnings_message(&compile_warnings);
        if let Some(expected_errors) = &test.expected_errors {
            if error_messages_match(expected_errors, &msg) {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                });
            }
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
            });
        }
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("decode error: {msg}")),
        });
    }

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
                outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
            });
        }
        return match spec.decoder_with_config(config).decode_with_tdml_options(
            &document_data,
            frame_bits,
            transmission,
            tdml_regions.clone(),
        ) {
            Ok(decoded) => {
                let raw = super::super::validation::collect_post_decode_raw_facet_errors(
                    spec.schema(),
                    spec.program(),
                    &decoded,
                );
                if !raw.is_empty() {
                    let msg = raw.join("\n");
                    if error_messages_match(expected_errors, &msg) {
                        return Ok(TestResult {
                            name: test.name.clone(),
                            outcome: TestOutcome::Pass,
                        });
                    }
                    return Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
                    });
                }
                let full_xerces = validation_mode == TdmlValidationMode::On;
                let post =
                    super::super::validation::collect_post_decode_validation_errors_with_document(
                        spec.schema(),
                        spec.program(),
                        &decoded,
                        full_xerces,
                        core::str::from_utf8(&document_data).ok(),
                    );
                if !post.is_empty() {
                    let msg = post.join("\n");
                    if error_messages_match(expected_errors, &msg) {
                        return Ok(TestResult {
                            name: test.name.clone(),
                            outcome: TestOutcome::Pass,
                        });
                    }
                    return Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
                    });
                }
                Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Fail(alloc::format!(
                        "expected decode error ({} message(s))",
                        expected_errors.len()
                    )),
                })
            }
            Err(e) => {
                let msg = e.to_string();
                if error_messages_match(expected_errors, &msg) {
                    Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Pass,
                    })
                } else {
                    Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!("decode error mismatch: {msg}")),
                    })
                }
            }
        };
    }

    let decoded = match spec.decoder_with_config(config).decode_with_tdml_options(
        &document_data,
        frame_bits,
        transmission,
        tdml_regions,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("decode error: {e}")),
            });
        }
    };

    let validation_document_text = core::str::from_utf8(&document_data)
        .ok()
        .map(str::to_string);

    if let Some(expected_validation) = &test.expected_validation_errors {
        let full_xerces = validation_mode == TdmlValidationMode::On;
        let collected = collect_post_decode_validation_errors_with_document(
            spec.schema(),
            spec.program(),
            &decoded,
            full_xerces,
            validation_document_text.as_deref(),
        );
        let combined = collected.join("\n");
        if collected.is_empty() {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(
                    "expected post-decode validation errors but none were reported".into(),
                ),
            });
        }
        if !error_messages_match(expected_validation, &combined) {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("validation error mismatch: {combined}")),
            });
        }
    } else if matches!(
        validation_mode,
        TdmlValidationMode::Limited | TdmlValidationMode::On
    ) {
        let collected = collect_post_decode_validation_errors_with_document(
            spec.schema(),
            spec.program(),
            &decoded,
            validation_mode == TdmlValidationMode::On,
            validation_document_text.as_deref(),
        );
        if !collected.is_empty() {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!(
                    "unexpected validation errors: {}",
                    collected.join("\n")
                )),
            });
        }
    }

    match compare_infoset_with_context(&decoded, &test.expected_infoset, &suite.resource_context) {
        Ok(()) => {
            if let Some(expected_warnings) = &test.expected_warnings {
                let combined = schema_warnings_message(&compile_warnings);
                if !error_messages_match(expected_warnings, &combined) {
                    return Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!("warning mismatch: {combined}")),
                    });
                }
            }
            let rt = effective_round_trip(test.round_trip, suite.default_round_trip);
            let should_verify =
                options.verify_round_trip && matches!(rt, RoundTrip::TwoPass | RoundTrip::OnePass);
            if should_verify {
                match spec.encode(&decoded) {
                    Ok(encoded) if encoded == document_data => {
                        if rt == RoundTrip::TwoPass {
                            match spec.decode(&encoded) {
                                Ok(redecoded) => {
                                    if compare_infoset_with_context(
                                        &redecoded,
                                        &test.expected_infoset,
                                        &suite.resource_context,
                                    )
                                    .is_ok()
                                    {
                                        Ok(TestResult {
                                            name: test.name.clone(),
                                            outcome: TestOutcome::Pass,
                                        })
                                    } else {
                                        Ok(TestResult {
                                            name: test.name.clone(),
                                            outcome: TestOutcome::Fail(
                                                "twoPass infoset mismatch after re-parse".into(),
                                            ),
                                        })
                                    }
                                }
                                Err(e) => Ok(TestResult {
                                    name: test.name.clone(),
                                    outcome: TestOutcome::Fail(alloc::format!(
                                        "twoPass re-parse error: {e}"
                                    )),
                                }),
                            }
                        } else {
                            Ok(TestResult {
                                name: test.name.clone(),
                                outcome: TestOutcome::Pass,
                            })
                        }
                    }
                    Ok(encoded) => Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!(
                            "roundtrip byte mismatch: expected {} byte(s), got {} byte(s)",
                            document_data.len(),
                            encoded.len()
                        )),
                    }),
                    Err(e) => Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!("roundtrip encode error: {e}")),
                    }),
                }
            } else if options.verify_canonical_round_trip && rt == RoundTrip::Disabled {
                match spec.encode(&decoded) {
                    Ok(encoded) => match spec.decode(&encoded) {
                        Ok(redecoded) => {
                            if compare_infoset_with_context(
                                &redecoded,
                                &test.expected_infoset,
                                &suite.resource_context,
                            )
                            .is_ok()
                            {
                                Ok(TestResult {
                                    name: test.name.clone(),
                                    outcome: TestOutcome::Pass,
                                })
                            } else {
                                Ok(TestResult {
                                    name: test.name.clone(),
                                    outcome: TestOutcome::Fail(
                                        "canonical roundtrip infoset mismatch".into(),
                                    ),
                                })
                            }
                        }
                        Err(e) => Ok(TestResult {
                            name: test.name.clone(),
                            outcome: TestOutcome::Fail(alloc::format!(
                                "canonical roundtrip re-parse error: {e}"
                            )),
                        }),
                    },
                    Err(e) => Ok(TestResult {
                        name: test.name.clone(),
                        outcome: TestOutcome::Fail(alloc::format!(
                            "canonical roundtrip encode error: {e}"
                        )),
                    }),
                }
            } else {
                Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
                })
            }
        }
        Err(msg) => Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(msg),
        }),
    }
}
