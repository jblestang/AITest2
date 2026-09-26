use dfdl_vm::tdml::{parse_tdml, run_parser_test, TdmlResourceContext, TdmlSchema, TestOutcome};
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
        if let Ok(xsd) = dfdl_vm::schema::read_schema_text_file(&p) {
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
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_tdml(&p, out);
        } else if p.extension().is_some_and(|e| e == "tdml") {
            out.push(p);
        }
    }
}

#[test]
fn test_section06_failures() {
    let mut files = Vec::new();
    collect_tdml(Path::new(TDML_ROOT), &mut files);
    files.sort();
    for path in files {
        let Ok(tdml) = fs::read_to_string(&path) else { continue };
        let Ok(mut suite) = parse_tdml(&tdml) else { continue };
        enrich(&mut suite, &path);
        for t in &suite.tests {
            if let Ok(r) = run_parser_test(&suite, t) {
                if matches!(r.outcome, TestOutcome::Fail(_)) {
                    println!("FAIL {}: {:?}", t.name, r.outcome);
                }
            }
        }
    }
}
