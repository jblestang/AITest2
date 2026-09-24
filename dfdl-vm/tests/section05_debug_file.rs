//! Debug failures for a single section05 TDML file.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TdmlSchema, TdmlSuite, TestOutcome};
use std::env;
use std::fs;
use std::path::Path;

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else { return };
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
#[ignore]
fn section05_debug_file_failures() {
    let rel = env::var("SECTION05_FILE").unwrap_or_else(|_| "simple_types/Boolean.tdml".into());
    let path = Path::new(TDML_ROOT).join(&rel);
    let tdml = fs::read_to_string(&path).expect("read tdml");
    let mut suite = parse_tdml(&tdml).expect("parse tdml");
    enrich(&mut suite, &path);
    let mut n = 0;
    for t in &suite.tests {
        let Ok(r) = run_parser_test(&suite, t) else {
            continue;
        };
        if let TestOutcome::Fail(msg) = r.outcome {
            eprintln!("{}: {msg}", t.name);
            n += 1;
            if n >= 30 {
                break;
            }
        }
    }
    eprintln!("(showed {n} failures)");
}
