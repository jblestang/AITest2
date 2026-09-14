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
fn byte_binary_02_leftover_bits() {
    assert!(matches!(run("byte_binary_02"), TestOutcome::Pass));
}

#[test]
fn double_binary_01_insufficient_bits() {
    assert!(matches!(run("double_binary_01"), TestOutcome::Pass));
}
