use dfdl_vm::tdml::parse_tdml;
use std::fs;
use std::path::Path;

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section07"
);

fn parse_rel(rel: &str) {
    let path = Path::new(ROOT).join(rel);
    let text = fs::read_to_string(&path).expect("read");
    parse_tdml(&text).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
}

#[test]
fn assert_tdml_parses() {
    parse_rel("assertions/assert.tdml");
}

#[test]
fn discriminator_tdml_parses() {
    parse_rel("discriminators/discriminator.tdml");
}

#[test]
fn nested_choice_discriminator_tdml_parses() {
    parse_rel("discriminators/nestedChoiceDiscriminator.tdml");
}

fn run_nested_choice_case(name: &str) {
    let path = Path::new(ROOT).join("discriminators/nestedChoiceDiscriminator.tdml");
    let text = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&text).expect("parse");
    suite.resource_context = dfdl_vm::tdml::TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let t = suite.tests.iter().find(|t| t.name == name).expect("case");
    let r = dfdl_vm::tdml::run_parser_test(&suite, t).expect("run");
    assert_eq!(r.outcome, dfdl_vm::tdml::TestOutcome::Pass, "{:?}", r.outcome);
}

#[test]
fn test_nested_choice_3() {
    let path = Path::new(ROOT).join("discriminators/nestedChoiceDiscriminator.tdml");
    let text = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&text).expect("parse");
    suite.resource_context = dfdl_vm::tdml::TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let t = suite.tests.iter().find(|t| t.name == "nestedChoice3").expect("case");
    let r = dfdl_vm::tdml::run_parser_test(&suite, t).expect("run");
    println!("RESULT 3: {r:#?}");
    assert_eq!(r.outcome, dfdl_vm::tdml::TestOutcome::Pass, "{:?}", r.outcome);
}

#[test]
fn test_nested_choice_3b() {
    run_nested_choice_case("nestedChoice3b");
}

#[test]
fn test_nested_choice_4() {
    run_nested_choice_case("nestedChoice4");
}

#[test]
fn escape_scheme_unparse_tdml_parses() {
    parse_rel("escapeScheme/escapeSchemeUnparse.tdml");
}

#[test]
fn set_var_with_value_length_tdml_parses() {
    parse_rel("variables/setVarWIthValueLength.tdml");
}

#[test]
fn variables_tdml_parses() {
    parse_rel("variables/variables.tdml");
}

fn run_variables_case(name: &str) {
    let path = Path::new(ROOT).join("variables/variables.tdml");
    let text = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&text).expect("parse");
    suite.resource_context = dfdl_vm::tdml::TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let t = suite.tests.iter().find(|t| t.name == name).expect("case");
    let r = dfdl_vm::tdml::run_parser_test(&suite, t).expect("run");
    assert_eq!(r.outcome, dfdl_vm::tdml::TestOutcome::Pass, "{:?}", r.outcome);
}

#[test]
fn test_var_instance_09() {
    run_variables_case("varInstance_09");
}

fn run_discriminator_case(name: &str) {
    let path = Path::new(ROOT).join("discriminators/discriminator.tdml");
    let text = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&text).expect("parse");
    suite.resource_context = dfdl_vm::tdml::TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let t = suite.tests.iter().find(|t| t.name == name).expect("case");
    let r = dfdl_vm::tdml::run_parser_test(&suite, t).expect("run");
    assert_eq!(r.outcome, dfdl_vm::tdml::TestOutcome::Pass, "{:?}", r.outcome);
}

#[test]
fn test_choice_branch_discrim_fail() {
    run_discriminator_case("choiceBranchDiscrimFail");
}

#[test]
fn test_discrim_expression_04() {
    run_discriminator_case("discrimExpression_04");
}
