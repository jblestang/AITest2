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
