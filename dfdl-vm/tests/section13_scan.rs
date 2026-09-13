//! Per-file Section 13 scan (avoids one slow/hung case blocking the whole section).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section13"
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

fn scan_file(path: &Path, per_test_limit: Duration) -> (usize, usize, usize, Vec<String>) {
    let (p, f, s, _, notes) = scan_file_detailed(path, per_test_limit);
    (p, f, s, notes)
}

fn scan_file_detailed(
    path: &Path,
    per_test_limit: Duration,
) -> (usize, usize, usize, Vec<(String, String)>, Vec<String>) {
    let Ok(tdml) = fs::read_to_string(path) else {
        return (0, 1, 0, Vec::new(), vec![format!("{}: read error", path.display())]);
    };
    let Ok(suite) = parse_tdml(&tdml) else {
        return (0, 1, 0, Vec::new(), vec![format!("{}: parse error", path.display())]);
    };
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut skip = 0usize;
    let mut passing = Vec::new();
    let mut notes = Vec::new();
    let relp = path.strip_prefix(TDML_ROOT).unwrap_or(path).display().to_string();

    for t in &suite.tests {
        let start = Instant::now();
        let outcome = match run_parser_test(&suite, t) {
            Ok(r) => r.outcome,
            Err(e) => {
                fail += 1;
                notes.push(format!("{relp}::{}: run error: {e}", t.name));
                continue;
            }
        };
        if start.elapsed() > per_test_limit {
            notes.push(format!("{relp}::{}: slow (>{}ms)", t.name, per_test_limit.as_millis()));
        }
        match outcome {
            TestOutcome::Pass => {
                pass += 1;
                passing.push((relp.clone(), t.name.clone()));
            }
            TestOutcome::Fail(msg) => {
                fail += 1;
                if notes.len() < 30 {
                    notes.push(format!("{relp}::{}: {msg}", t.name));
                }
            }
            TestOutcome::Skip(msg) => {
                skip += 1;
                if notes.len() < 30 {
                    notes.push(format!("{relp}::{} SKIP: {msg}", t.name));
                }
            }
        }
    }
    for t in &suite.unparser_tests {
        let start = Instant::now();
        let outcome = match run_unparser_test(&suite, t) {
            Ok(r) => r.outcome,
            Err(e) => {
                fail += 1;
                notes.push(format!("{relp}::unparse:{}: run error: {e}", t.name));
                continue;
            }
        };
        if start.elapsed() > per_test_limit {
            notes.push(format!(
                "{relp}::unparse:{}: slow (>{}ms)",
                t.name,
                per_test_limit.as_millis()
            ));
        }
        match outcome {
            TestOutcome::Pass => {
                pass += 1;
                passing.push((relp.clone(), format!("unparse:{}", t.name)));
            }
            TestOutcome::Fail(msg) => {
                fail += 1;
                if notes.len() < 30 {
                    notes.push(format!("{relp}::unparse:{}: {msg}", t.name));
                }
            }
            TestOutcome::Skip(msg) => {
                skip += 1;
                if notes.len() < 30 {
                    notes.push(format!("{relp}::unparse:{} SKIP: {msg}", t.name));
                }
            }
        }
    }
    (pass, fail, skip, passing, notes)
}

fn report_file(rel: &str) {
    let path = Path::new(TDML_ROOT).join(rel);
    let (p, f, s, _, notes) = scan_file_detailed(&path, Duration::from_secs(2));
    eprintln!("{rel}: pass={p} fail={f} skip={s}");
    for n in notes {
        eprintln!("  {n}");
    }
}

/// Skipped in full-section scans: known to hang the VM (packed decimal loop).
const SKIP_HANG_FILES: &[&str] = &["packed/packed.tdml"];

#[test]
#[ignore = "diagnostic: single TDML file"]
fn section13_scan_packed() {
    report_file("packed/packed.tdml");
}

#[test]
#[ignore = "diagnostic: single TDML file"]
fn section13_scan_text_number_props() {
    for rel in [
        "text_number_props/TextNumberProps.tdml",
        "text_number_props/TextNumberPropsUnparse.tdml",
        "text_number_props/TextPad.tdml",
        "text_number_props/TextStandardBase.tdml",
    ] {
        report_file(rel);
    }
}

#[test]
#[ignore = "diagnostic: single TDML file"]
fn section13_scan_zoned() {
    for rel in [
        "zoned/pv.tdml",
        "zoned/zoned.tdml",
        "zoned/zoned2.tdml",
    ] {
        report_file(rel);
    }
}

#[test]
#[ignore = "diagnostic: scan section13 per TDML file"]
fn section13_per_file_report() {
    let root = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect_tdml(root, &mut files);
    files.sort();
    let limit = Duration::from_secs(2);
    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let mut total_skip = 0usize;
    for path in &files {
        let relp = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if SKIP_HANG_FILES.iter().any(|s| *s == relp) {
            eprintln!("{relp}: SKIPPED (known hang)");
            continue;
        }
        let (p, f, s, notes) = scan_file(path, limit);
        total_pass += p;
        total_fail += f;
        total_skip += s;
        eprintln!(
            "{}: pass={p} fail={f} skip={s}",
            path.strip_prefix(root).unwrap_or(path).display()
        );
        for n in notes {
            eprintln!("  {n}");
        }
    }
    eprintln!(
        "\nsection13 TOTAL: pass={total_pass} fail={total_fail} skip={total_skip} files={}",
        files.len()
    );
}

#[test]
#[ignore = "emit Rust snippets for conformance.rs"]
fn emit_section13_passing_cases() {
    let root = Path::new(TDML_ROOT);
    let mut files = Vec::new();
    collect_tdml(root, &mut files);
    files.sort();
    let limit = Duration::from_secs(2);
    let mut all = Vec::new();
    for path in &files {
        let relp = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if SKIP_HANG_FILES.iter().any(|s| *s == relp) {
            continue;
        }
        let (p, f, s, passing, _) = scan_file_detailed(path, limit);
        eprintln!("// {relp}: pass={p} fail={f} skip={s}");
        all.extend(passing);
    }
    eprintln!("// TOTAL PASSING: {}", all.len());
    for (file, name) in all {
        eprintln!("    (\"{file}\", \"{name}\"),");
    }
}
