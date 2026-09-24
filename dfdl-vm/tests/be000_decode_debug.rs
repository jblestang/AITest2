use dfdl_vm::length_validate::DaffodilTunables;
use dfdl_vm::schema::{parse_schema, SchemaResolver};
use dfdl_vm::tdml::parse_tdml;
use dfdl_vm::value::DfdlValue;
use dfdl_vm::vm::RuntimeConfig;
use dfdl_vm::DfdlSpec;

#[test]
fn be000_y_count_from_tdml_bytes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section14/unordered_sequences/BE.tdml");
    let suite = parse_tdml(&std::fs::read_to_string(path).unwrap()).unwrap();
    let t = suite.tests.iter().find(|x| x.name == "BE000").unwrap();
    let data = t.documents.first().unwrap().data.clone();

    let resolver = SchemaResolver::new();
    let xsd = resolver
        .resolve("/org/apache/daffodil/section14/unordered_sequences/BE.dfdl.xsd")
        .unwrap();
    let schema = parse_schema(&xsd).unwrap();
    let spec =
        DfdlSpec::from_schema_root_with_tunables(schema, Some("seq"), DaffodilTunables::default())
            .unwrap();
    let config = RuntimeConfig {
        strict_eos: true,
        enable_facet_validation: true,
        defer_facet_validation: true,
        ..RuntimeConfig::default()
    };
    let decoded = spec
        .decoder_with_config(config)
        .decode_with_tdml_options(&data, None, None, None)
        .expect("decode");

    fn y_len(v: &DfdlValue) -> usize {
        let DfdlValue::Sequence(outer) = v else {
            return 0;
        };
        let inner = outer
            .fields
            .get("seq")
            .or_else(|| outer.fields.values().next());
        let Some(DfdlValue::Sequence(seq)) = inner else {
            return 0;
        };
        match seq.fields.get("y") {
            Some(DfdlValue::Array(a)) => a.len(),
            Some(_) => 1,
            None => 0,
        }
    }
    let n = y_len(&decoded);
    eprintln!("data={:?}", std::str::from_utf8(&data));
    assert_eq!(n, 3, "y count {n}, tdml bytes len={}", data.len());
}
