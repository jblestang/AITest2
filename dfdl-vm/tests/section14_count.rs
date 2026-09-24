//! Full section14 pass/fail counter (ignored).
use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TdmlSchema, TdmlSuite, TestOutcome,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn enrich_external_tdml_models(suite: &mut TdmlSuite, tdml_path: &Path) {
    let Some(dir) = tdml_path.parent() else {
        return;
    };
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
        let path = dir.join(&model);
        let Ok(xsd) = fs::read_to_string(&path) else {
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

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section14"
);

fn collect_tdml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_tdml(&p, out);
        } else if p.extension().is_some_and(|x| x == "tdml") {
            out.push(p);
        }
    }
}

#[test]
#[ignore]
fn count_section14() {
    let mut files = Vec::new();
    collect_tdml(Path::new(TDML_ROOT), &mut files);
    files.sort();
    let mut pass = 0usize;
    let mut fail = 0usize;
    for path in files {
        let Ok(tdml) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        enrich_external_tdml_models(&mut suite, &path);
        for t in &suite.tests {
            match run_parser_test(&suite, t) {
                Ok(r) => match r.outcome {
                    TestOutcome::Pass => pass += 1,
                    TestOutcome::Fail(_) => fail += 1,
                    TestOutcome::Skip(_) => {}
                },
                Err(_) => fail += 1,
            }
        }
        for t in &suite.unparser_tests {
            match run_unparser_test(&suite, t) {
                Ok(r) => match r.outcome {
                    TestOutcome::Pass => pass += 1,
                    TestOutcome::Fail(_) => fail += 1,
                    TestOutcome::Skip(_) => {}
                },
                Err(_) => fail += 1,
            }
        }
    }
    eprintln!("section14 total pass={pass} fail={fail}");
}
