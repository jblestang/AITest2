use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;
const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/SimpleTypes.tdml"
);

fn run(case: &str) -> TestOutcome {
    let text = fs::read_to_string(TDML).expect("read");
    let suite = parse_tdml(&text).expect("parse");
    let t = suite
        .tests
        .iter()
        .find(|t| t.name == case)
        .unwrap_or_else(|| panic!("missing {case}"));
    run_parser_test(&suite, t).expect("run").outcome
}

#[test]
fn date_time_bin4_timezone() {
    assert!(matches!(run("dateTimeBin4"), TestOutcome::Pass));
}

#[test]
fn time_bin_bcd_smoke() {
    assert!(matches!(run("timeBinBCD"), TestOutcome::Pass));
}

#[test]
fn time_bin_bcd2_fraction() {
    assert!(matches!(run("timeBinBCD2"), TestOutcome::Pass));
}
