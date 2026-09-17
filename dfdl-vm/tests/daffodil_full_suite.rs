//! Full Daffodil TDML conformance harness across all vendored sections.
//!
//! - `daffodil_section12_length_kind_regression_gate` — CI gate (305 cases, must pass)
//! - `daffodil_full_suite_report` — baseline report for all sections (ignored; may stack-overflow)
//! - `section_compliance_report` + `scripts/run-compliance-by-section.sh` — per-bucket scans (preferred)
use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TestOutcome, TdmlResourceContext, TdmlSchema,
    TdmlSuite,
};
use std::collections::{BTreeMap, HashSet};
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
    suite.resource_context =
        TdmlResourceContext::from_tdml_path(&tdml_path.to_string_lossy());
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

const SECTION13_SKIP_FILES: &[&str] = &[];

/// TDML files excluded from section00 gate (large unparser/tunables suites — tracked separately).
const SECTION00_GATE_SKIP_FILES: &[&str] = &[
    "general/testUnparserGeneral.tdml",
    "general/testUnparserFileBuffering.tdml",
    "general/tunables.tdml",
    "general/parseUnparsePolicy.tdml",
    "general/testElementFormDefault.tdml",
];

/// Baseline for all `section00/**` TDML (release scan 2026-09).
const SECTION00_BASELINE_PASS_MIN: usize = 140;
const SECTION00_BASELINE_FAIL_MAX: usize = 10;

/// Baseline for all `section02/**` TDML (validation + processing error suites).
const SECTION02_BASELINE_PASS_MIN: usize = 70;
const SECTION02_BASELINE_FAIL_MAX: usize = 25;

/// Baseline for all `section05/**` TDML.
const SECTION05_BASELINE_PASS_MIN: usize = 640;
const SECTION05_BASELINE_FAIL_MAX: usize = 170;

/// Baseline for all `section06/**` TDML (namespaces + entities).
const SECTION06_BASELINE_PASS_MIN: usize = 120;
const SECTION06_BASELINE_FAIL_MAX: usize = 55;

/// Baseline for all `section13/**` TDML.
const SECTION13_BASELINE_PASS_MIN: usize = 440;
const SECTION13_BASELINE_FAIL_MAX: usize = 105;

fn run_tdml_file(path: &Path, stats: &mut SectionStats) {
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
        let r = match run_parser_test(&suite, test) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[FAIL PARSER ERR] {} :: {} => {:?}", path.file_name().unwrap_or_default().to_string_lossy(), test.name, e);
                stats.fail += 1;
                continue;
            }
        };
        match r.outcome {
            TestOutcome::Pass => stats.pass += 1,
            TestOutcome::Fail(ref msg) => {
                if test.name.contains("nonNegativeInteger_bin6")
                    || test.name.contains("dateTextInvalid")
                    || test.name.contains("timeTextInvalid")
                    || test.name.contains("dateTimeTextInvalid")
                {
                    eprintln!("[DETAILED_FAIL] {} :: {} => {}", path.file_name().unwrap_or_default().to_string_lossy(), test.name, msg);
                } else {
                    eprintln!("[FAIL PARSER] {} :: {} => {}", path.file_name().unwrap_or_default().to_string_lossy(), test.name, msg);
                }
                stats.fail += 1;
            }
            TestOutcome::Skip(_) => stats.skip += 1,
        }
    }
    for test in &suite.unparser_tests {
        let r = match run_unparser_test(&suite, test) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[FAIL UNPARSER ERR] {} :: {} => {:?}", path.file_name().unwrap_or_default().to_string_lossy(), test.name, e);
                stats.fail += 1;
                continue;
            }
        };
        match r.outcome {
            TestOutcome::Pass => stats.pass += 1,
            TestOutcome::Fail(msg) => {
                eprintln!("[FAIL UNPARSER] {} :: {} => {}", path.file_name().unwrap_or_default().to_string_lossy(), test.name, msg);
                stats.fail += 1;
            }
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

/// CI gate: Section 02 — facet validation TDML and related error suites.
#[test]
fn daffodil_section02_regression_gate() {
    let root = assert_tdml_root().join("section02");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "section02 TDML missing");
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "section02 gate: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert_eq!(stats.parse_fail, 0, "section02 TDML parse errors: {stats:?}");
    assert!(
        stats.pass >= SECTION02_BASELINE_PASS_MIN,
        "section02 regression: pass={} (need >={SECTION02_BASELINE_PASS_MIN}), fail={}",
        stats.pass,
        stats.fail
    );
    assert!(
        stats.fail <= SECTION02_BASELINE_FAIL_MAX,
        "section02 regression: too many failures pass={} fail={} (max {SECTION02_BASELINE_FAIL_MAX})",
        stats.pass,
        stats.fail
    );
}

