use dfdl_vm::tdml::{parse_tdml, run_parser_test, TdmlSchema, TdmlSuite, TestOutcome};
use std::collections::HashSet;
use std::path::Path;

fn enrich(suite: &mut TdmlSuite, tdml_path: &Path) {
    let Some(dir) = tdml_path.parent() else {
        return;
    };
    let dir_str = dir.to_string_lossy().into_owned();
    let mut models = HashSet::new();
    for t in &suite.tests {
        models.insert(t.model.clone());
    }
    for model in models {
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) {
            continue;
        }
        let path = dir.join(&model);
        let Ok(xsd) = std::fs::read_to_string(&path) else {
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

#[test]
fn be000_tdml_decode() {
    let path_buf = format!("{}/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section14/unordered_sequences/BE.tdml", env!("CARGO_MANIFEST_DIR"));
    let path = Path::new(&path_buf);
    let tdml = std::fs::read_to_string(path).unwrap();
    let mut suite = parse_tdml(&tdml).unwrap();
    enrich(&mut suite, path);
    let t = suite.tests.iter().find(|x| x.name == "BE000").unwrap();
    let r = run_parser_test(&suite, t).unwrap();
    match r.outcome {
        TestOutcome::Pass => {}
        other => panic!("{other:?}"),
    }
}
