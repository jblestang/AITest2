use dfdl_vm::tdml::{run_suite, TestOutcome};

#[test]
fn daffodil_ai_length_kind_pattern() {
    let tdml = include_str!("../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AI.tdml");
    let results = run_suite(tdml).expect("parse tdml");
    assert!(!results.is_empty(), "expected test cases");
    for result in &results {
        match &result.outcome {
            TestOutcome::Pass => {}
            TestOutcome::Fail(msg) => panic!("test `{}` failed: {msg}", result.name),
            TestOutcome::Skip(msg) => panic!("test `{}` skipped: {msg}", result.name),
        }
    }
}