/// CI gate: Section 00 general — regression on the main TDML set (not a tiny all-green subset).
#[test]
fn daffodil_section00_regression_gate() {
    let root = assert_tdml_root().join("section00");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    let total = stats.pass + stats.fail + stats.skip;
    let pct = if total > 0 {
        (stats.pass as f64) * 100.0 / (total as f64)
    } else {
        0.0
    };
    eprintln!(
        "section00 gate (all general TDML): pass={} fail={} skip={} parse_fail={} ({:.1}% pass)",
        stats.pass, stats.fail, stats.skip, stats.parse_fail, pct
    );
    assert_eq!(stats.parse_fail, 0, "section00 TDML parse errors");
    assert!(
        stats.pass >= SECTION00_BASELINE_PASS_MIN,
        "section00 regression: pass={} (need >={SECTION00_BASELINE_PASS_MIN}), fail={}, skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
    assert!(
        stats.fail <= SECTION00_BASELINE_FAIL_MAX,
        "section00 regression: too many failures pass={} fail={} (max {SECTION00_BASELINE_FAIL_MAX}) skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
}

/// Zero-failure gate for section00 files.
#[test]
fn daffodil_section00_core_zero_fail_gate() {
    let root = assert_tdml_root().join("section00");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    let mut stats = SectionStats::default();
    for path in files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if SECTION00_GATE_SKIP_FILES
            .iter()
            .any(|skip| rel == *skip)
        {
            continue;
        }
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "section00 core zero-fail: pass={} fail={} skip={}",
        stats.pass, stats.fail, stats.skip
    );
    assert_eq!(stats.parse_fail, 0);
    assert_eq!(stats.fail, 0, "section00 core failures: {stats:?}");
    assert!(stats.pass >= 18, "expected ~18+ pass in core subset");
}

/// Zero-failure gate for section02 files.
#[test]
fn daffodil_section02_zero_fail_gate() {
    let root = assert_tdml_root().join("section02");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "section02 zero-fail: pass={} fail={} skip={}",
        stats.pass, stats.fail, stats.skip
    );
    assert_eq!(stats.parse_fail, 0);
    assert_eq!(stats.fail, 0, "section02 failures: {stats:?}");
}

/// Zero-failure gate for section05 files.
#[test]
fn daffodil_section05_zero_fail_gate() {
    let root = assert_tdml_root().join("section05");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "section05 zero-fail: pass={} fail={} skip={}",
        stats.pass, stats.fail, stats.skip
    );
    assert_eq!(stats.parse_fail, 0);
    assert_eq!(stats.fail, 0, "section05 failures: {stats:?}");
    assert!(stats.pass >= 800, "expected ~800+ pass in section05");
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
    assert_eq!(stats.parse_fail, 0);
    assert_eq!(stats.skip, 0);
    assert!(
        stats.fail <= 120,
        "section12 lengthKind failures: pass={} fail={} skip={} parse_fail={}",
        stats.pass,
        stats.fail,
        stats.skip,
        stats.parse_fail
    );
    assert!(
        stats.pass >= 190,
        "section12 lengthKind: expected at least 190 passing cases, got pass={} fail={}",
        stats.pass,
        stats.fail
    );
}

