use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TdmlSchema, TdmlSuite};
use std::fs;
use std::path::Path;

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else { return };
    let dir_str = dir.to_string_lossy().into_owned();
    for t in &suite.unparser_tests {
        let model = t.model.clone();
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) {
            continue;
        }
        let p = dir.join(&model);
        let Ok(xsd) = fs::read_to_string(&p) else { continue };
        suite.schemas.insert(
            model.clone(),
            TdmlSchema {
                name: model,
                xsd,
                compile_base_dir: Some(dir_str.clone()),
            },
        );
    }
    for t in &suite.tests {
        let model = t.model.clone();
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) && !suite.schemas.contains_key(&model) {
            if let Some(def) = suite.schemas.get(&model) {
                suite.schemas.insert(model.clone(), def.clone());
            }
        }
    }
}

#[test]
#[ignore]
fn initiated_content_parse_then_unparse() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section15/choice_groups/ChoiceGroupInitiatedContent.tdml"
    );
    let tdml = fs::read_to_string(path).unwrap();
    let mut suite = parse_tdml(&tdml).unwrap();
    enrich(&mut suite, Path::new(path));

    let parse = suite.tests.iter().find(|t| t.name == "initiatedContentChoice1").unwrap();
    let pr = run_parser_test(&suite, parse).unwrap();
    eprintln!("parse: {:?}", pr.outcome);

    let unparse = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == "unparse_initiatedContentChoice1")
        .unwrap();
    let ur = run_unparser_test(&suite, unparse).unwrap();
    eprintln!("unparse from xml infoset: {:?}", ur.outcome);
}
