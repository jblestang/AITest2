use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TestOutcome, TdmlResourceContext, TdmlSchema,
    TdmlSuite,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section00"
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
        let Ok(xsd) = fs::read_to_string(&p) else { continue };
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
fn list_all_failures() {
    let root = Path::new(ROOT);
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    let mut by_prefix: BTreeMap<String, usize> = BTreeMap::new();
    let mut n = 0usize;
    for path in files {
        let Ok(tdml) = fs::read_to_string(&path) else { continue };
        let Ok(mut suite) = parse_tdml(&tdml) else { continue };
        enrich(&mut suite, &path);
        let file = path.file_name().unwrap().to_string_lossy();
        let mut file_fail = 0usize;
        for t in &suite.tests {
            let Ok(r) = run_parser_test(&suite, t) else { continue };
            if let TestOutcome::Fail(msg) = r.outcome {
                n += 1;
                file_fail += 1;
                let prefix = msg.split(':').next().unwrap_or("").trim().to_string();
                *by_prefix.entry(prefix).or_default() += 1;
                if n <= 95 {
                    eprintln!("{file}::{}: {msg}", t.name);
                }
            }
        }
        for t in &suite.unparser_tests {
            let Ok(r) = run_unparser_test(&suite, t) else { continue };
            if let TestOutcome::Fail(msg) = r.outcome {
                n += 1;
                file_fail += 1;
                let prefix = msg.split(':').next().unwrap_or("").trim().to_string();
                *by_prefix.entry(prefix).or_default() += 1;
                if n <= 95 {
                    eprintln!("{file}::unparse:{}: {msg}", t.name);
                }
            }
        }
        if file_fail > 0 {
            eprintln!("-- {file}: fail={file_fail}");
        }
    }
    eprintln!("\n=== failure categories ({n} total) ===");
    for (k, v) in by_prefix {
        eprintln!("  {v:>3}  {k}");
    }
}
