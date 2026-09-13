//! Focused Section 13 smoke tests for schema/VM fixes (not a full gate).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;
use std::path::PathBuf;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn tdml(path: &str) -> PathBuf {
    PathBuf::from(TDML).join(path)
}

fn run_named(tdml_rel: &str, case_name: &str) -> TestOutcome {
    let text = fs::read_to_string(tdml(tdml_rel)).expect("read tdml");
    let suite = parse_tdml(&text).expect("parse tdml");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == case_name)
        .unwrap_or_else(|| panic!("case `{case_name}` not in {tdml_rel}"));
    let r = run_parser_test(&suite, test).expect("run parser test");
    r.outcome
}

#[test]
fn section13_literal_character_text_01() {
    match run_named("section13/nillable/literal-character-nils.tdml", "text_01") {
        TestOutcome::Pass => {}
        other => panic!("text_01: {other:?}"),
    }
}

#[test]
fn section13_text_standard_base_max_samples() {
    for name in ["base2_long_max", "base16_int_max", "base16_ulong_max"] {
        match run_named("section13/text_number_props/TextStandardBase.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_standard_base_schema_loads() {
    let text =
        fs::read_to_string(tdml("section13/text_number_props/TextStandardBase.tdml")).expect("read");
    parse_tdml(&text).expect("TextStandardBase.tdml should parse after textStandardBase support");
}

#[test]
fn section13_packed_hex_and_sign_cases() {
    for name in [
        "hexCharset01",
        "packedCharset01",
        "packedCharset02",
        "packedCharset03",
        "DelimitedPackedIntSeq",
        "DelimitedPackedDecSeq",
    ] {
        match run_named("section13/packed/packed.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
#[ignore = "slow; scans all parser cases in packed.tdml"]
fn section13_packed_tdml_scan() {
    use dfdl_vm::tdml::{run_parser_test, TestOutcome};
    let text = fs::read_to_string(tdml("section13/packed/packed.tdml")).expect("read packed");
    let suite = parse_tdml(&text).expect("parse packed");
    let mut pass = 0usize;
    let mut fail = 0usize;
    for test in &suite.tests {
        let Ok(r) = run_parser_test(&suite, test) else {
            fail += 1;
            continue;
        };
        match r.outcome {
            TestOutcome::Pass => pass += 1,
            TestOutcome::Fail(msg) => {
                eprintln!("FAIL {}: {msg}", test.name);
                fail += 1;
            }
            TestOutcome::Skip(_) => {}
        }
    }
    eprintln!("packed.tdml: pass={pass} fail={fail}");
    assert!(fail == 0, "packed regressions: pass={pass} fail={fail}");
}
