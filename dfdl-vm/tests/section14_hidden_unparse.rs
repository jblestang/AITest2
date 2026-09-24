//! Section 14 hidden group unparse cases.
use dfdl_vm::tdml::{
    parse_tdml, run_unparser_test, TdmlResourceContext, TdmlSchema, TdmlSuite, TestOutcome,
};
use std::collections::HashSet;
use std::path::Path;

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let dir_str = dir.to_string_lossy().into_owned();
    let mut models = HashSet::new();
    for t in &suite.tests {
        models.insert(t.model.clone());
    }
    for t in &suite.unparser_tests {
        models.insert(t.model.clone());
    }
    for model in models {
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) {
            continue;
        }
        let p = dir.join(&model);
        let Ok(xsd) = dfdl_vm::schema::read_schema_text_file(&p) else {
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
fn hidden_nested_group_unparse() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section14/sequence_groups/HiddenSequences.tdml"
    ));
    let tdml = std::fs::read_to_string(path).unwrap();
    let mut suite = parse_tdml(&tdml).unwrap();
    enrich(&mut suite, path);
    for name in [
        "unparseNestedHiddenAndRegularRef",
        "unparseNestedRegularAndHiddenRef",
    ] {
        let t = suite
            .unparser_tests
            .iter()
            .find(|t| t.name == name)
            .unwrap();
        let r = run_unparser_test(&suite, t).unwrap();
        match r.outcome {
            TestOutcome::Pass => {}
            TestOutcome::Fail(msg) => panic!("{name}: {msg}"),
            TestOutcome::Skip(msg) => panic!("{name} skipped: {msg}"),
        }
    }
}
