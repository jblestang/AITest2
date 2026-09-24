//! Temporary test to list section23 failures in detail.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section23"
);

fn collect_tdml(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_tdml(&p, out);
            } else if p.extension().is_some_and(|e| e == "tdml") {
                out.push(p);
            }
        }
    }
}

#[test]
#[ignore]
fn scan_section23_detail() {
    let mut files = Vec::new();
    collect_tdml(Path::new(ROOT), &mut files);
    files.sort();
    let mut pass = 0usize;
    let mut fail = 0usize;
    for path in &files {
        let Ok(tdml) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        // enrich external schema refs
        let dir = path.parent().unwrap();
        let dir_str = dir.to_string_lossy().into_owned();
        for model_name in suite
            .tests
            .iter()
            .map(|t| t.model.clone())
            .collect::<std::collections::HashSet<_>>()
        {
            if suite.schemas.contains_key(&model_name) {
                continue;
            }
            if !(model_name.ends_with(".xsd") || model_name.ends_with(".dfdl.xsd")) {
                continue;
            }
            let p = dir.join(&model_name);
            if let Ok(xsd) = dfdl_vm::schema::read_schema_text_file(&p) {
                suite.schemas.insert(
                    model_name.clone(),
                    dfdl_vm::tdml::TdmlSchema {
                        name: model_name,
                        xsd,
                        compile_base_dir: Some(dir_str.clone()),
                    },
                );
            }
        }
        for t in &suite.tests {
            match run_parser_test(&suite, t) {
                Ok(r) => match r.outcome {
                    TestOutcome::Pass => pass += 1,
                    TestOutcome::Fail(msg) => {
                        fail += 1;
                        eprintln!(
                            "FAIL {}::{}: {}",
                            path.file_name().unwrap().to_string_lossy(),
                            t.name,
                            &msg[..msg.len().min(120)]
                        );
                    }
                    TestOutcome::Skip(_) => {}
                },
                Err(_) => {
                    fail += 1;
                }
            }
        }
    }
    eprintln!("section23: pass={pass} fail={fail}");
}
