use dfdl_vm::ir::{IrNode, compile_named};
use dfdl_vm::schema::{parse_schema_with_options, ParseOptions};
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use dfdl_vm::DfdlSpec;

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
);

#[test]
fn tdml_e3_one_has_leading_skip_in_ir() {
    let suite = parse_tdml(TDML).expect("tdml");
    let def = suite.schemas.get("alignmentSchema").expect("schema");
    let schema = parse_schema_with_options(
        &def.xsd,
        &ParseOptions {
            base_dir: def.compile_base_dir.clone(),
        },
    )
    .expect("parse");
    let program = compile_named(&schema, Some("e3")).expect("ir");
    let skip = program
        .nodes
        .iter()
        .find_map(|n| match n {
            IrNode::Element { name, props, .. }
                if program.strings.get(*name).ok() == Some("one") =>
            {
                Some((props.leading_skip, props.length, props.length_units))
            }
            _ => None,
        })
        .expect("one");
    assert_eq!(skip.0, 4, "leading_skip");
}

#[test]
fn tdml_hb_decode_via_spec() {
    let suite = parse_tdml(TDML).expect("tdml");
    let def = suite.schemas.get("implicitAlignmentSchema").expect("schema");
    let schema = parse_schema_with_options(
        &def.xsd,
        &ParseOptions {
            base_dir: def.compile_base_dir.clone(),
        },
    )
    .expect("parse");
    let spec = DfdlSpec::from_schema_root_with_tunables(
        schema,
        Some("hB"),
        dfdl_vm::length_validate::DaffodilTunables::default(),
    )
    .expect("spec");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == "impAlignmentHexBinary")
        .expect("case");
    let doc = &test.documents[0];
    let frame = doc.significant_bit_length();
    let value = spec
        .decode_with_bit_limit(&doc.data, frame)
        .expect("decode");
    let dfdl_vm::value::DfdlValue::Sequence(fields) = value else {
        panic!("expected root wrap");
    };
    let h = fields
        .fields
        .get("hB")
        .expect("hB field");
    let dfdl_vm::value::DfdlValue::HexBinary(bytes) = h else {
        panic!("expected hex");
    };
    assert_eq!(bytes.as_slice(), &[0xF4], "hex bytes {bytes:02x?}");
}

#[test]
fn tdml_alignment02_parser_test() {
    let suite = parse_tdml(TDML).expect("tdml");
    let test = suite.tests.iter().find(|t| t.name == "alignment02").expect("t");
    let r = run_parser_test(&suite, test).expect("run");
    match r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{msg}"),
        TestOutcome::Skip(msg) => panic!("skip: {msg}"),
    }
}
