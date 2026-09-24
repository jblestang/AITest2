//! Temporary diagnostics for Section 13 cases (run with --ignored).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};
use std::fs;
use std::path::PathBuf;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn run(case: &str, tdml_rel: &str) -> TestOutcome {
    let text = fs::read_to_string(PathBuf::from(TDML).join(tdml_rel)).expect("read");
    let suite = parse_tdml(&text).expect("parse tdml");
    if let Some(name) = case.strip_prefix("unparse:") {
        let test = suite
            .unparser_tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("unparser case `{name}`"));
        return run_unparser_test(&suite, test).expect("run").outcome;
    }
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == case)
        .expect("parser case");
    run_parser_test(&suite, test).expect("run").outcome
}

#[test]
#[ignore]
fn debug_section13_cases() {
    for (case, file) in [
        (
            "standardZeroRep04b",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "standardZeroRep11",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "lengthDeterminedFirst02",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "dynamic",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "textStandardFloatPatternNoSeparators1",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "textNumberIntWithDecimal01",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "textNumberPaddingAmbiguity02",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "unparse:textStandardDecimalSeparator09u",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "unparse:textStandardFloatPatternNoSeparators2",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "unparse:textNumberRoundingIncrement1",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "unparse:textNumberIntWithDecimal02",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
        (
            "unparse:textNumberIntegerWithDecimal02",
            "section13/text_number_props/TextNumberProps.tdml",
        ),
    ] {
        eprintln!("{case}: {:?}", run(case, file));
    }
}
