use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data"
);

#[test]
#[ignore]
fn categorize_aligned_data_failures() {
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut cats: BTreeMap<String, usize> = BTreeMap::new();
    let dir = PathBuf::from(ROOT);
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("tdml") {
            continue;
        }
        let tdml = fs::read_to_string(&path).unwrap();
        let suite = parse_tdml(&tdml).expect("parse tdml");
        for t in &suite.tests {
            let r = run_parser_test(&suite, t).expect("run");
            match r.outcome {
                TestOutcome::Pass => pass += 1,
                TestOutcome::Fail(msg) => {
                    fail += 1;
                    let key = if msg.contains("invalid alignment `implicit`") {
                        "invalid alignment implicit".into()
                    } else if let Some(i) = msg.find("decode error:") {
                        msg[i..].chars().take(70).collect()
                    } else if let Some(i) = msg.find("compile error") {
                        msg[i..].chars().take(70).collect()
                    } else {
                        msg.chars().take(70).collect()
                    };
                    *cats.entry(key).or_default() += 1;
                }
                TestOutcome::Skip(_) => {}
            }
        }
    }
    eprintln!("pass={pass} fail={fail}");
    for (k, v) in &cats {
        eprintln!("{v:4} {k}");
    }
}

#[test]
#[ignore]
fn list_aligned_data_failures() {
    let dir = PathBuf::from(ROOT);
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("tdml") {
            continue;
        }
        let tdml = fs::read_to_string(&path).unwrap();
        let suite = parse_tdml(&tdml).expect("parse");
        for t in &suite.tests {
            let r = run_parser_test(&suite, t).expect("run");
            if matches!(r.outcome, TestOutcome::Fail(_)) {
                eprintln!("FAIL {}", t.name);
            }
        }
    }
}

#[test]
#[ignore]
fn list_aligned_data_passes() {
    let dir = PathBuf::from(ROOT);
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("tdml") {
            continue;
        }
        let tdml = fs::read_to_string(&path).unwrap();
        let suite = parse_tdml(&tdml).expect("parse");
        for t in &suite.tests {
            let r = run_parser_test(&suite, t).expect("run");
            if matches!(r.outcome, TestOutcome::Pass) {
                eprintln!("PASS {}", t.name);
            }
        }
    }
}
