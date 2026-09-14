//! Sample section05 failure messages (ignored diagnostic).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome, TdmlSchema, TdmlSuite};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/SimpleTypes.tdml"
);

fn enrich(suite: &mut TdmlSuite, dir: &Path) {
    let dir_str = dir.to_string_lossy().into_owned();
    let mut models = HashSet::new();
    for t in &suite.tests {
        models.insert(t.model.clone());
    }
    for model in models {
        if suite.schemas.contains_key(&model) {
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
}

#[test]
#[ignore]
fn section05_simpletypes_sample_failures() {
    let path = Path::new(TDML);
    let tdml = fs::read_to_string(path).unwrap();
    let mut suite = parse_tdml(&tdml).unwrap();
    enrich(&mut suite, path.parent().unwrap());
    let mut n = 0;
    for t in &suite.tests {
        let Ok(r) = run_parser_test(&suite, t) else { continue };
        if let TestOutcome::Fail(msg) = r.outcome {
            eprintln!("{}: {msg}", t.name);
            n += 1;
            if n >= 40 {
                break;
            }
        }
    }
}
