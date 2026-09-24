//! Per-file pass/fail counts for section00 TDML (ignored diagnostic).
use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TdmlResourceContext, TdmlSchema, TdmlSuite,
    TestOutcome,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section00"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else { return };
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

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "tdml") {
            out.push(p);
        }
    }
}

#[test]
#[ignore]
fn section00_per_file_stats() {
    let dir = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    for path in files {
        let Ok(tdml) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        enrich(&mut suite, &path);
        suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
        let mut pass = 0usize;
        let mut fail = 0usize;
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                fail += 1;
                continue;
            };
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(_) => fail += 1,
                _ => {}
            }
        }
        for t in &suite.unparser_tests {
            let Ok(r) = run_unparser_test(&suite, t) else {
                fail += 1;
                continue;
            };
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(_) => fail += 1,
                _ => {}
            }
        }
        eprintln!(
            "{}: pass={pass} fail={fail}",
            path.file_name().unwrap().to_string_lossy()
        );
    }
}
