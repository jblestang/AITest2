use dfdl_vm::tdml::parse_tdml;

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/BitOrder.tdml"
);

#[test]
fn page70_and_mil_urn_document_load() {
    let suite = parse_tdml(TDML).expect("parse");
    for name in [
        "TestMIL2045_47001D_Page70_TableB_I_with_string",
        "TestMIL2045_47001D_1",
    ] {
        let test = suite.tests.iter().find(|t| t.name == name).expect(name);
        let doc = &test.documents[0];
        assert!(
            doc.load_error.is_none(),
            "{name}: load error {:?}",
            doc.load_error
        );
        assert!(!doc.data.is_empty(), "{name}: empty document");
        eprintln!("{name}: len={} first_bytes={:02x?}", doc.data.len(), &doc.data[..doc.data.len().min(4)]);
    }
}
