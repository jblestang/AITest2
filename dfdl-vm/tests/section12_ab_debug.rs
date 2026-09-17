use dfdl_vm::tdml::{parse_tdml, run_parser_test, TdmlSchema, TdmlSuite};
use std::fs;
use std::path::Path;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AB.tdml"
);

fn enrich(suite: &mut TdmlSuite, path: &Path) {
    let dir = path.parent().unwrap();
    let dir_str = dir.to_string_lossy().into_owned();
    for t in &suite.tests {
        let m = t.model.clone();
        if suite.schemas.contains_key(&m) || !m.ends_with(".xsd") {
            continue;
        }
        let Ok(xsd) = fs::read_to_string(dir.join(&m)) else {
            continue;
        };
        suite.schemas.insert(
            m.clone(),
            TdmlSchema {
                name: m,
                xsd,
                compile_base_dir: Some(dir_str.clone()),
            },
        );
    }
}

#[test]
#[ignore]
fn debug_ab_cases() {
    let path = Path::new(TDML);
    let tdml = fs::read_to_string(path).unwrap();
    let mut suite = parse_tdml(&tdml).unwrap();
    enrich(&mut suite, path);
    for name in ["AB001", "AB000", "AN000", "nested_seq"] {
        let t = suite.tests.iter().find(|t| t.name == name);
        if t.is_none() {
            continue;
        }
        let t = t.unwrap();
        match run_parser_test(&suite, t) {
            Ok(r) => eprintln!("{name}: {:?}", r.outcome),
            Err(e) => eprintln!("{name}: compile {e}"),
        }
    }
}
