use dfdl_vm::tdml::{parse_tdml, run_parser_test};

macro_rules! daffodil_tdml {
    ($file:literal) => {
        include_str!(concat!(
            "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/",
            $file
        ))
    };
}

#[test]
fn probe_remaining_prefixed() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    let suite = parse_tdml(tdml).expect("parse tdml");
    for name in [
        "pl_complex_bin_bytes_suspension",
        "pl_complex_bin_bytes_suspension_includes",
        "pl_text_dec_txt_chars",
        "pl_text_bool_txt_chars",
        "plSlash1_data",
        "pl_complexContentLengthBytes_1",
        "pl_simpleValueLengthBytes_1",
        "pl_text_string_txt_bytes_not_enough_prefix_data_includes_backtrack",
    ] {
        let test = suite.tests.iter().find(|t| t.name == name).unwrap();
        let r = run_parser_test(&suite, test).unwrap();
        eprintln!("{name}: {:?}", r.outcome);
    }
}
