use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome, TdmlResourceContext, TdmlSchema};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06"
);

fn enrich(suite: &mut dfdl_vm::tdml::TdmlSuite, path: &Path) {
    let dir = path.parent().unwrap();
    let dir_str = dir.to_string_lossy().into_owned();
    suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
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
        let p = dir.join(&model);
        if let Ok(xsd) = fs::read_to_string(&p) {
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
}

fn collect_tdml(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_tdml(&p, out);
        } else if p.extension().is_some_and(|e| e == "tdml") {
            out.push(p);
        }
    }
}

#[test]
#[ignore]
fn list_section06_failures() {
    let mut files = Vec::new();
    collect_tdml(Path::new(TDML_ROOT), &mut files);
    for path in files {
        let tdml = fs::read_to_string(&path).unwrap();
        let mut suite = parse_tdml(&tdml).unwrap();
        enrich(&mut suite, &path);
        for test in &suite.tests {
            let r = run_parser_test(&suite, test).unwrap();
            if let TestOutcome::Fail(m) = r.outcome {
                eprintln!("FAIL {} :: {}", test.name, m);
            }
        }
    }
}
