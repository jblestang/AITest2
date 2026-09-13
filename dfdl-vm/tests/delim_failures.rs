use dfdl_vm::tdml::{parse_tdml, run_parser_test};

macro_rules! tdml {
    ($path:literal) => {
        include_str!(concat!(
            "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/",
            $path
        ))
    };
}

fn run(name: &str) {
    let suite = parse_tdml(tdml!("DelimitedTests.tdml")).unwrap();
    let test = suite.tests.iter().find(|t| t.name == name).unwrap();
    let r = run_parser_test(&suite, test).unwrap();
    eprintln!("{name}: {:?}", r.outcome);
}

#[test]
fn print_delimited_failures() {
    for name in [
        "lengthKindDelimited_02",
        "lengthKindDelimited_03",
        "lengthKindDelimited_04",
        "NumSeq_09",
        "NumSeq_11",
        "NumSeq_12",
        "delimsCheck",
    ] {
        run(name);
    }
}
