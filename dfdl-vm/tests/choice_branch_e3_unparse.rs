use dfdl_vm::tdml::{parse_tdml, run_unparser_test, TestOutcome};

#[test]
fn choice_branch_e3_unparse() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section15/choice_groups/ChoiceBranches.tdml"
    );
    let suite = parse_tdml(tdml).expect("parse");
    let test = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == "choiceBranch_e3")
        .expect("test");
    let result = run_unparser_test(&suite, test).expect("run");
    assert!(matches!(result.outcome, TestOutcome::Pass), "{:?}", result.outcome);
}
