use super::infoset::{compare_infoset, infoset_xml_to_root_value};
use super::parser::{
    effective_round_trip, parse_tdml, ParserTestCase, RoundTrip, TdmlDocument, TdmlSuite,
    UnparserTestCase,
};
use crate::api::DfdlSpec;
use crate::length_validate::DaffodilTunables;
use crate::error::Result;
use crate::vm::RuntimeConfig;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Outcome of running one TDML parser test case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    Pass,
    Fail(String),
    Skip(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub name: String,
    pub outcome: TestOutcome,
}

/// Run all parser test cases in a suite.
pub fn run_suite(tdml: &str) -> Result<Vec<TestResult>> {
    let suite = parse_tdml(tdml)?;
    let mut results = Vec::new();
    for test in &suite.tests {
        results.push(run_parser_test(&suite, test)?);
    }
    Ok(results)
}

/// Options for [`run_parser_test_with_options`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParserTestRunOptions {
    /// Verify byte-identical encode for `roundTrip="onePass"` / `"twoPass"` tests.
    pub verify_round_trip: bool,
    /// For `roundTrip="false"` tests, verify decode → encode → decode preserves infoset.
    pub verify_canonical_round_trip: bool,
}

/// Run a single parser test case from an already-parsed suite.
pub fn run_parser_test(suite: &TdmlSuite, test: &ParserTestCase) -> Result<TestResult> {
    run_parser_test_with_options(suite, test, ParserTestRunOptions::default())
}

/// Run a parser test case, optionally verifying roundtrip behavior after a successful parse.
pub fn run_parser_test_with_options(
    suite: &TdmlSuite,
    test: &ParserTestCase,
    options: ParserTestRunOptions,
) -> Result<TestResult> {
    let schema_xsd = match resolve_model_schema(suite, &test.model) {
        Ok(xsd) => xsd,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("schema `{}`: {e}", test.model)),
            });
        }
    };

    let tunables = test
        .config
        .as_ref()
        .and_then(|name| suite.configs.get(name))
        .copied()
        .unwrap_or_default();

    let spec = match compile_tdml_schema(&schema_xsd, &test.root, tunables) {
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

    if test.documents.is_empty() {
        return Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Skip("no document".into()),
        });
    }

    let doc = &test.documents[0];
    let config = RuntimeConfig {
        strict_eos: true,
    };

    if let Some(expected_errors) = &test.expected_errors {
        return match spec.decoder_with_config(config).decode(&doc.data) {
            Ok(_) => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!(
                    "expected decode error ({} message(s))",
                    expected_errors.len()
                )),
            }),
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

    let decoded = match spec.decoder_with_config(config).decode(&doc.data) {
        Ok(v) => v,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("decode error: {e}")),
            });
        }
    };

    match compare_infoset(&decoded, &test.expected_infoset) {
        Ok(()) => {
            let rt = effective_round_trip(test.round_trip, suite.default_round_trip);
            let should_verify = options.verify_round_trip
                && matches!(rt, RoundTrip::TwoPass | RoundTrip::OnePass);
            if should_verify {
                match spec.encode(&decoded) {
                    Ok(encoded) if encoded == doc.data => {
                        if rt == RoundTrip::TwoPass {
                            match spec.decode(&encoded) {
                                Ok(redecoded) => {
                                    if compare_infoset(&redecoded, &test.expected_infoset).is_ok() {
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
                            doc.data.len(),
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
                            if compare_infoset(&redecoded, &test.expected_infoset).is_ok() {
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

/// Run a single unparser test case from an already-parsed suite.
pub fn run_unparser_test(suite: &TdmlSuite, test: &UnparserTestCase) -> Result<TestResult> {
    let schema_xsd = match resolve_model_schema(suite, &test.model) {
        Ok(xsd) => xsd,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("schema `{}`: {e}", test.model)),
            });
        }
    };

    let tunables = test
        .config
        .as_ref()
        .and_then(|name| suite.configs.get(name))
        .copied()
        .unwrap_or_default();

    let spec = match compile_tdml_schema(&schema_xsd, &test.root, tunables) {
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

    let value = match infoset_xml_to_root_value(&test.infoset, &test.root, spec.program()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("infoset parse error: {e}")),
            });
        }
    };

    if let Some(expected_errors) = &test.expected_errors {
        return match spec.encode(&value) {
            Ok(_) => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!(
                    "expected encode error ({} message(s))",
                    expected_errors.len()
                )),
            }),
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
                        outcome: TestOutcome::Fail(alloc::format!("encode error mismatch: {msg}")),
                    })
                }
            }
        };
    }

    match spec.encode_with_bit_count(&value) {
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

    // Hex/byte TDML documents often store bit-stream data in full bytes; infer trailing
    // significant bits from the encoded output when the expected doc is byte-aligned.
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

fn normalize_error_text(text: &str) -> alloc::string::String {
    text.replace('\n', "%NL;")
        .replace('\r', "%CR;")
        .replace('\t', "%HT;")
        .to_lowercase()
}

fn error_messages_match(expected: &[String], err: &str) -> bool {
    const OPTIONAL: &[&str] = &[
        "schema definition error",
        "parse error",
        "unparse error",
    ];
    let err_lower = normalize_error_text(err);
    expected.iter().all(|fragment| {
        let fragment = fragment.trim();
        if fragment.is_empty() {
            return true;
        }
        let fl = normalize_error_text(fragment);
        if OPTIONAL.contains(&fl.as_str()) {
            return true;
        }
        err_lower.contains(&fl)
    })
}

fn compile_tdml_schema(xsd: &str, root: &str, tunables: DaffodilTunables) -> Result<DfdlSpec> {
    let schema = crate::schema::parse_schema(xsd)?;
    DfdlSpec::from_schema_root_with_tunables(schema, Some(root), tunables)
}

fn resolve_model_schema(suite: &TdmlSuite, model: &str) -> Result<alloc::string::String> {
    if let Some(def) = suite.schemas.get(model) {
        return Ok(def.xsd.clone());
    }
    let resolver = crate::schema::SchemaResolver::new();
    resolver.resolve(model).map_err(Into::into)
}
