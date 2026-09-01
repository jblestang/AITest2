use super::infoset::compare_infoset;
use super::parser::{parse_tdml, ParserTestCase, TdmlSuite};
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
    let schema_def = match suite.schemas.get(&test.model) {
        Some(s) => s,
        None => {
            return Ok(TestResult {
                name: test.name.clone(),
                outcome: TestOutcome::Fail(alloc::format!("schema `{}` not found", test.model)),
            });
        }
    };

    let spec = match compile_tdml_schema(&schema_def.xsd, &test.root) {
        Ok(s) => s,
        Err(e) => {
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

fn compile_tdml_schema(xsd: &str, root: &str) -> Result<DfdlSpec> {
    DfdlSpec::from_xsd_root(xsd, Some(root))
}
