use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
);

fn run(name: &str) {
    let suite = parse_tdml(TDML).expect("parse");
    let test = suite.tests.iter().find(|t| t.name == name).expect("test");
    let r = run_parser_test(&suite, test).expect("run");
    match r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{name}: {msg}"),
        TestOutcome::Skip(msg) => panic!("skip: {msg}"),
    }
}

#[test]
fn alignment01() {
    run("alignment01");
}

#[test]
fn alignment02() {
    run("alignment02");
}

#[test]
fn leading_skip2_tunable_limit() {
    run("leadingSkip2");
}

#[test]
fn implicit_alignment_unsigned_int_t2b() {
    run("implicitAlignmentUnsignedIntT2b");
}

#[test]
fn imp_alignment_non_negative_integer2() {
    run("impAlignmentNonNegativeInteger2");
}

#[test]
fn explicit_alignment_no_skips01() {
    run("explicitAlignmentNoSkips01");
}

#[test]
fn imp_alignment_hex_binary() {
    run("impAlignmentHexBinary");
}

#[test]
fn imp_alignment_integer3() {
    run("impAlignmentInteger3");
}

#[test]
fn imp_alignment_integer2() {
    run("impAlignmentInteger2");
}

#[test]
fn alignment03() {
    run("alignment03");
}

#[test]
fn explicit_alignment_no_skips03() {
    run("explicitAlignmentNoSkips03");
}

#[test]
fn explicit_alignment_no_skips04() {
    run("explicitAlignmentNoSkips04");
}

#[test]
fn explicit_alignment_no_skips05() {
    run("explicitAlignmentNoSkips05");
}

#[test]
fn implicit_alignment_long() {
    run("implicitAlignmentLong");
}

#[test]
fn implicit_alignment_unsigned_long() {
    run("implicitAlignmentUnsignedLong");
}

#[test]
fn implicit_alignment_double_t() {
    run("implicitAlignmentDoubleT");
}

#[test]
fn implicit_alignment_double_t2() {
    run("implicitAlignmentDoubleT2");
}
