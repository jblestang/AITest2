use super::infoset::{compare_infoset, infoset_xml_to_root_value};
use super::parser::{parse_tdml, ParserTestCase, TdmlSuite, UnparserTestCase};
use crate::api::DfdlSpec;
use crate::error::Result;
use crate::vm::RuntimeConfig;
use alloc::string::String;
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

/// Run a single parser test case from an already-parsed suite.
pub fn run_parser_test(suite: &TdmlSuite, test: &ParserTestCase) -> Result<TestResult> {
    let schema_xsd = match resolve_model_schema(suite, &test.model) {
        Ok(xsd) => xsd,
        Err(e) => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("schema `{}`: {e}", test.model)),
            });
        }
    };

    let spec = match compile_tdml_schema(&schema_xsd, &test.root) {
        Ok(s) => s,
        Err(e) => {
            if test.expected_errors.is_some() {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
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

    if let Some(expected_errors) = test.expected_errors {
        return match spec.decoder_with_config(config).decode(&doc.data) {
            Ok(_) => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!(
                    "expected decode error ({expected_errors} error(s))"
                )),
            }),
            Err(_) => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Pass,
            }),
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
        Ok(()) => Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Pass,
        }),
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

    let spec = match compile_tdml_schema(&schema_xsd, &test.root) {
        Ok(s) => s,
        Err(e) => {
            if test.expected_errors.is_some() {
                return Ok(TestResult {
                    name: test.name.clone(),
                    outcome: TestOutcome::Pass,
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

    if let Some(expected_errors) = test.expected_errors {
        return match spec.encode(&value) {
            Ok(_) => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!(
                    "expected encode error ({expected_errors} error(s))"
                )),
            }),
            Err(_) => Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Pass,
            }),
        };
    }

    match spec.encode(&value) {
        Ok(_) => Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Pass,
        }),
        Err(e) => Ok(TestResult {
            name: test.name.clone(),
            outcome: TestOutcome::Fail(alloc::format!("encode error: {e}")),
        }),
    }
}

fn compile_tdml_schema(xsd: &str, root: &str) -> Result<DfdlSpec> {
    DfdlSpec::from_xsd_root(xsd, Some(root))
}

fn resolve_model_schema(suite: &TdmlSuite, model: &str) -> Result<alloc::string::String> {
    if let Some(def) = suite.schemas.get(model) {
        return Ok(def.xsd.clone());
    }
    let resolver = crate::schema::SchemaResolver::new();
    resolver.resolve(model).map_err(Into::into)
}
