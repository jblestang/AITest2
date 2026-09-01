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

// --- AB implicit CSV matrix (Section 12) ---

#[test]
fn daffodil_ab_implicit_csv_matrix() {
    assert_named_test_passes(daffodil_tdml!("AB.tdml"), "AB000");
}

#[test]
fn daffodil_ab_implicit_csv_postfix_separator() {
    assert_named_test_passes(daffodil_tdml!("AB.tdml"), "AB001");
}

#[test]
fn daffodil_ab_implicit_csv_nillable() {
    assert_named_test_passes(daffodil_tdml!("AB.tdml"), "AB002");
}

// --- Delimited: compound newline, eof, mixed sequences (Section 12) ---

#[test]
fn daffodil_delimited_double_newline_terminator() {
    assert_named_test_passes(
        daffodil_tdml!("DelimitedTests.tdml"),
        "TestDoubleNewLineTerminator",
    );
}

#[test]
fn daffodil_delimited_double_newline_separator() {
    assert_named_test_passes(
        daffodil_tdml!("DelimitedTests.tdml"),
        "TestDoubleNewLineSeparator",
    );
}

#[test]
fn daffodil_delimited_double_newline_separator_basic() {
    assert_named_test_passes(
        daffodil_tdml!("DelimitedTests.tdml"),
        "TestDoubleNewLineSeparatorBasic",
    );
}

#[test]
fn daffodil_delimited_eof_no_enclosing_patterns() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "eofTest1");
}

#[test]
fn daffodil_delimited_fixed_length_suspends_scanning() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "delimited_construct");
}

#[test]
fn daffodil_delimited_mixed_type_sequence() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_05");
}

#[test]
fn daffodil_delimited_nested_mixed_sequence() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_06");
}

#[test]
fn daffodil_delimited_initiator_on_element_ref() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "refInitiator");
}

#[test]
fn daffodil_delimited_initiator_on_element_decl() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "refInitiator2");
}

#[test]
fn daffodil_delimited_optional_nested_ref() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_07");
}

#[test]
fn daffodil_delimited_optional_nested_ref_min_zero() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_08");
}

#[test]
fn daffodil_delimited_explicit_length_complex() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_09");
}

#[test]
fn daffodil_delimited_nested_initiator_terminator() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_11");
}

#[test]
fn daffodil_delimited_empty_nested_group() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_12");
}

#[test]
fn daffodil_delimited_prefix_separator_complex() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_14");
}

#[test]
fn daffodil_delimited_compound_wsp_separator_space() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_01");
}

#[test]
fn daffodil_delimited_compound_wsp_separator_tab() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_02");
}

#[test]
fn daffodil_delimited_unused_trailing_bytes() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_03");
}

#[test]
fn daffodil_delimited_unused_trailing_bytes_no_extra_elem() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_04");
}

// --- Implicit complex element (Section 12) ---

#[test]
fn daffodil_implicit_complex_element_terminator() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "nested_seq");
}

#[test]
fn daffodil_implicit_complex_element_terminator_max_one() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "nested_seq_01");
}
