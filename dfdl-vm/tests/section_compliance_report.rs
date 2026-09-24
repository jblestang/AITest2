//! Per-section TDML compliance snapshot (ignored). Avoids single-process full-suite stack overflow.
//!
//! One section:
//!   DFDL_COMPLIANCE_SECTION=section07 cargo test -p dfdl-vm --test section_compliance_report compliance_section_snapshot -- --ignored --nocapture
//!
//! All top-level buckets (each in fresh process via scripts/run-compliance-by-section.sh):
//!   cargo test -p dfdl-vm --test section_compliance_report compliance_all_sections_table -- --ignored --nocapture

use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TdmlResourceContext, TdmlSchema, TdmlSuite,
    TestOutcome,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

#[derive(Default, Debug, Clone, Copy)]
struct Stats {
    pass: usize,
    fail: usize,
    skip: usize,
    parse_fail: usize,
}

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
    suite.resource_context = TdmlResourceContext::from_tdml_path(&tdml_path.to_string_lossy());
    for model in models {
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) {
            continue;
        }
        let path = dir.join(&model);
        let Ok(xsd) = dfdl_vm::schema::read_schema_text_file(&path) else {
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

fn run_tdml_file(path: &Path, stats: &mut Stats) {
    let Ok(tdml) = fs::read_to_string(path) else {
        stats.parse_fail += 1;
        return;
    };
    let Ok(mut suite) = parse_tdml(&tdml) else {
        stats.parse_fail += 1;
        return;
    };
    enrich_external_tdml_models(&mut suite, path);
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

fn scan_dir(root: &Path) -> Stats {
    let mut files = Vec::new();
    collect_tdml_files(root, &mut files);
    let mut stats = Stats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    stats
}

fn tdml_root() -> PathBuf {
    let root = PathBuf::from(TDML_ROOT);
    assert!(
        root.is_dir(),
        "Daffodil TDML missing. Run: scripts/setup-daffodil-tests.sh"
    );
    root
}

/// Scan one section (or bucket) directory; set `DFDL_COMPLIANCE_SECTION` to e.g. `section07` or `unparser`.
/// Optional `DFDL_COMPLIANCE_TDML` — single `.tdml` file relative to TDML root (for dirs that stack-overflow whole-tree).
#[test]
#[ignore = "slow; run one section at a time"]
fn compliance_section_snapshot() {
    let root = tdml_root();
    if let Ok(rel) = std::env::var("DFDL_COMPLIANCE_TDML") {
        let path = root.join(&rel);
        assert!(path.is_file(), "missing TDML file: {}", path.display());
        let mut stats = Stats::default();
        run_tdml_file(&path, &mut stats);
        eprintln!(
            "COMPLIANCE\t{rel}\tpass={}\tfail={}\tskip={}\tparse_fail={}",
            stats.pass, stats.fail, stats.skip, stats.parse_fail
        );
        return;
    }
    let section = std::env::var("DFDL_COMPLIANCE_SECTION").unwrap_or_else(|_| {
        eprintln!("Set DFDL_COMPLIANCE_SECTION (e.g. section13) or DFDL_COMPLIANCE_TDML");
        std::process::exit(2);
    });
    let dir = root.join(&section);
    assert!(dir.is_dir(), "missing TDML dir: {}", dir.display());
    let stats = scan_dir(&dir);
    eprintln!(
        "COMPLIANCE\t{section}\tpass={}\tfail={}\tskip={}\tparse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
}

/// Spawns one `compliance_section_snapshot` per top-level bucket (fresh stack each).
#[test]
#[ignore = "very slow; full compliance table"]
fn compliance_all_sections_table() {
    let root = tdml_root();
    let mut buckets: Vec<String> = fs::read_dir(&root)
        .expect("read tdml root")
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    buckets.sort();

    eprintln!("\n=== Daffodil TDML compliance by section (subprocess scan) ===");
    eprintln!(
        "{:<18} {:>8} {:>8} {:>8} {:>8}",
        "Section", "Pass", "Fail", "Skip", "ParseErr"
    );

    let exe = std::env::current_exe().expect("current exe");
    let mut total = Stats::default();

    for section in &buckets {
        let out = std::process::Command::new(&exe)
            .env("DFDL_COMPLIANCE_SECTION", section)
            .args([
                "compliance_section_snapshot",
                "--ignored",
                "--exact",
                "--nocapture",
            ])
            .output()
            .unwrap_or_else(|e| panic!("spawn failed for {section}: {e}"));

        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let line = combined
            .lines()
            .find(|l| l.starts_with("COMPLIANCE\t"))
            .unwrap_or_else(|| {
                panic!(
                    "no COMPLIANCE line for {section} (status={:?}):\n{combined}",
                    out.status
                )
            });
        let parts: Vec<&str> = line.split('\t').collect();
        let pass = parts[2]
            .strip_prefix("pass=")
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        let fail = parts[3]
            .strip_prefix("fail=")
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        let skip = parts[4]
            .strip_prefix("skip=")
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        let parse_fail = parts[5]
            .strip_prefix("parse_fail=")
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);

        eprintln!(
            "{:<18} {:>8} {:>8} {:>8} {:>8}",
            section, pass, fail, skip, parse_fail
        );
        total.pass += pass;
        total.fail += fail;
        total.skip += skip;
        total.parse_fail += parse_fail;
    }

    eprintln!("{:-<58}", "");
    eprintln!(
        "{:<18} {:>8} {:>8} {:>8} {:>8}",
        "TOTAL", total.pass, total.fail, total.skip, total.parse_fail
    );
}
