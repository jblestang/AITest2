use dfdl_vm::tdml::{parse_tdml, run_parser_test};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
);

fn check(name: &str) {
    let suite = parse_tdml(TDML).expect("parse");
    let test = suite.tests.iter().find(|t| t.name == name).expect("test");
    let r = run_parser_test(&suite, test).expect("run");
    eprintln!("{name}: {:?}", r.outcome);
}

#[test]
#[ignore]
fn debug_section12_failures() {
    for name in [
        "implicitAlignmentUnsignedIntT2",
        "implicitAlignmentByteT2",
        "leftAndRightFramingNested01",
        "alignmentStringErr",
        "implicitAlignmentDoubleT_Fail",
        "leftFraming01",
    ] {
        check(name);
    }
}
