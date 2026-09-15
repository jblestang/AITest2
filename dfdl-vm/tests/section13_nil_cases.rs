use dfdl_vm::ir::{compile_named, IrNode};
use dfdl_vm::parse_schema;
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section13/nillable/literal-value-nils.tdml"
);

fn assert_pass(name: &str) {
    let suite = parse_tdml(TDML).expect("tdml");
    let t = suite.tests.iter().find(|t| t.name == name).unwrap();
    let r = run_parser_test(&suite, t).expect("run");
    match &r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{name}: {msg}"),
        TestOutcome::Skip(msg) => panic!("{name} skipped: {msg}"),
    }
}

#[test]
fn nillable_complex_ct_length_kind() {
    let suite = parse_tdml(TDML).expect("tdml");
    let def = suite.schemas.get("nillableComplex").unwrap();
    let schema = parse_schema(&def.xsd).unwrap();
    let program = compile_named(&schema, Some("doc")).unwrap();
    let props = program.nodes.iter().find_map(|n| match n {
        IrNode::Element { name, props, .. } if program.strings.get(*name).ok() == Some("ct") => {
            Some(props.length_kind)
        }
        _ => None,
    });
    eprintln!("ct length_kind={props:?}");
    assert_eq!(props, Some(dfdl_vm::schema::LengthKind::Implicit));
    for n in &program.nodes {
        if let IrNode::Sequence { props, .. } = n {
            let sep = props
                .separator
                .and_then(|id| program.strings.get(id).ok());
            let term = props
                .terminator
                .and_then(|id| program.strings.get(id).ok());
            eprintln!("seq sep={sep:?} term={term:?}");
        }
    }
}

#[test]
fn text_04_passes() {
    assert_pass("text_04");
}

#[test]
fn text_03_passes() {
    assert_pass("text_03");
}

#[test]
fn test_complex_nil_passes() {
    assert_pass("test_complex_nil");
}

#[test]
fn text_05_passes() {
    assert_pass("text_05");
}

#[test]
fn text_03ic_passes() {
    assert_pass("text_03ic");
}