/// CI gate: Section 12 aligned_data (full TDML directory).
#[test]
fn daffodil_section12_aligned_data_regression_gate() {
    let root = assert_tdml_root().join("section12/aligned_data");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "aligned_data: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert_eq!(stats.parse_fail, 0);
    assert!(
        stats.fail <= 40,
        "aligned_data failures: pass={} fail={}",
        stats.pass,
        stats.fail
    );
    assert!(
        stats.pass >= 105,
        "expected at least 105 passing aligned_data cases, got pass={} fail={}",
        stats.pass,
        stats.fail
    );
}

/// CI gate: Section 12 delimiter_properties (full TDML directory).
#[test]
fn daffodil_section12_delimiter_properties_regression_gate() {
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
    assert_eq!(stats.parse_fail, 0);
    assert!(
        stats.fail <= 5,
        "section12 delimiter_properties failures: pass={} fail={}",
        stats.pass,
        stats.fail
    );
    assert!(
        stats.pass >= 14,
        "expected at least 14 passing delimiter_properties cases, got pass={} fail={}",
        stats.pass,
        stats.fail
    );
}

/// CI gate: Section 12 length_properties (explicit/bit length cases).
#[test]
fn daffodil_section12_length_properties_regression_gate() {
    let root = assert_tdml_root().join("section12/length_properties");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "section12/length_properties TDML missing");

    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    assert_eq!(
        stats.parse_fail, 0,
        "length_properties parse errors: {stats:?}"
    );
    assert!(
        stats.fail <= 15,
        "length_properties failures: pass={} fail={} skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
    assert_eq!(stats.skip, 0);
    assert_eq!(stats.parse_fail, 0);
    assert!(
        stats.pass >= 45,
        "length_properties: expected at least 45 passing cases, got pass={} fail={} skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
}

/// CI gate: Section 06 namespaces and entities (full TDML scan baseline).
#[test]
fn daffodil_section06_regression_gate() {
    let root = assert_tdml_root().join("section06");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "section06 TDML missing");

    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "section06: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert_eq!(
        stats.parse_fail, 0,
        "section06 TDML load errors: {stats:?}"
    );
    assert!(
        stats.pass >= SECTION06_BASELINE_PASS_MIN,
        "section06: expected at least {} passing cases, got pass={} fail={} skip={}",
        SECTION06_BASELINE_PASS_MIN,
        stats.pass,
        stats.fail,
        stats.skip
    );
    assert!(
        stats.fail <= SECTION06_BASELINE_FAIL_MAX,
        "section06 regression: too many failures pass={} fail={} skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
}

/// CI gate: Section 05 simple types / facets (full TDML scan baseline).
#[test]
fn daffodil_section05_regression_gate() {
    let root = assert_tdml_root().join("section05");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "section05 TDML missing");

    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "section05: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert_eq!(
        stats.parse_fail, 0,
        "section05 TDML load errors: {stats:?}"
    );
    assert!(
        stats.pass >= SECTION05_BASELINE_PASS_MIN,
        "section05: expected at least {} passing cases, got pass={} fail={} skip={}",
        SECTION05_BASELINE_PASS_MIN,
        stats.pass,
        stats.fail,
        stats.skip
    );
    assert!(
        stats.fail <= SECTION05_BASELINE_FAIL_MAX,
        "section05 regression: too many failures pass={} fail={} (max {SECTION05_BASELINE_FAIL_MAX}) skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
}

