use dfdl_vm::schema::{parse_schema_with_options, ParseOptions, Representation, TypeName};

#[test]
fn rna_base_simple_type_has_binary_representation() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06/namespaces/ibm_format_compat_2.dfdl.xsd"
    );
    let xsd = std::fs::read_to_string(path).unwrap();
    let dir = std::path::Path::new(path)
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let doc = parse_schema_with_options(
        &xsd,
        &ParseOptions {
            base_dir: Some(dir),
            schema_label: None,
        },
    )
    .expect("parse");
    let keys: Vec<_> = doc.types.keys().map(|k| k.as_str().to_string()).collect();
    eprintln!("types: {keys:?}");
    let name = keys
        .iter()
        .find(|k| k.contains("RNABase"))
        .cloned()
        .map(TypeName::new)
        .expect("RNABase type");
    let props = doc
        .effective_simple_type_props(&name)
        .expect("effective props");
    eprintln!(
        "RNABase effective: repr={:?} len_kind={:?} len={:?} align={:?} units={:?}",
        props.representation, props.length_kind, props.length, props.alignment, props.length_units
    );
    assert_eq!(props.representation, Some(Representation::Binary));
}
