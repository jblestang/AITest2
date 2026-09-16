use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;
use std::time::{Duration, Instant};

/// Regression: ordered sequence decode must not spin when optional
/// discriminators skip a slot (ockImplicit24 / repetition SDE cases).
#[test]
fn ock_implicit24_finishes_quickly() {
    let path = "/workspace/third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section14/occursCountKind/ockImplicit.tdml";
    let suite = parse_tdml(&fs::read_to_string(path).unwrap()).unwrap();
    let t = suite.tests.iter().find(|x| x.name == "ockImplicit24").unwrap();
    let start = Instant::now();
    let r = run_parser_test(&suite, t).unwrap();
    assert!(start.elapsed() < Duration::from_secs(3), "decode hung");
    assert!(matches!(r.outcome, TestOutcome::Pass));
}
