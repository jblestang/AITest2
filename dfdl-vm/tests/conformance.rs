use dfdl_vm::tdml::{run_parser_test, parse_tdml, TestOutcome};

fn assert_named_test_passes(tdml: &str, test_name: &str) {
    let suite = parse_tdml(tdml).expect("parse tdml");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == test_name)
        .unwrap_or_else(|| panic!("test `{test_name}` not found"));
    let result = run_parser_test(&suite, test).expect("run test");
    match result.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("test `{test_name}` failed: {msg}"),
        TestOutcome::Skip(msg) => panic!("test `{test_name}` skipped: {msg}"),
    }
}

#[test]
fn daffodil_ai_length_kind_pattern() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AI.tdml"
    );
    assert_named_test_passes(tdml, "AI000");
}

#[test]
fn daffodil_explicit_length_address() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/ExplicitTests.tdml"
    );
    assert_named_test_passes(tdml, "Lesson1_lengthKind_explicit");
}

#[test]
fn daffodil_section12_pattern_suite() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
    );
    assert_named_test_passes(tdml, "AI000_rev");
}

#[test]
fn daffodil_length_kind_pattern_alternation() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
    );
    assert_named_test_passes(tdml, "lengthKindPattern_01");
}

#[test]
fn daffodil_length_kind_pattern_no_match() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
    );
    assert_named_test_passes(tdml, "lengthKindPattern_02");
}

#[test]
fn daffodil_length_kind_pattern_unicode_fail() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
    );
    assert_named_test_passes(tdml, "lengthKindPatternFail");
}
