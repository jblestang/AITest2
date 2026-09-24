use dfdl_vm::ir::{compile_named, IrNode};
use dfdl_vm::parse_schema;
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};

const TDML: &str = include_str!(
    "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section13/zoned/pv.tdml"
);

#[test]
fn money_has_zero_rep() {
    let suite = parse_tdml(TDML).expect("tdml");
    let def = suite.schemas.get("s1").unwrap();
    let schema = parse_schema(&def.xsd).unwrap();
    let program = compile_named(&schema, Some("money")).unwrap();
    let props = program.nodes.iter().find_map(|n| match n {
        IrNode::Element { name, props, .. } if program.strings.get(*name).ok() == Some("money") => {
            Some((
                props.text_standard_zero_rep_defined,
                program
                    .strings
                    .get(props.text_standard_zero_rep)
                    .ok()
                    .map(|s| s.to_string()),
            ))
        }
        _ => None,
    });
    eprintln!("zero_rep={props:?}");
    let (defined, raw) = props.unwrap();
    assert!(defined);
    let (pattern, custom, base) = program
        .nodes
        .iter()
        .find_map(|n| match n {
            IrNode::Element { name, props, .. }
                if program.strings.get(*name).ok() == Some("money") =>
            {
                Some((
                    props
                        .text_number_pattern
                        .and_then(|id| program.strings.get(id).ok())
                        .map(|s| s.to_string()),
                    props.custom_text_number_pattern,
                    props.text_standard_base,
                ))
            }
            _ => None,
        })
        .unwrap();
    eprintln!("pattern={pattern:?} custom={custom} base={base} raw_zero={raw:?}");
}

#[test]
fn zero_rep_pattern_matches_document() {
    let pat = "Z%WSP*;Z%WSP*;Z";
    let doc = b"Z Z Z";
    let n = dfdl_vm::schema::match_delimiter_opts(doc, pat, false);
    eprintln!("match={n:?} len={}", doc.len());
    assert_eq!(n, Some(doc.len()));
}

#[test]
fn vpattern_zero_passes() {
    let suite = parse_tdml(TDML).expect("tdml");
    let t = suite
        .tests
        .iter()
        .find(|t| t.name == "vpattern_zero")
        .unwrap();
    let r = run_parser_test(&suite, t).expect("run");
    assert!(matches!(r.outcome, TestOutcome::Pass));
}

#[test]
fn vpattern_zzz_passes() {
    let suite = parse_tdml(TDML).expect("tdml");
    let t = suite
        .tests
        .iter()
        .find(|t| t.name == "vpattern_ZZZ")
        .unwrap();
    let r = run_parser_test(&suite, t).expect("run");
    match &r.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("{msg}"),
        TestOutcome::Skip(msg) => panic!("skipped: {msg}"),
    }
}
