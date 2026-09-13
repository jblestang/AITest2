use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/delimiter_properties/DelimiterProperties.tdml"
);

fn assert_named_test_passes(test_name: &str) {
    let suite = parse_tdml(TDML).expect("parse tdml");
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
fn delims_ignorecase_01() {
    assert_named_test_passes("delims_ignorecase_01");
}

#[test]
fn delims_ignorecase_02() {
    assert_named_test_passes("delims_ignorecase_02");
}

#[test]
fn parse_sequence4_brace_escaping() {
    assert_named_test_passes("ParseSequence4");
}

#[test]
fn parse_sequence5_length_expr() {
    assert_named_test_passes("ParseSequence5");
}

#[test]
#[ignore = "prefix separator vs enclosing $$ conflict not yet detected"]
fn req_field_missing_prefix() {
    assert_named_test_passes("ReqFieldMissingAndSepIsPrefixOfTerminator_Prefix");
}

#[test]
fn percent_initiator_schema_error() {
    assert_named_test_passes("percentInitiator");
}

#[test]
fn empty_initiator_schema_error() {
    assert_named_test_passes("emptyInitiator1");
}

#[test]
fn parse_sequence5_cursor_steps() {
    let doc = b"[more[{{((55)),,((66)),,((77))}}]nomore]";
    let mut pos = 0usize;
    for (pat, expect) in [("[more[", 6usize), ("{{", 8), ("((", 10)] {
        let n = dfdl_vm::schema::match_delimiter_opts(&doc[pos..], pat, false).expect("match");
        pos += n;
        assert_eq!(pos, expect, "after `{pat}`");
    }
}
