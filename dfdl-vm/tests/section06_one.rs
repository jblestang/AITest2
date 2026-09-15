use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome, TdmlResourceContext, TdmlSchema};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06");

fn enrich(suite: &mut dfdl_vm::tdml::TdmlSuite, path: &Path) {
    let dir = path.parent().unwrap();
    let dir_str = dir.to_string_lossy().into_owned();
    suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let mut models = HashSet::new();
    for t in &suite.tests { models.insert(t.model.clone()); }
    for model in models {
        if suite.schemas.contains_key(&model) { continue; }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) { continue; }
        let p = dir.join(&model);
        if let Ok(xsd) = fs::read_to_string(&p) {
            suite.schemas.insert(model.clone(), TdmlSchema { name: model, xsd, compile_base_dir: Some(dir_str.clone()) });
        }
    }
}

#[test]
#[ignore]
fn debug_complex_includes() {
    let path = Path::new(ROOT).join("namespaces/multiFile.tdml");
    let mut suite = parse_tdml(&fs::read_to_string(&path).unwrap()).unwrap();
    enrich(&mut suite, &path);
    for name in ["complexIncludesNamespaces_01", "complexIncludesNamespaces_02"] {
        let t = suite.tests.iter().find(|t| t.name == name).expect(name);
        let r = run_parser_test(&suite, t).unwrap();
        eprintln!("{name}: {:?}", r.outcome);
    }
}

#[test]
#[ignore]
fn debug_priority_section06() {
    let cases = [
        ("namespaces/namespaces.tdml", &["multifile_choice_01", "multifile_choice_02b", "multifile_choice_03", "long_chain_02"][..]),
        ("entities/Entities.tdml", &["dataDumpEncoding"][..]),
    ];
    for (rel, names) in cases {
        let path = Path::new(ROOT).join(rel);
        let mut suite = parse_tdml(&fs::read_to_string(&path).unwrap()).unwrap();
        enrich(&mut suite, &path);
        for name in names {
            let t = suite.tests.iter().find(|t| t.name == *name).expect(name);
            let r = run_parser_test(&suite, t).unwrap();
            eprintln!("{name}: {:?}", r.outcome);
            if let TestOutcome::Fail(m) = r.outcome {
                eprintln!("  {m}");
            }
        }
    }
}

#[test]
#[ignore]
fn debug_combinations_and_long_chain() {
    let path = Path::new(ROOT).join("namespaces/namespaces.tdml");
    let mut suite = parse_tdml(&fs::read_to_string(&path).unwrap()).unwrap();
    enrich(&mut suite, &path);
    for name in ["combinations_02", "combinations_03", "long_chain_06", "long_chain_06b"] {
        let t = suite.tests.iter().find(|t| t.name == name).expect(name);
        let r = run_parser_test(&suite, t).unwrap();
        eprintln!("{name}: {:?}", r.outcome);
        if let TestOutcome::Fail(m) = r.outcome {
            eprintln!("  {m}");
        }
    }
}

#[test]
#[ignore]
fn debug_default_namespace_in_expression() {
    let path = Path::new(ROOT).join("namespaces/namespaces.tdml");
    let mut suite = parse_tdml(&fs::read_to_string(&path).unwrap()).unwrap();
    enrich(&mut suite, &path);
    for name in ["defaultNamespaceInExpression", "defaultNamespaceInExpression2"] {
        let t = suite.tests.iter().find(|t| t.name == name).expect(name);
        let r = run_parser_test(&suite, t).unwrap();
        eprintln!("{name}: {:?}", r.outcome);
        if let TestOutcome::Fail(m) = r.outcome {
            eprintln!("  {m}");
        }
    }
}

#[test]
#[ignore]
fn debug_char_class_entities() {
    let path = Path::new(ROOT).join("entities/charClassEntities.tdml");
    let mut suite = parse_tdml(&fs::read_to_string(&path).unwrap()).unwrap();
    enrich(&mut suite, &path);
    for name in ["CarriageReturn", "LineSeparator", "NextLine"] {
        if let Some(t) = suite.tests.iter().find(|t| t.name == name) {
            let r = run_parser_test(&suite, t).unwrap();
            eprintln!("{name}: {:?}", r.outcome);
        }
    }
}
