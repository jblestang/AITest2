//! Count failures per section05 TDML file (ignored, slow).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome, TdmlSchema, TdmlSuite};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
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

fn collect_tdml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tdml(&path, out);
        } else if path.extension().is_some_and(|e| e == "tdml") {
            out.push(path);
        }
    }
}

#[test]
#[ignore]
fn section05_failure_counts_by_file() {
    let mut files = Vec::new();
    collect_tdml(Path::new(TDML_ROOT), &mut files);
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for path in &files {
        let rel = path
            .strip_prefix(TDML_ROOT)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let tdml = fs::read_to_string(path).expect("read");
        let mut suite = parse_tdml(&tdml).expect("parse");
        enrich(&mut suite, path);
        let total = suite.tests.len();
        let mut fail = 0usize;
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                fail += 1;
                continue;
            };
            if matches!(r.outcome, TestOutcome::Fail(_)) {
                fail += 1;
            }
        }
        if fail > 0 {
            counts.insert(rel, (fail, total));
        }
    }
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.1 .0));
    for (rel, (fail, total)) in sorted.iter().take(25) {
        eprintln!("{rel}: {fail}/{total} fail");
    }
}
