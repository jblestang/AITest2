use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};
use std::fs;
use std::path::PathBuf;

const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data"
);

#[test]
#[ignore]
fn print_all_failures_with_messages() {
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
            if let TestOutcome::Fail(msg) = r.outcome {
                eprintln!(
                    "PARSER FAIL {} :: {}",
                    t.name,
                    msg.chars().take(120).collect::<String>()
                );
            }
        }
        for t in &suite.unparser_tests {
            let r = run_unparser_test(&suite, t).expect("run");
            if let TestOutcome::Fail(msg) = r.outcome {
                eprintln!(
                    "UNPARSER FAIL {} :: {}",
                    t.name,
                    msg.chars().take(120).collect::<String>()
                );
            }
        }
    }
}
