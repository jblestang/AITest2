//! Diagnostics for Section 13 cases (run with --ignored).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};
use std::fs;
use std::path::PathBuf;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn run(case: &str, tdml_rel: &str) -> TestOutcome {
    let text = fs::read_to_string(PathBuf::from(TDML).join(tdml_rel)).expect("read");
    let suite = parse_tdml(&text).expect("parse tdml");
    if let Some(name) = case.strip_prefix("unparse:") {
        let test = suite
            .unparser_tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("unparser case `{name}`"));
        return run_unparser_test(&suite, test).expect("run").outcome;
    }
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == case)
        .expect("parser case");
    run_parser_test(&suite, test).expect("run").outcome
}

#[test]
#[ignore]
fn debug_section13_cases() {
    let cases = [
        ("booleanDefaultSDE", "section13/boolean/boolean.tdml"),
        ("booleanInputValueCalc", "section13/boolean/boolean.tdml"),
        ("booleanInputValueCalcError", "section13/boolean/boolean.tdml"),
        ("unparse:text_complex_nil4", "section13/nillable/literal-value-nils-unparse.tdml"),
        ("test_complex_nil", "section13/nillable/literal-value-nils.tdml"),
        ("packedCharset08", "section13/packed/packed.tdml"),
        ("runtimeLengthPackedCharset1", "section13/packed/packed.tdml"),
        ("packedCharset10", "section13/packed/packed.tdml"),
        ("bcdCharset07", "section13/packed/packed.tdml"),
        ("bcdCharset08", "section13/packed/packed.tdml"),
        ("bcdCharset09", "section13/packed/packed.tdml"),
        ("bcdCharset10", "section13/packed/packed.tdml"),
        ("bcdCharset11", "section13/packed/packed.tdml"),
        ("bcdCharset13", "section13/packed/packed.tdml"),
        ("IBM4690Charset08", "section13/packed/packed.tdml"),
        ("IBM4690Charset10", "section13/packed/packed.tdml"),
        ("textNumberPattern_positiveMandatory", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_negativeIgnored04", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_specialChar03", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_specialChar07", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparator01", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparator03", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparator05", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparatorOneOnly1", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparatorOneOnly2", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparator14", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparator15", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardDecimalSeparator16", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardGroupingSeparator02", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardGroupingSeparator04", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardGroupingSeparator06", "section13/text_number_props/TextNumberProps.tdml"),
        ("textStandardGroupingSeparator07", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_scientificNotation07", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding01", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding04", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding05", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding06", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding07", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding08", "section13/text_number_props/TextNumberProps.tdml"),
        ("textNumberPattern_padding09", "section13/text_number_props/TextNumberProps.tdml"),
    ];

    let mut pass_cnt = 0;
    let mut fail_cnt = 0;
    for (case, file) in cases {
        let res = run(case, file);
        if matches!(res, TestOutcome::Pass) {
            pass_cnt += 1;
        } else {
            fail_cnt += 1;
            eprintln!("{case}: {res:?}");
        }
    }
    eprintln!("\n=== SUMMARY === pass={pass_cnt} fail={fail_cnt}");
    assert_eq!(fail_cnt, 0, "{fail_cnt} test cases failed");
}
