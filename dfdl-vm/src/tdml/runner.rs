pub(crate) mod common;
pub(crate) mod parser;
pub(crate) mod unparser;

use super::parser::{parse_tdml, ParserTestCase, TdmlSuite};
use crate::error::Result;
use alloc::string::String;
use alloc::vec::Vec;

pub use unparser::run_unparser_test;

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

/// Options for [`run_parser_test_with_options`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParserTestRunOptions {
    /// Verify byte-identical encode for `roundTrip="onePass"` / `"twoPass"` tests.
    pub verify_round_trip: bool,
    /// For `roundTrip="false"` tests, verify decode → encode → decode preserves infoset.
    pub verify_canonical_round_trip: bool,
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
    run_parser_test_with_options(suite, test, ParserTestRunOptions::default())
}

pub use parser::run_parser_test_with_options;
