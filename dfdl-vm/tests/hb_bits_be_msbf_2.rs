use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

#[test]
fn hex_binary_bits_be_msbf_2_tdml() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/SimpleTypes.tdml"
    );
    let tdml = std::fs::read_to_string(path).unwrap();
    let suite = dfdl_vm::tdml::parse_tdml(&tdml).unwrap();
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == "hexBinary_bits_be_msbf_2")
        .expect("test case");
    let result = dfdl_vm::tdml::run_parser_test(&suite, test).unwrap();
    assert!(
        matches!(result.outcome, TestOutcome::Pass),
        "{:?}",
        result.outcome
    );
}
