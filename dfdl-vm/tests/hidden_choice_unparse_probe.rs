use dfdl_vm::tdml::{parse_tdml, run_unparser_test, TestOutcome, TdmlResourceContext, TdmlSchema};
use std::fs;
use std::path::Path;

#[test]
fn unparse_hidden_group_ref() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section15/choice_groups/HiddenChoices.tdml",
    );
    let tdml = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&tdml).expect("parse");
    suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
    let dir = path.parent().unwrap().to_string_lossy().into_owned();
    suite.schemas.insert(
        "ChoicesInHiddenContexts.dfdl.xsd".into(),
        TdmlSchema {
            name: "ChoicesInHiddenContexts.dfdl.xsd".into(),
            xsd: fs::read_to_string(path.parent().unwrap().join("ChoicesInHiddenContexts.dfdl.xsd"))
                .expect("xsd"),
            compile_base_dir: Some(dir),
        },
    );
    let test = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == "unparseHiddenGroupRef")
        .expect("test");
    let result = run_unparser_test(&suite, test).expect("run");
    match result.outcome {
        TestOutcome::Pass => {}
        other => panic!("{other:?}"),
    }
}
