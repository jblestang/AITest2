use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/SimpleTypes.tdml"
);

fn run(case: &str) -> TestOutcome {
    let text = fs::read_to_string(TDML).expect("read");
    let suite = parse_tdml(&text).expect("parse");
    let t = suite.tests.iter().find(|t| t.name == case).unwrap();
    run_parser_test(&suite, t).expect("run").outcome
}

#[test]
fn decimal_binary_06_large_magnitude() {
    assert!(matches!(run("decimal_binary_06"), TestOutcome::Pass));
}

#[test]
fn non_negative_integer_bin6() {
    assert!(matches!(run("nonNegativeInteger_bin6"), TestOutcome::Pass));
}

#[test]
fn date_time_calendar_tz_empty() {
    assert!(matches!(
        run("dateTime_calendarTimeZone_EmptyString"),
        TestOutcome::Pass
    ));
}
