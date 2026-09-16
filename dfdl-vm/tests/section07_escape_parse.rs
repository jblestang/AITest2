use dfdl_vm::tdml::parse_tdml;
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
