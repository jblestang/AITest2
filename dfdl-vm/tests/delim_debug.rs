use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/DelimitedTests.tdml"
);

fn run(name: &str) -> TestOutcome {
    let suite = parse_tdml(TDML).unwrap();
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("test `{name}` not found"));
    run_parser_test(&suite, test).unwrap().outcome
}

#[test]
fn delimited_tests_separator_mismatch_cluster() {
    for name in ["refInitiator", "refInitiator2", "NumSeq_05", "NumSeq_06"] {
        let outcome = run(name);
        assert!(
            matches!(outcome, TestOutcome::Pass),
            "{name} failed: {outcome:?}"
        );
    }
}
