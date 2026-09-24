use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TdmlResourceContext, TdmlSchema, TdmlSuite,
    TestOutcome,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section02"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
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
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
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
fn scan_section02() {
    let root = Path::new(ROOT);
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    let mut pass = 0usize;
    let mut fail = 0usize;
    for path in &files {
        let mut pf = 0usize;
        let mut ff = 0usize;
        let Ok(tdml) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        enrich(&mut suite, path);
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                ff += 1;
                continue;
            };
            match r.outcome {
                TestOutcome::Pass => pf += 1,
                TestOutcome::Fail(msg) => {
                    ff += 1;
                    eprintln!(
                        "FAIL {}::{}: {msg}",
                        path.file_name().unwrap().to_string_lossy(),
                        t.name
                    );
                }
                _ => {}
            }
        }
        for t in &suite.unparser_tests {
            let Ok(r) = run_unparser_test(&suite, t) else {
                ff += 1;
                continue;
            };
            match r.outcome {
                TestOutcome::Pass => pf += 1,
                TestOutcome::Fail(msg) => {
                    ff += 1;
                    eprintln!(
                        "{}::unparse:{}: {msg}",
                        path.file_name().unwrap().to_string_lossy(),
                        t.name
                    );
                }
                _ => {}
            }
        }
        eprintln!(
            "{}: pass={pf} fail={ff}",
            path.file_name().unwrap().to_string_lossy()
        );
        pass += pf;
        fail += ff;
    }
    eprintln!("\nsection02 TOTAL: pass={pass} fail={fail}");
}
