//! Full Daffodil TDML conformance harness across all vendored sections.
//!
//! - `daffodil_section12_length_kind_regression_gate` — CI gate (305 cases, must pass)
//! - `daffodil_full_suite_report` — baseline report for all sections (ignored, slow)
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

#[derive(Default, Debug)]
struct SectionStats {
    pass: usize,
    fail: usize,
    skip: usize,
    parse_fail: usize,
}

fn collect_tdml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tdml_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "tdml") {
            out.push(path);
        }
    }
}

fn section_key(path: &Path) -> String {
    path.strip_prefix(TDML_ROOT)
        .ok()
        .and_then(|p| p.components().next())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into())
}

fn run_tdml_file(path: &Path, stats: &mut SectionStats) {
    let Ok(tdml) = fs::read_to_string(path) else {
        stats.parse_fail += 1;
        return;
    };
    let Ok(suite) = parse_tdml(&tdml) else {
        stats.parse_fail += 1;
        return;
    };
    for test in &suite.tests {
        let Ok(r) = run_parser_test(&suite, test) else {
            stats.fail += 1;
            continue;
        };
        match r.outcome {
            TestOutcome::Pass => stats.pass += 1,
            TestOutcome::Fail(_) => stats.fail += 1,
            TestOutcome::Skip(_) => stats.skip += 1,
        }
    }
    for test in &suite.unparser_tests {
        let Ok(r) = run_unparser_test(&suite, test) else {
            stats.fail += 1;
            continue;
        };
        match r.outcome {
            TestOutcome::Pass => stats.pass += 1,
            TestOutcome::Fail(_) => stats.fail += 1,
            TestOutcome::Skip(_) => stats.skip += 1,
        }
    }
}

fn assert_tdml_root() -> PathBuf {
    let root = PathBuf::from(TDML_ROOT);
    assert!(
        root.is_dir(),
        "Daffodil TDML missing. Run: scripts/setup-daffodil-tests.sh"
    );
    root
}

#[test]
#[ignore = "slow baseline report across all Daffodil sections"]
fn daffodil_full_suite_report() {
    let root = assert_tdml_root();
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    files.sort();

    let mut by_section: BTreeMap<String, SectionStats> = BTreeMap::new();
    for path in &files {
        let section = section_key(path);
        run_tdml_file(path, by_section.entry(section).or_default());
    }

    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let mut total_skip = 0usize;
    let mut total_parse_fail = 0usize;

    eprintln!("\n=== Daffodil TDML conformance by section ===");
    eprintln!(
        "{:<14} {:>8} {:>8} {:>8} {:>8}",
        "Section", "Pass", "Fail", "Skip", "ParseErr"
    );
    for (section, stats) in &by_section {
        eprintln!(
            "{:<14} {:>8} {:>8} {:>8} {:>8}",
            section, stats.pass, stats.fail, stats.skip, stats.parse_fail
        );
        total_pass += stats.pass;
        total_fail += stats.fail;
        total_skip += stats.skip;
        total_parse_fail += stats.parse_fail;
    }
    eprintln!("{:-<50}", "");
    eprintln!(
        "{:<14} {:>8} {:>8} {:>8} {:>8}",
        "TOTAL", total_pass, total_fail, total_skip, total_parse_fail
    );
    eprintln!("TDML files: {}", files.len());
}

#[test]
fn daffodil_section12_length_kind_regression_gate() {
    let root = assert_tdml_root().join("section12/lengthKind");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "section12/lengthKind TDML missing");

    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    assert_eq!(
        stats.fail, 0,
        "section12 lengthKind failures: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert_eq!(stats.skip, 0);
    assert_eq!(stats.parse_fail, 0);
    assert!(stats.pass >= 300, "expected ~305 passing cases, got {}", stats.pass);
}

/// Track progress on Section 12 delimiter_properties (not yet fully passing).
#[test]
#[ignore = "work in progress toward full section coverage"]
fn daffodil_section12_delimiter_properties_progress_gate() {
    let root = assert_tdml_root().join("section12/delimiter_properties");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "delimiter_properties: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert!(
        stats.pass >= 19,
        "expected at least 19 passing delimiter_properties cases, got {}",
        stats.pass
    );
}
