use dfdl_vm::api::DfdlSpec;
use dfdl_vm::tdml::parse_tdml;

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AB.tdml"
);

#[test]
fn ab004_roundtrip_debug() {
    let suite = parse_tdml(TDML).unwrap();
    let test = suite.tests.iter().find(|t| t.name == "AB004").unwrap();
    let xsd = suite
        .schemas
        .get("AB.dfdl.xsd")
        .map(|s| s.xsd.clone())
        .unwrap_or_else(|| {
            dfdl_vm::schema::SchemaResolver::new()
                .resolve("AB.dfdl.xsd")
                .unwrap()
        });
    let spec = DfdlSpec::from_xsd_root(&xsd, Some("matrix_02")).unwrap();
    let input = &test.documents[0].data;
    let decoded = spec.decode(input).expect("decode");
    eprintln!("decoded={decoded:?}");
    let encoded = spec.encode(&decoded).expect("encode");
    eprintln!("input={input:?}");
    eprintln!("encoded={encoded:?}");
    assert_eq!(encoded, *input);
}
