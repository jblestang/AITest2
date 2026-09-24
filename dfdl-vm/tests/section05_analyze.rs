//! Categorize section05 failures for prioritization.
use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TdmlSchema, TdmlSuite, TestOutcome,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05"
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

fn bucket(msg: &str) -> &'static str {
    if msg.contains("unknown type") {
        return "unknown_type";
    }
    if msg.contains("expected decode error") {
        return "expected_decode_not_triggered";
    }
    if msg.contains("decode error mismatch") {
        return "decode_error_mismatch";
    }
    if msg.contains("compile error mismatch") {
        return "compile_error_mismatch";
    }
    if msg.starts_with("compile error:") {
        return "compile_error_unexpected";
    }
    if msg.contains("infoset") || msg.contains("child count") || msg.contains("mismatch") {
        return "infoset_or_doc_mismatch";
    }
    if msg.contains("encoded document") {
        return "unparse_mismatch";
    }
    "other"
}

#[test]
#[ignore]
fn section05_failure_buckets() {
    let root = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();

    let mut buckets: BTreeMap<&str, usize> = BTreeMap::new();
    let mut per_file: BTreeMap<String, (usize, usize)> = BTreeMap::new();

    for path in files {
        let rel = path.strip_prefix(root).unwrap().display().to_string();
        let Ok(tdml) = fs::read_to_string(&path) else {
            *buckets.entry("tdml_read").or_default() += 1;
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            *buckets.entry("tdml_parse").or_default() += 1;
            eprintln!("PARSE FAIL {rel}");
            continue;
        };
        enrich(&mut suite, &path);
        let (mut p, mut f) = (0usize, 0usize);
        for t in &suite.tests {
            match run_parser_test(&suite, t) {
                Ok(r) => match r.outcome {
                    TestOutcome::Pass => p += 1,
                    TestOutcome::Fail(msg) => {
                        f += 1;
                        *buckets.entry(bucket(&msg)).or_default() += 1;
                    }
                    TestOutcome::Skip(_) => {}
                },
                Err(e) => {
                    f += 1;
                    *buckets.entry("run_err").or_default() += 1;
                    eprintln!("{rel}::{} run err {e}", t.name);
                }
            }
        }
        for t in &suite.unparser_tests {
            match run_unparser_test(&suite, t) {
                Ok(r) => match r.outcome {
                    TestOutcome::Pass => p += 1,
                    TestOutcome::Fail(msg) => {
                        f += 1;
                        *buckets.entry(bucket(&msg)).or_default() += 1;
                    }
                    TestOutcome::Skip(_) => {}
                },
                Err(_e) => {
                    f += 1;
                    *buckets.entry("run_err").or_default() += 1;
                }
            }
        }
        per_file.insert(rel, (p, f));
    }

    eprintln!("\n=== BUCKETS ===");
    for (k, v) in &buckets {
        eprintln!("{k}: {v}");
    }
    eprintln!("\n=== PER FILE (fail>0) ===");
    for (f, (p, fail)) in &per_file {
        if *fail > 0 {
            eprintln!("{f}: pass={p} fail={fail}");
        }
    }
    let tp: usize = per_file.values().map(|x| x.0).sum();
    let tf: usize = per_file.values().map(|x| x.1).sum();
    eprintln!("\nTOTAL pass={tp} fail={tf}");
}

#[test]
#[ignore]
fn section05_list_all_failures() {
    let root = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    for path in files {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy();
        let Ok(tdml) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        enrich(&mut suite, &path);
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                continue;
            };
            if let TestOutcome::Fail(msg) = r.outcome {
                eprintln!("P {rel} :: {} :: {msg}", t.name);
            }
        }
        for t in &suite.unparser_tests {
            let Ok(r) = run_unparser_test(&suite, t) else {
                continue;
            };
            if let TestOutcome::Fail(msg) = r.outcome {
                eprintln!("U {rel} :: {} :: {msg}", t.name);
            }
        }
    }
}

#[test]
#[ignore]
fn debug_bitorder_tdml_parse() {
    let p = Path::new(TDML_ROOT).join("simple_types/BitOrder.tdml");
    let tdml = fs::read_to_string(p).unwrap();
    match parse_tdml(&tdml) {
        Ok(s) => eprintln!("BitOrder parse ok: {} tests", s.tests.len()),
        Err(e) => eprintln!("BitOrder parse error: {e:?}"),
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
fn section05_compile_unexpected_samples() {
    let root = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    let mut n = 0;
    for path in files {
        let Ok(tdml) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        enrich(&mut suite, &path);
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                continue;
            };
            if let TestOutcome::Fail(msg) = r.outcome {
                if msg.starts_with("compile error:") && t.expected_errors.is_none() {
                    eprintln!(
                        "{}::{}: {msg}",
                        path.strip_prefix(root).unwrap().display(),
                        t.name
                    );
                    n += 1;
                    if n >= 20 {
                        return;
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn section05_compile_mismatch_samples() {
    let root = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    let mut n = 0;
    for path in files {
        let Ok(tdml) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            continue;
        };
        enrich(&mut suite, &path);
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                continue;
            };
            if let TestOutcome::Fail(msg) = r.outcome {
                if msg.contains("compile error mismatch") {
                    eprintln!(
                        "{}::{}: {msg}",
                        path.strip_prefix(root).unwrap().display(),
                        t.name
                    );
                    n += 1;
                    if n >= 25 {
                        return;
                    }
                }
            }
        }
    }
}
