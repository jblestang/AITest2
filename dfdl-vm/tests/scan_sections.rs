//! Scan a single Daffodil section subdirectory and print pass/fail summary.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};
use std::fs;
use std::path::{Path, PathBuf};

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
        let Ok(tdml) = fs::read_to_string(&path) else {
            parse_fail += 1;
            continue;
        };
        let Ok(suite) = parse_tdml(&tdml) else {
            parse_fail += 1;
            samples.push(format!("{}: parse error", path.display()));
            continue;
        };
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

scan_test!(scan_section12_delimiter_properties, "section12/delimiter_properties");
scan_test!(scan_section12_length_properties, "section12/length_properties");
scan_test!(scan_section12_aligned_data, "section12/aligned_data");
