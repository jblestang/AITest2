//! Scan a single Daffodil section subdirectory and print pass/fail summary.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome, TdmlSchema, TdmlSuite};
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
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn collect_tdml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_tdml(&p, out);
        } else if p.extension().is_some_and(|x| x == "tdml") {
            out.push(p);
        }
    }
}

fn scan_dir(rel: &str) -> (usize, usize, usize, usize, Vec<String>) {
    scan_dir_skipping(rel, &[])
}

fn scan_dir_skipping(rel: &str, skip_rel_paths: &[&str]) -> (usize, usize, usize, usize, Vec<String>) {
    let dir = Path::new(TDML_ROOT).join(rel);
    let mut files = Vec::new();
    collect_tdml(&dir, &mut files);
    files.sort();
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut skip = 0usize;
    let mut parse_fail = 0usize;
    let mut samples = Vec::new();
    for path in files {
        let file_rel = path
            .strip_prefix(&dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if skip_rel_paths.iter().any(|s| *s == file_rel) {
            eprintln!("  skip {file_rel} (excluded from scan)");
            continue;
        }
        let Ok(tdml) = fs::read_to_string(&path) else {
            parse_fail += 1;
            continue;
        };
        let Ok(mut suite) = parse_tdml(&tdml) else {
            parse_fail += 1;
            samples.push(format!("{}: parse error", path.display()));
            continue;
        };
        enrich_external_tdml_models(&mut suite, &path);
        let relp = path.strip_prefix(TDML_ROOT).unwrap_or(&path).display().to_string();
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else {
                fail += 1;
                continue;
            };
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    if samples.len() < 40 {
                        samples.push(format!("{relp}::{}: {msg}", t.name));
                    }
                }
                TestOutcome::Skip(msg) => {
                    skip += 1;
                    if samples.len() < 40 {
                        samples.push(format!("{relp}::{} SKIP: {msg}", t.name));
                    }
                }
            }
        }
        for t in &suite.unparser_tests {
            let Ok(r) = run_unparser_test(&suite, t) else {
                fail += 1;
                continue;
            };
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    if samples.len() < 40 {
                        samples.push(format!("{relp}::unparse:{}: {msg}", t.name));
                    }
                }
                TestOutcome::Skip(msg) => {
                    skip += 1;
                    if samples.len() < 40 {
                        samples.push(format!("{relp}::unparse:{} SKIP: {msg}", t.name));
                    }
                }
            }
        }
    }
    (pass, fail, skip, parse_fail, samples)
}

macro_rules! scan_test {
    ($name:ident, $dir:literal) => {
        #[test]
        #[ignore]
        fn $name() {
            let (pass, fail, skip, parse_fail, samples) = scan_dir($dir);
            eprintln!(
                "\n=== {} === pass={pass} fail={fail} skip={skip} parse_fail={parse_fail}",
                $dir
            );
            for s in &samples {
                eprintln!("  {s}");
            }
        }
    };
}

scan_test!(scan_section12_length_kind, "section12/lengthKind");
scan_test!(scan_section12_delimiter_properties, "section12/delimiter_properties");
scan_test!(scan_section12_length_properties, "section12/length_properties");
scan_test!(scan_section12_aligned_data, "section12/aligned_data");
macro_rules! scan_test_skip {
    ($name:ident, $dir:literal, $skip:expr) => {
        #[test]
        #[ignore]
        fn $name() {
            let (pass, fail, skip, parse_fail, samples) = scan_dir_skipping($dir, $skip);
            eprintln!(
                "\n=== {} === pass={pass} fail={fail} skip={skip} parse_fail={parse_fail}",
                $dir
            );
            for s in &samples {
                eprintln!("  {s}");
            }
        }
    };
}

scan_test!(scan_section00, "section00");
scan_test!(scan_section06, "section06");

#[test]
fn daffodil_section06_regression_gate() {
    let (pass, fail, skip, parse_fail, _) = scan_dir("section06");
    assert_eq!(parse_fail, 0, "section06 TDML parse errors");
    assert!(
        pass >= 100,
        "section06: expected at least 100 passing cases, got pass={pass} fail={fail} skip={skip}"
    );
    assert!(
        fail <= 79,
        "section06 regression: too many failures pass={pass} fail={fail} skip={skip}"
    );
}
scan_test!(scan_section08, "section08/property_scoping");
scan_test!(scan_section05, "section05");
scan_test!(scan_section13, "section13");
scan_test!(scan_section14, "section14");
scan_test!(scan_section15, "section15");

#[test]
fn daffodil_section14_regression_gate() {
    let (pass, fail, skip, parse_fail, _) = scan_dir("section14");
    assert_eq!(parse_fail, 0, "section14 TDML parse errors");
    assert!(
        pass >= 153,
        "section14: expected at least 153 passing cases, got pass={pass} fail={fail} skip={skip}"
    );
    assert_eq!(
        fail, 0,
        "section14 regression: expected 0 failures pass={pass} fail={fail} skip={skip}"
    );
}

#[test]
fn daffodil_section15_regression_gate() {
    let (pass, fail, skip, parse_fail, _) = scan_dir("section15");
    assert_eq!(parse_fail, 0, "section15 TDML parse errors");
    assert!(
        pass >= 160,
        "section15: expected at least 160 passing cases, got pass={pass} fail={fail} skip={skip}"
    );
    assert!(
        fail <= 8,
        "section15 regression: too many failures pass={pass} fail={fail} skip={skip}"
    );
}
