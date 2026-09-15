use dfdl_vm::schema::{parse_schema_with_options, ParseOptions, Representation};
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome, TdmlResourceContext, TdmlSchema};

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06/namespaces"
);

#[test]
#[ignore]
fn inspect_nest_type3_and_run() {
    let path = format!("{ROOT}/multi_base_03.dfdl.xsd");
    let xsd = std::fs::read_to_string(&path).unwrap();
    let doc = parse_schema_with_options(
        &xsd,
        &ParseOptions {
            base_dir: Some(ROOT.into()),
            schema_label: Some("multi_base_03.dfdl.xsd".into()),
        },
    )
    .unwrap();
    eprintln!("format_defaults.length={:?}", doc.format_defaults.props.length);
    for key in doc.types.keys() {
        if key.as_str().contains("nest")
            || key.as_str().contains("subNest")
        {
            let eff = doc.effective_simple_type_props(key).unwrap();
            eprintln!(
                "{:?}: repr={:?} len={:?} len_kind={:?} units={:?}",
                key.as_str(),
                eff.representation,
                eff.length,
                eff.length_kind,
                eff.length_units
            );
        }
    }
    let tdml_path = std::path::Path::new(ROOT).join("namespaces.tdml");
    let mut suite = parse_tdml(&std::fs::read_to_string(&tdml_path).unwrap()).unwrap();
    suite.resource_context = TdmlResourceContext::from_tdml_path(&tdml_path.to_string_lossy());
    suite.schemas.insert(
        "multi_base_03.dfdl.xsd".into(),
        TdmlSchema {
            name: "multi_base_03.dfdl.xsd".into(),
            xsd,
            compile_base_dir: Some(ROOT.into()),
        },
    );
    let t = suite
        .tests
        .iter()
        .find(|t| t.name == "long_chain_04")
        .unwrap();
    let r = run_parser_test(&suite, t).unwrap();
    eprintln!("outcome: {:?}", r.outcome);
    if let TestOutcome::Pass = r.outcome {
        eprintln!("UNEXPECTED PASS - parsed when should error");
    }
    assert_eq!(eff_check_nest3(), Representation::Binary);
}

fn eff_check_nest3() -> Representation {
    let path = format!("{ROOT}/multi_base_03.dfdl.xsd");
    let xsd = std::fs::read_to_string(&path).unwrap();
    let doc = parse_schema_with_options(
        &xsd,
        &ParseOptions {
            base_dir: Some(ROOT.into()),
            schema_label: None,
        },
    )
    .unwrap();
    let name = doc
        .types
        .keys()
        .find(|k| k.as_str().contains("nestType3"))
        .cloned()
        .unwrap();
    doc.effective_simple_type_props(&name)
        .unwrap()
        .representation
        .unwrap()
}
