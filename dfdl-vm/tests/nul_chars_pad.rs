use dfdl_vm::tdml::{parse_tdml, run_parser_test, TdmlSchema, TestOutcome};
use std::fs;
use std::path::Path;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/facets/NulChars.tdml"
);

fn enrich_suite(suite: &mut dfdl_vm::tdml::TdmlSuite, path: &Path) {
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

#[test]
fn nul_pad_tdml_parser_tests() {
    let tdml = fs::read_to_string(TDML).expect("read tdml");
    let mut suite = parse_tdml(&tdml).expect("parse tdml");
    enrich_suite(&mut suite, Path::new(TDML));

    for name in ["nulPad1", "nulPad2"] {
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("missing test {name}"));
        let result = run_parser_test(&suite, test).expect("run test");
        assert!(
            matches!(result.outcome, TestOutcome::Pass),
            "{}: {:?}",
            name,
            result.outcome
        );
    }
}
