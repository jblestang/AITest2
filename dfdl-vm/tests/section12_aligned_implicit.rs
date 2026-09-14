use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
);

#[test]
fn implicit_alignment_unsigned_int() {
    let suite = parse_tdml(TDML).expect("parse");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == "implicitAlignmentUnsignedInt")
        .expect("test");
    let r = run_parser_test(&suite, test).expect("run");
    match r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{msg}"),
        TestOutcome::Skip(msg) => panic!("skip: {msg}"),
    }
}