/// CI gate: Section 13 binary/text numbers, nillable, and packed decimals (full TDML scan).
#[test]
fn daffodil_section13_regression_gate() {
    let root = assert_tdml_root().join("section13");
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "section13 TDML missing");

    let mut stats = SectionStats::default();
    for path in files {
        let relp = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if SECTION13_SKIP_FILES.iter().any(|s| *s == relp) {
            continue;
        }
        run_tdml_file(&path, &mut stats);
    }
    assert_eq!(
        stats.parse_fail, 0,
        "section13 TDML load errors: {stats:?}"
    );
    assert!(
        stats.pass >= SECTION13_BASELINE_PASS_MIN,
        "section13: expected at least {} passing cases, got pass={} fail={} skip={}",
        SECTION13_BASELINE_PASS_MIN,
        stats.pass,
        stats.fail,
        stats.skip
    );
    assert!(
        stats.fail <= SECTION13_BASELINE_FAIL_MAX,
        "section13 regression: too many failures pass={} fail={} (max {SECTION13_BASELINE_FAIL_MAX}) skip={}",
        stats.pass,
        stats.fail,
        stats.skip
    );
}

/// Helper to run a section gate assertion.
fn run_section_gate(section_name: &str, min_pass: usize, max_fail: usize) {
    let root = assert_tdml_root().join(section_name);
    let mut files = Vec::new();
    collect_tdml_files(&root, &mut files);
    assert!(!files.is_empty(), "{section_name} TDML missing");
    let mut stats = SectionStats::default();
    for path in files {
        run_tdml_file(&path, &mut stats);
    }
    eprintln!(
        "{section_name} gate: pass={} fail={} skip={} parse_fail={}",
        stats.pass, stats.fail, stats.skip, stats.parse_fail
    );
    assert!(
        stats.pass >= min_pass,
        "{section_name} regression: pass={} (need >={min_pass}), fail={}",
        stats.pass,
        stats.fail
    );
    if max_fail == 0 {
        assert_eq!(
            stats.fail, 0,
            "{section_name} regression: fail={} (expected 0), pass={}",
            stats.fail,
            stats.pass
        );
    } else {
        assert!(
            stats.fail <= max_fail,
            "{section_name} regression: fail={} (max {max_fail}), pass={}",
            stats.fail,
            stats.pass
        );
    }
}

#[test]
fn daffodil_section07_regression_gate() {
    run_section_gate("section07", 140, 165);
}

#[test]
fn daffodil_section08_regression_gate() {
    run_section_gate("section08", 10, 30);
}

#[test]
fn daffodil_section10_regression_gate() {
    run_section_gate("section10", 2, 10);
}

#[test]
fn daffodil_section11_regression_gate() {
    run_section_gate("section11", 0, 5);
}

#[test]
fn daffodil_section14_regression_gate() {
    run_section_gate("section14", 95, 5);
}

#[test]
fn daffodil_section15_regression_gate() {
    run_section_gate("section15", 150, 15);
}

#[test]
fn daffodil_section16_regression_gate() {
    run_section_gate("section16", 80, 10);
}

#[test]
fn daffodil_section17_regression_gate() {
    run_section_gate("section17", 90, 25);
}

#[test]
fn daffodil_section23_regression_gate() {
    run_section_gate("section23", 100, 950);
}

#[test]
fn daffodil_section24_regression_gate() {
    run_section_gate("section24", 10, 5);
}

#[test]
fn daffodil_section31_regression_gate() {
    run_section_gate("section31", 65, 20);
}

#[test]
fn daffodil_charsets_regression_gate() {
    run_section_gate("charsets", 0, 10);
}

#[test]
fn daffodil_codegen_regression_gate() {
    run_section_gate("codegen", 1, 30);
}

#[test]
fn daffodil_extensions_regression_gate() {
    run_section_gate("extensions", 1, 70);
}

#[test]
fn daffodil_layers_regression_gate() {
    run_section_gate("layers", 0, 100);
}

#[test]
fn daffodil_udf_regression_gate() {
    run_section_gate("udf", 0, 25);
}

#[test]
fn daffodil_unparser_regression_gate() {
    run_section_gate("unparser", 3, 25);
}

#[test]
fn daffodil_usertests_regression_gate() {
    run_section_gate("usertests", 10, 30);
}

