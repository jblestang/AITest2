use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome, TdmlSchema, TdmlSuite};
use std::collections::HashSet;
use std::env;
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

#[test]
#[ignore]
fn run_single_tdml_file() {
    let path = env::var("TDML_FILE").expect("TDML_FILE");
    let tdml = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&tdml).expect("parse");
    enrich_external_tdml_models(&mut suite, Path::new(&path));
    if let Ok(only) = env::var("TDML_TEST") {
        let t = suite
            .tests
            .iter()
            .find(|t| t.name == only)
            .unwrap_or_else(|| panic!("test {only} not found"));
        let r = run_parser_test(&suite, t).expect("run");
        eprintln!("{only}: {:?}", r.outcome);
        return;
    }
    let mut pass = 0;
    let mut fail = 0;
    for t in &suite.tests {
        eprintln!("running {}", t.name);
        match run_parser_test(&suite, t) {
            Ok(r) => match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    eprintln!("FAIL {}: {msg}", t.name);
                }
                TestOutcome::Skip(_) => {}
            },
            Err(e) => eprintln!("ERR {}: {e}", t.name),
        }
    }
    for t in &suite.unparser_tests {
        match run_unparser_test(&suite, t) {
            Ok(r) => match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    eprintln!("FAIL unparse {}: {msg}", t.name);
                }
                TestOutcome::Skip(_) => {}
            },
            Err(e) => eprintln!("ERR unparse {}: {e}", t.name),
        }
    }
    eprintln!("pass={pass} fail={fail}");
}
