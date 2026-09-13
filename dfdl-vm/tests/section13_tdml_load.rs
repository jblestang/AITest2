//! TDML loader checks for Section 13 zoned suites (runtime zoned decode still WIP).
use dfdl_vm::tdml::parse_tdml;
use std::fs;

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section13"
);

#[test]
fn section13_zoned_tdml_files_parse() {
    for rel in ["zoned/zoned.tdml", "zoned/zoned2.tdml"] {
        let path = format!("{ROOT}/{rel}");
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        parse_tdml(&text).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
    }
}
