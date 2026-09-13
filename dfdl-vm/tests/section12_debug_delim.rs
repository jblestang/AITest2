//! Find slow/hanging delimiter_properties TDML cases.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/delimiter_properties"
);

fn run_file(rel: &str) {
    let path = PathBuf::from(TDML).join(rel);
    let text = fs::read_to_string(&path).expect("read");
    let suite = parse_tdml(&text).expect("parse");
    eprintln!("=== {rel} ===");
    for t in &suite.tests {
        eprint!("  parser {} ... ", t.name);
        let start = Instant::now();
        let r = run_parser_test(&suite, t);
        let elapsed = start.elapsed();
        eprintln!("{elapsed:?} {:?}", r.as_ref().map(|x| &x.outcome));
        if elapsed > Duration::from_secs(2) {
            eprintln!("  SLOW parser {}", t.name);
        }
    }
    for t in &suite.unparser_tests {
        eprint!("  unparse {} ... ", t.name);
        let start = Instant::now();
        let r = run_unparser_test(&suite, t);
        let elapsed = start.elapsed();
        eprintln!("{elapsed:?} {:?}", r.as_ref().map(|x| &x.outcome));
        if elapsed > Duration::from_secs(2) {
            eprintln!("  SLOW unparse {}", t.name);
        }
    }
}

#[test]
#[ignore]
fn debug_delimiter_properties_timing() {
    run_file("DelimiterProperties.tdml");
    run_file("DelimiterPropertiesUnparse.tdml");
}
