use dfdl_vm::tdml::parse_tdml;
use std::fs;

#[test]
fn bitorder_and_blobs_tdml_parse() {
    for rel in [
        "section05/simple_types/BitOrder.tdml",
        "section05/simple_types/Blobs.tdml",
    ] {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/",
        )
        .to_string()
            + rel;
        let text = fs::read_to_string(&path).unwrap();
        parse_tdml(&text).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
    }
}
