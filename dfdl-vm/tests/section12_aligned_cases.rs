use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
);

fn assert_passes(name: &str) {
    let suite = parse_tdml(TDML).expect("parse");
    let test = suite.tests.iter().find(|t| t.name == name).expect("test");
    let r = run_parser_test(&suite, test).expect("run");
    match r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{name}: {msg}"),
        TestOutcome::Skip(msg) => panic!("{name} skip: {msg}"),
    }
}

#[test]
fn alignment_terminator_bit_skip() {
    assert_passes("alignmentTerminatorBitSkip");
}

#[test]
fn right_framing01() {
    assert_passes("rightFraming01");
}

#[test]
fn left_and_right_framing01() {
    assert_passes("leftAndRightFraming01");
}

#[test]
fn alignment_string_bit_skip() {
    assert_passes("alignmentStringBitSkip");
}

#[test]
fn implicit_alignment_time_t() {
    assert_passes("implicitAlignmentTimeT");
}

#[test]
fn implicit_alignment_date_t2() {
    assert_passes("implicitAlignmentDateT2");
}

#[test]
fn implicit_alignment_date_time_t() {
    assert_passes("implicitAlignmentDateTimeT");
}
