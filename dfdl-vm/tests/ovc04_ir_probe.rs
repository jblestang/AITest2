use dfdl_vm::ir::compile_named;
use dfdl_vm::ir::IrNode;
use dfdl_vm::schema::parse_schema_with_options;
use dfdl_vm::tdml::parse_tdml;
use std::fs;
use std::path::Path;

#[test]
fn ovc_04_x_has_output_new_line_sibling_in_ir() {
    let tdml_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section17/calc_value_properties/outputValueCalc.tdml",
    );
    let tdml = fs::read_to_string(&tdml_path).expect("tdml");
    let suite = parse_tdml(&tdml).expect("parse tdml");
    let def = suite
        .schemas
        .get("outputValueCalc-Embedded.dfdl.xsd")
        .expect("embedded schema");
    let schema = parse_schema_with_options(
        &def.xsd,
        &dfdl_vm::schema::ParseOptions {
            base_dir: def.compile_base_dir.clone(),
            schema_label: Some("outputValueCalc-Embedded.dfdl.xsd".into()),
        },
    )
    .expect("parse xsd");
    let program = compile_named(&schema, Some("ovc_04")).expect("compile");
    let IrNode::Element {
        child: Some(seq_id),
        ..
    } = program.node(program.root).expect("root")
    else {
        panic!("root not element");
    };
    let IrNode::Sequence { children, .. } = program.node(*seq_id).expect("seq") else {
        panic!("not seq");
    };
    for cid in children {
        let IrNode::Element { name, props, .. } = program.node(*cid).expect("child") else {
            continue;
        };
        let n = program.strings.get(*name).expect("name");
        let local = dfdl_vm::xml_util::local_name_str(n);
        if local == "x" || local == "y" {
            let term = props
                .terminator
                .and_then(|id| program.strings.get(id).ok())
                .unwrap_or("");
            eprintln!(
                "{local}: term={term:?} sib={:?}",
                props
                    .output_new_line_sibling
                    .map(|id| program.strings.get(id).ok())
            );
        }
        if local == "x" {
            assert!(
                props.output_new_line_sibling.is_some(),
                "x missing output_new_line_sibling"
            );
            let sib = program
                .strings
                .get(props.output_new_line_sibling.unwrap())
                .expect("sib");
            assert_eq!(sib, "xonl");
            let term = props
                .terminator
                .and_then(|id| program.strings.get(id).ok())
                .unwrap_or("");
            assert!(term.contains("%NL;"), "x terminator {term:?}");
            return;
        }
    }
    panic!("x element not found");
}

#[test]
fn ovc_04_resolve_xonl_for_x_element_props() {
    use dfdl_vm::value::DfdlValue;
    use dfdl_vm::vm::resolve_output_new_line_for_encode;
    use std::collections::BTreeMap;

    let tdml_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section17/calc_value_properties/outputValueCalc.tdml",
    );
    let tdml = fs::read_to_string(&tdml_path).expect("tdml");
    let suite = parse_tdml(&tdml).expect("parse tdml");
    let def = suite
        .schemas
        .get("outputValueCalc-Embedded.dfdl.xsd")
        .expect("embedded schema");
    let schema = parse_schema_with_options(
        &def.xsd,
        &dfdl_vm::schema::ParseOptions {
            base_dir: def.compile_base_dir.clone(),
            schema_label: Some("outputValueCalc-Embedded.dfdl.xsd".into()),
        },
    )
    .expect("parse xsd");
    let program = compile_named(&schema, Some("ovc_04")).expect("compile");
    let IrNode::Element {
        child: Some(seq_id),
        ..
    } = program.node(program.root).expect("root")
    else {
        panic!("root not element");
    };
    let IrNode::Sequence { children, .. } = program.node(*seq_id).expect("seq") else {
        panic!("not seq");
    };
    let mut x_props = None;
    for cid in children {
        let IrNode::Element { name, props, .. } = program.node(*cid).expect("child") else {
            continue;
        };
        let n = program.strings.get(*name).expect("name");
        if dfdl_vm::xml_util::local_name_str(n) == "x" {
            x_props = Some(props.clone());
            break;
        }
    }
    let x_props = x_props.expect("x props");
    let mut map: BTreeMap<String, DfdlValue> = BTreeMap::new();
    map.insert(
        "{http://example.com}xonl".into(),
        DfdlValue::string("\u{0085}"),
    );
    assert!(
        map.iter()
            .any(|(k, _)| dfdl_vm::xml_util::local_name_str(k) == "xonl"),
        "map keys: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    let resolved = resolve_output_new_line_for_encode(&x_props, Some(&map), &program.strings)
        .expect("resolve");
    assert_eq!(
        resolved.as_deref(),
        Some("\u{0085}"),
        "resolved={resolved:?} sibling={:?}",
        x_props
            .output_new_line_sibling
            .map(|id| program.strings.get(id).ok())
    );
    map.clear();
    map.insert("xonl".into(), DfdlValue::string("\u{0085}"));
    let resolved2 = resolve_output_new_line_for_encode(&x_props, Some(&map), &program.strings)
        .expect("resolve2");
    assert_eq!(resolved2.as_deref(), Some("\u{0085}"));
}

#[test]
fn ovc_04_compare_xy_output_new_line_ir() {
    let tdml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section17/calc_value_properties/outputValueCalc.tdml",
    );
    let tdml = std::fs::read_to_string(&tdml_path).expect("tdml");
    let suite = dfdl_vm::tdml::parse_tdml(&tdml).expect("parse tdml");
    let def = suite
        .schemas
        .get("outputValueCalc-Embedded.dfdl.xsd")
        .expect("schema");
    let schema = dfdl_vm::schema::parse_schema_with_options(
        &def.xsd,
        &dfdl_vm::schema::ParseOptions {
            base_dir: def.compile_base_dir.clone(),
            schema_label: Some("outputValueCalc-Embedded.dfdl.xsd".into()),
        },
    )
    .expect("parse xsd");
    let program = dfdl_vm::ir::compile_named(&schema, Some("ovc_04")).expect("compile");
    let IrNode::Element {
        child: Some(seq_id),
        ..
    } = program.node(program.root).expect("root")
    else {
        panic!()
    };
    let IrNode::Sequence { children, .. } = program.node(*seq_id).expect("seq") else {
        panic!()
    };
    for cid in children {
        let IrNode::Element {
            name, props, child, ..
        } = program.node(*cid).expect("child")
        else {
            continue;
        };
        let local = dfdl_vm::xml_util::local_name_str(program.strings.get(*name).expect("n"));
        if local == "x" || local == "y" {
            eprintln!(
                "{local}: sib={:?} onl={:?} child={:?} initiator={:?}",
                props
                    .output_new_line_sibling
                    .map(|id| program.strings.get(id).ok()),
                props.output_new_line.map(|id| program.strings.get(id).ok()),
                child,
                props.initiator.map(|id| program.strings.get(id).ok()),
            );
        }
    }
}
