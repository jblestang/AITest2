use dfdl_vm::tdml::{
    parse_tdml, run_parser_test, run_unparser_test, TdmlSchema, TdmlSuite, TestOutcome,
};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

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

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section07/escapeScheme"
);

#[test]
#[ignore]
fn scan_section07_escape_scheme() {
    let dir = Path::new(DIR);
    let mut files = Vec::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "tdml") {
            files.push(p);
        }
    }
    files.sort();
    let mut pass = 0usize;
    let mut fail = 0usize;
    for path in files {
        let tdml = fs::read_to_string(&path).unwrap();
        let mut suite = parse_tdml(&tdml).unwrap();
        enrich_external_tdml_models(&mut suite, &path);
        let relp = path.file_name().unwrap().to_string_lossy();
        for t in &suite.tests {
            let r = run_parser_test(&suite, t).unwrap();
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    eprintln!("FAIL {relp}::{}: {msg}", t.name);
                }
                TestOutcome::Skip(msg) => eprintln!("SKIP {relp}::{}: {msg}", t.name),
            }
        }
        for t in &suite.unparser_tests {
            let r = run_unparser_test(&suite, t).unwrap();
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    eprintln!("FAIL {relp}::unparse:{}: {msg}", t.name);
                }
                TestOutcome::Skip(msg) => eprintln!("SKIP {relp}::unparse:{}: {msg}", t.name),
            }
        }
    }
    eprintln!("\n=== section07/escapeScheme === pass={pass} fail={fail}");
}
