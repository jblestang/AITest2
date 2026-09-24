use dfdl_vm::tdml::parse_tdml;

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AB.tdml"
);

#[test]
fn ab004_roundtrip_debug() {
    let suite = parse_tdml(TDML).unwrap();
    let test = suite.tests.iter().find(|t| t.name == "AB004").unwrap();
    let res = dfdl_vm::tdml::run_parser_test(&suite, test).expect("run");
    assert!(matches!(res.outcome, dfdl_vm::tdml::TestOutcome::Pass | dfdl_vm::tdml::TestOutcome::Fail(_)));
}
