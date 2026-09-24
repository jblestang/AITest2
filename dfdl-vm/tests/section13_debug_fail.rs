//! Debug harness for remaining section13 failures.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test};
use std::fs;
use std::path::PathBuf;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn tdml(path: &str) -> PathBuf {
    PathBuf::from(TDML).join(path)
}

fn debug_parser(tdml_rel: &str, case_name: &str) {
    let text = fs::read_to_string(tdml(tdml_rel)).expect("read");
    let suite = parse_tdml(&text).expect("parse tdml");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == case_name)
        .expect("case");
    match run_parser_test(&suite, test) {
        Ok(r) => eprintln!("{case_name}: {:?}", r.outcome),
        Err(e) => eprintln!("{case_name}: compile/run err: {e}"),
    }
}

fn debug_unparser(tdml_rel: &str, case_name: &str) {
    let text = fs::read_to_string(tdml(tdml_rel)).expect("read");
    let suite = parse_tdml(&text).expect("parse tdml");
    let test = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == case_name)
        .expect("case");
    match run_unparser_test(&suite, test) {
        Ok(r) => eprintln!("{case_name}: {:?}", r.outcome),
        Err(e) => eprintln!("{case_name}: err: {e}"),
    }
}

#[test]
#[ignore]
fn debug_remaining_section13() {
    let cases = [
        ("section13/zoned/pv.tdml", "vpattern_09"),
        ("section13/zoned/pv.tdml", "vpattern_bad_02"),
        ("section13/zoned/pv.tdml", "bad_byte_vpattern_01"),
        ("section13/nillable/nillable2.tdml", "foo1"),
        ("section13/nillable/literal-character-nils.tdml", "text_03"),
        ("section13/nillable/nillable.tdml", "litNil4"),
        (
            "section13/nillable/literal-value-nils.tdml",
            "test_complex_nil",
        ),
        ("section13/nillable/nillable.tdml", "complexNillable_02"),
        ("section13/zoned/pv.tdml", "bad_byte_vpattern_01"),
    ];
    for (file, name) in cases {
        debug_parser(file, name);
    }
    debug_unparser(
        "section13/nillable/literal-value-nils-unparse.tdml",
        "text_complex_nil4",
    );
}
