use dfdl_vm::tdml::parse_tdml;
use dfdl_vm::{DfdlSpec, RuntimeConfig};
use std::fs;
use std::time::{Duration, Instant};

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/delimiter_properties/DelimiterProperties.tdml"
);

#[test]
fn delimprop_06_expected_error() {
    let text = fs::read_to_string(TDML).expect("read");
    let suite = parse_tdml(&text).expect("parse");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == "DelimProp_06")
        .expect("case");
    let xsd = suite.schemas.get(&test.model).expect("model").xsd.clone();
    let t0 = Instant::now();
    let schema = dfdl_vm::schema::parse_schema(&xsd).expect("parse schema");
    let spec =
        DfdlSpec::from_schema_root_with_tunables(schema, Some(&test.root), Default::default())
            .expect("compile");
    eprintln!("compile {:?}", t0.elapsed());

    let doc = &test.documents[0];
    let t1 = Instant::now();
    let err = spec
        .decoder_with_config(RuntimeConfig {
            strict_eos: true,
            ..RuntimeConfig::default()
        })
        .decode(&doc.data)
        .err();
    eprintln!("decode {:?} err={err:?}", t1.elapsed());
    assert!(t1.elapsed() < Duration::from_secs(5));
    let msg = err.expect("expected decode error").to_string();
    assert!(msg.to_lowercase().contains("wsp"));
}
