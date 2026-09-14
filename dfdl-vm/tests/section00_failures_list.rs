use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TestOutcome, TdmlResourceContext, TdmlSchema,
    TdmlSuite,
};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const SKIP: &[&str] = &[
    "general/testUnparserGeneral.tdml",
    "general/testUnparserFileBuffering.tdml",
    "general/tunables.tdml",
    "general/parseUnparsePolicy.tdml",
    "general/testElementFormDefault.tdml",
];

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let Some(dir) = path.parent() else { return };
    suite.resource_context = TdmlResourceContext::from_tdml_path(&path.to_string_lossy());
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

#[test]
#[ignore]
fn list_section00_gate_failures() {
    let root = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section00"
    ));
    for entry in walkdir(root) {
        let rel = entry.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if SKIP.iter().any(|s| *s == rel) {
            continue;
        }
        let tdml = fs::read_to_string(&entry).unwrap();
        let mut suite = parse_tdml(&tdml).unwrap();
        enrich(&mut suite, &entry);
        for t in &suite.tests {
            let r = run_parser_test(&suite, t).unwrap();
            if let TestOutcome::Fail(msg) = r.outcome {
                eprintln!("{rel}::{}: {msg}", t.name);
            }
        }
        for t in &suite.unparser_tests {
            let r = run_unparser_test(&suite, t).unwrap();
            if let TestOutcome::Fail(msg) = r.outcome {
                eprintln!("{rel}::unparse:{}: {msg}", t.name);
            }
        }
    }
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else if p.extension().is_some_and(|x| x == "tdml") {
            out.push(p);
        }
    }
    out.sort();
    out
}
