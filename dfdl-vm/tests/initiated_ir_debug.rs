use dfdl_vm::ir::IrNode;
use dfdl_vm::DfdlSpec;

#[test]
#[ignore]
fn print_e1_initiators() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section14/sequence_groups/SequenceGroupInitiatedContent.tdml"
    );
    let tdml = std::fs::read_to_string(path).unwrap();
    let suite = dfdl_vm::tdml::parse_tdml(&tdml).unwrap();
    let schema = suite.schemas.get("s1").unwrap();
    let spec = DfdlSpec::from_xsd_root(&schema.xsd, Some("e1")).unwrap();
    fn walk(prog: &dfdl_vm::IrProgram, id: u32, depth: usize) {
        let pad = " ".repeat(depth * 2);
        match prog.node(id).unwrap() {
            IrNode::Element {
                name, props, child, ..
            } => {
                let n = prog.strings.get(*name).unwrap();
                let init = props
                    .initiator
                    .and_then(|i| prog.strings.get(i).ok())
                    .unwrap_or("");
                let term = props
                    .terminator
                    .and_then(|i| prog.strings.get(i).ok())
                    .unwrap_or("");
                eprintln!(
                    "{pad}el {n} init={init:?} term={term:?} min={}",
                    props.occurs_min
                );
                if let Some(c) = child {
                    walk(prog, *c, depth + 1);
                }
            }
            IrNode::Sequence {
                children, props, ..
            } => {
                eprintln!("{pad}seq initiated={}", props.initiated_content);
                for &c in children {
                    walk(prog, c, depth + 1);
                }
            }
            _ => {}
        }
    }
    walk(spec.program(), spec.program().root, 0);
    let dec = spec.decode(b"[123](abc)").unwrap();
    eprintln!("decoded {:?}", dec);
}
