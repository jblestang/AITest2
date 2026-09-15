use dfdl_vm::ir::{IrNode, IrProgram};
use dfdl_vm::schema::parse_schema_with_resolver;
use dfdl_vm::schema::SchemaResolver;
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome, TdmlSchema, TdmlSuite};
use dfdl_vm::ir::compile_named;
use std::fs;
use std::path::Path;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/dfdl_xsdl_subset/DFDLSubset.tdml"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    let dir_str = dir.to_string_lossy().into_owned();
    for t in &suite.tests {
        let model = t.model.clone();
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) {
            continue;
        }
        let p = dir.join(&model);
        let Ok(xsd) = fs::read_to_string(&p) else {
            continue;
        };
        suite.schemas.insert(
            model.clone(),
            TdmlSchema {
                name: model,
                xsd,
                compile_base_dir: Some(dir_str.clone()),
            },
        );
    }
}

fn dump_ir(prog: &IrProgram, id: u32, depth: usize) {
    let pad = "  ".repeat(depth);
    match &prog.nodes[id as usize] {
        IrNode::Sequence { children, props } => {
            let sep = props
                .separator
                .and_then(|s| prog.strings.get(s).ok())
                .unwrap_or("");
            eprintln!("{pad}Sequence sep={sep:?} children={children:?}");
            for &c in children {
                dump_ir(prog, c, depth + 1);
            }
        }
        IrNode::Choice { branches, .. } => {
            eprintln!("{pad}Choice branches={}", branches.len());
            for b in branches {
                dump_ir(prog, b.node, depth + 1);
            }
        }
        IrNode::Element { name, kind, child, .. } => {
            let n = prog.strings.get(*name).unwrap_or("?");
            eprintln!("{pad}Element {n} {kind:?} child={child:?}");
            if let Some(c) = child {
                dump_ir(prog, *c, depth + 1);
            }
        }
    }
}

#[test]
fn group_ref_ir_shape() {
    let path = Path::new(TDML);
    let tdml = fs::read_to_string(path).expect("read");
    let suite = parse_tdml(&tdml).expect("parse tdml");
    let xsd = suite.schemas.get("groupRef.xsd").expect("schema").xsd.clone();
    let base = path.parent().unwrap().to_string_lossy().into_owned();
    let resolver = SchemaResolver::new().with_base_dir(base);
    let schema = parse_schema_with_resolver(&xsd, resolver).expect("parse xsd");
    let item = dfdl_vm::schema::get_global_element(&schema, "Item").expect("Item");
    eprintln!("Item type {:?}", item.type_name);
    if let Some(dfdl_vm::schema::TypeDef::Complex { content, .. }) =
        schema.resolve_type(&item.type_name)
    {
        eprintln!("Item complex content {:?}", content);
    }
    eprintln!("groups keys {:?}", schema.groups.keys().collect::<Vec<_>>());
    let prog = compile_named(&schema, Some("list")).expect("compile");
    dump_ir(&prog, prog.root, 0);
}

#[test]
fn group_ref_list_parses() {
    let path = Path::new(TDML);
    let tdml = fs::read_to_string(path).expect("read");
    let mut suite = parse_tdml(&tdml).expect("parse tdml");
    enrich(&mut suite, path);
    let t = suite.tests.iter().find(|t| t.name == "groupRef").expect("case");
    let r = run_parser_test(&suite, t).expect("run");
    eprintln!("{r:?}");
    assert!(matches!(r.outcome, TestOutcome::Pass));
}
