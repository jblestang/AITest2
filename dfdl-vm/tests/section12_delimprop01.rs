use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/delimiter_properties/DelimiterProperties.tdml"
);

fn run_case(name: &str) {
    let suite = parse_tdml(TDML).expect("parse");
    let test = suite.tests.iter().find(|t| t.name == name).expect("test");
    let r = run_parser_test(&suite, test).expect("run");
    match r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{name}: {msg}"),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn delimprop_01() {
    run_case("DelimProp_01");
}

#[test]
fn delimprop_07() {
    run_case("DelimProp_07");
}
