use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;
use std::path::Path;

#[test]
fn escape_scheme_tdml_parses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section07/escapeScheme/escapeScheme.tdml",
    );
    let text = fs::read_to_string(&path).expect("read");
    parse_tdml(&text).unwrap_or_else(|e| panic!("parse escapeScheme.tdml: {e}"));
}

fn run_named_parser_case(name: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section07/escapeScheme/escapeScheme.tdml",
    );
    let text = fs::read_to_string(&path).expect("read");
    let suite = parse_tdml(&text).expect("parse");
    let t = suite.tests.iter().find(|t| t.name == name).expect("case");
    let r = run_parser_test(&suite, t).expect("run");
    assert_eq!(r.outcome, TestOutcome::Pass, "{:?}", r.outcome);
}

#[test]
fn escape_expressions_06() {
    run_named_parser_case("escapeExpressions_06");
}

#[test]
fn escape_expressions_07_and_08() {
    run_named_parser_case("escapeExpressions_07");
    run_named_parser_case("escapeExpressions_08");
}

#[test]
fn escape_scheme_with_comment() {
    run_named_parser_case("escapeScheme_with_comment");
}

#[test]
fn es7_inherits_format_escape_scheme() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section07/escapeScheme/escapeScheme.tdml",
    );
    let text = fs::read_to_string(&path).expect("read");
    let suite = parse_tdml(&text).expect("parse");
    let model = suite.schemas.get("es7").expect("model");
    let schema = dfdl_vm::schema::parse_schema(&model.xsd).expect("schema");
    let prog = dfdl_vm::ir::compile_named(&schema, Some("list")).expect("compile");
    for node in &prog.nodes {
        if let dfdl_vm::ir::IrNode::Element { name, props, .. } = node {
            let n = prog.strings.get(*name).expect("name");
            if n.ends_with("field") {
                assert_eq!(
                    props
                        .escape_scheme
                        .as_ref()
                        .and_then(|s| s.escape_character.as_deref()),
                    Some("#")
                );
            }
        }
    }
}
