use dfdl_vm::tdml::{parse_tdml, run_unparser_test, TestOutcome};
use std::fs;
use std::path::Path;

#[test]
fn output_value_calc_04_unparse() {
    let tdml_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section17/calc_value_properties/outputValueCalc.tdml",
    );
    let tdml = fs::read_to_string(&tdml_path).expect("tdml");
    let suite = parse_tdml(&tdml).expect("parse");
    let test = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == "OutputValueCalc_04")
        .expect("test");
    let result = run_unparser_test(&suite, test).expect("run");
    eprintln!("{:?}", result.outcome);
    assert!(matches!(result.outcome, TestOutcome::Pass));
}

#[test]
fn ovc_w_runtime_cal_lang_debug() {
    let tdml_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section17/calc_value_properties/outputValueCalc2.tdml",
    );
    let tdml = fs::read_to_string(&tdml_path).expect("tdml");
    let suite = parse_tdml(&tdml).expect("parse");
    let test = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == "ovc_w_runtime_cal_lang")
        .expect("test");
    let result = run_unparser_test(&suite, test).expect("run");
    eprintln!("{:?}", result.outcome);
    assert!(matches!(result.outcome, TestOutcome::Pass));
}
