use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

macro_rules! daffodil_tdml {
    ($file:literal) => {
        include_str!(concat!(
            "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/",
            $file
        ))
    };
}

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

// --- AI / pattern / explicit (Section 12) ---

#[test]
fn daffodil_ai_length_kind_pattern() {
    assert_named_test_passes(daffodil_tdml!("AI.tdml"), "AI000");
}

#[test]
fn daffodil_explicit_length_address() {
    assert_named_test_passes(daffodil_tdml!("ExplicitTests.tdml"), "Lesson1_lengthKind_explicit");
}

#[test]
fn daffodil_section12_pattern_suite() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "AI000_rev");
}

#[test]
fn daffodil_length_kind_pattern_alternation() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_01");
}

#[test]
fn daffodil_length_kind_pattern_no_match() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_02");
}

#[test]
fn daffodil_length_kind_pattern_comma() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_03");
}

#[test]
fn daffodil_length_kind_pattern_simple_type() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_04");
}

#[test]
fn daffodil_length_kind_pattern_unicode_fail() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPatternFail");
}

// --- Delimited (Section 12) ---

#[test]
fn daffodil_length_kind_delimited_address() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "Lesson1_lengthKind_delimited");
}

#[test]
fn daffodil_length_kind_delimited_int_terminator() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_00a");
}

#[test]
fn daffodil_length_kind_delimited_newline() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_00nl");
}

#[test]
fn daffodil_length_kind_delimited_unbounded_list() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_01");
}

#[test]
fn daffodil_length_kind_delimited_no_trailing_delim() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_03");
}

#[test]
fn daffodil_length_kind_delimited_nested_unbounded() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "nested_NumSeq_01");
}

#[test]
fn daffodil_length_kind_delimited_mixed_implicit() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_04");
}

// --- AN path delimited (Section 12) ---

#[test]
fn daffodil_an_path_with_file() {
    assert_named_test_passes(daffodil_tdml!("AN.tdml"), "AN000");
}

#[test]
fn daffodil_an_path_folders_only() {
    assert_named_test_passes(daffodil_tdml!("AN.tdml"), "AN001");
}

// --- Implicit binary (Section 12) ---

#[test]
fn daffodil_implicit_binary_ignored_length() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "implicit_ignored_len");
}

// --- endOfParent NYI negative (Section 12) ---

#[test]
fn daffodil_end_of_parent_nyi_simple() {
    assert_named_test_passes(
        daffodil_tdml!("EndOfParentTests.tdml"),
        "TestEndOfParentNYISimpleTypes",
    );
}
