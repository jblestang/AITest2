//! List section06 failures (ignored).
use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TestOutcome, TdmlResourceContext, TdmlSchema,
    TdmlSuite,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else { return; };
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
        let Ok(xsd) = fs::read_to_string(&p) else { continue; };
        suite.schemas.insert(
            model.clone(),
            TdmlSchema { name: model, xsd, compile_base_dir: Some(dir_str.clone()) },
        );
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.extension().is_some_and(|x| x == "tdml") {
                out.push(p);
            }
        }
    }
}

fn categorize(msg: &str) -> &'static str {
    if msg.contains("compile error") {
        return "compile";
    }
    if msg.contains("text mismatch") {
        return "text mismatch";
    }
    if msg.contains("child count") || msg.contains("unexpected child") {
        return "infoset";
    }
    if msg.contains("decode error") {
        return "decode";
    }
    if msg.contains("encode") {
        return "encode";
    }
    "other"
}

#[test]
#[ignore]
fn list_section06_failures() {
    let mut files = Vec::new();
    collect(Path::new(TDML_ROOT), &mut files);
    files.sort();
    let mut cats: BTreeMap<&str, usize> = BTreeMap::new();
    for path in files {
        let rel = path.strip_prefix(TDML_ROOT).unwrap().to_string_lossy();
        let tdml = fs::read_to_string(&path).expect("read");
        let mut suite = parse_tdml(&tdml).expect("parse");
        suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
        enrich(&mut suite, &path);
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                *cats.entry("compile/run err").or_default() += 1;
                eprintln!("{rel}::{}: compile/run error", t.name);
                continue;
            };
            if let TestOutcome::Fail(msg) = r.outcome {
                *cats.entry(categorize(&msg)).or_default() += 1;
                eprintln!("{rel}::{}: {msg}", t.name);
            }
        }
    }
    eprintln!("\n=== categories ===");
    for (k, v) in &cats {
        eprintln!("  {v:4} {k}");
    }
}
