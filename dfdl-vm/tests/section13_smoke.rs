//! Focused Section 13 smoke tests for schema/VM fixes (not a full gate).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;
use std::path::PathBuf;

const TDML: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn tdml(path: &str) -> PathBuf {
    PathBuf::from(TDML).join(path)
}

fn run_named(tdml_rel: &str, case_name: &str) -> TestOutcome {
    let text = fs::read_to_string(tdml(tdml_rel)).expect("read tdml");
    let suite = parse_tdml(&text).expect("parse tdml");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == case_name)
        .unwrap_or_else(|| panic!("case `{case_name}` not in {tdml_rel}"));
    let r = run_parser_test(&suite, test).expect("run parser test");
    r.outcome
}

#[test]
fn section13_exp_char_classes_compile() {
    match run_named(
        "section13/text_number_props/TextNumberProps.tdml",
        "expCharClasses",
    ) {
        TestOutcome::Pass => {}
        other => panic!("expCharClasses: {other:?}"),
    }
}

#[test]
fn section13_text_standard_decimal_separator08() {
    match run_named(
        "section13/text_number_props/TextNumberProps.tdml",
        "textStandardDecimalSeparator08",
    ) {
        TestOutcome::Pass => {}
        other => panic!("textStandardDecimalSeparator08: {other:?}"),
    }
}

#[test]
fn section13_scientific_and_grouping() {
    for name in [
        "textNumberPattern_scientificNotation03",
        "textNumberPattern_scientificNotation04",
        "textNumberPattern_scientificNotation05",
        "textNumberPattern_scientificNotation06",
        "textStandardGroupingSeparator13",
    ] {
        match run_named("section13/text_number_props/TextNumberProps.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_number_padding() {
    for name in [
        "textNumberPattern_padding01",
        "textNumberPattern_padding05",
        "textNumberPattern_padding06",
        "textNumberPattern_padding07",
        "textNumberPattern_padding08",
        "textNumberPattern_padding09",
        "textNumberPattern_paddingCombo01",
        "textNumberPattern_padding11",
    ] {
        match run_named("section13/text_number_props/TextNumberProps.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_zoned_ebcdic() {
    match run_named(
        "section13/zoned/zoned2.tdml",
        "ZonedEBCDICLeadingOverpunchedSign",
    ) {
        TestOutcome::Pass => {}
        other => panic!("ZonedEBCDICLeadingOverpunchedSign: {other:?}"),
    }
}

#[test]
fn section13_zoned_ebcdic_b5() {
    match run_named(
        "section13/zoned/zoned2.tdml",
        "ZonedEBCDICLeadingOverpunchedSign_B5",
    ) {
        TestOutcome::Pass => {}
        other => panic!("ZonedEBCDICLeadingOverpunchedSign_B5: {other:?}"),
    }
}

#[test]
fn section13_zoned_standard() {
    for name in ["ZonedStandard01", "ZonedStandard02", "ZonedStandard05"] {
        match run_named("section13/zoned/zoned.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_number_p_symbol() {
    for name in ["textNumberPattern_pSymbol01", "textNumberPattern_pSymbol02"] {
        match run_named("section13/text_number_props/TextNumberProps.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_standard_separator_siblings() {
    for name in [
        "textStandardDecimalSeparator05",
        "textStandardGroupingSeparator05",
        "textStandardGroupingSeparator08",
    ] {
        match run_named("section13/text_number_props/TextNumberProps.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_number_exponent01() {
    match run_named(
        "section13/text_number_props/TextNumberProps.tdml",
        "textNumberPattern_exponent01",
    ) {
        TestOutcome::Pass => {}
        other => panic!("textNumberPattern_exponent01: {other:?}"),
    }
}

#[test]
fn section13_literal_character_text_01() {
    match run_named("section13/nillable/literal-character-nils.tdml", "text_01") {
        TestOutcome::Pass => {}
        other => panic!("text_01: {other:?}"),
    }
}

#[test]
fn section13_pv_vpatterns() {
    for name in ["vpattern_01", "vpattern_05", "ppattern_02"] {
        match run_named("section13/zoned/pv.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_standard_base_errors() {
    for name in [
        "base2_invalid_char_err",
        "non_base_10_empty_string_err",
        "unsupported_base_err",
    ] {
        match run_named("section13/text_number_props/TextStandardBase.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_standard_base_max_samples() {
    for name in ["base2_long_max", "base16_int_max", "base16_ulong_max"] {
        match run_named("section13/text_number_props/TextStandardBase.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_text_standard_base_schema_loads() {
    let text =
        fs::read_to_string(tdml("section13/text_number_props/TextStandardBase.tdml")).expect("read");
    parse_tdml(&text).expect("TextStandardBase.tdml should parse after textStandardBase support");
}

#[test]
fn section13_packed_hex_and_sign_cases() {
    for name in [
        "hexCharset01",
        "packedCharset01",
        "packedCharset02",
        "packedCharset03",
        "DelimitedPackedIntSeq",
        "DelimitedPackedDecSeq",
        "DelimitedBCDIntSeq",
        "DelimitedBCDDecSeq",
        "DelimitedIBM4690IntSeq",
    ] {
        match run_named("section13/packed/packed.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn section13_packed_tdml_scan() {
    use dfdl_vm::tdml::{run_parser_test, TestOutcome};
    let text = fs::read_to_string(tdml("section13/packed/packed.tdml")).expect("read packed");
    let suite = parse_tdml(&text).expect("parse packed");
    let mut pass = 0usize;
    let mut fail = 0usize;
    for test in &suite.tests {
        let Ok(r) = run_parser_test(&suite, test) else {
            fail += 1;
            continue;
        };
        match r.outcome {
            TestOutcome::Pass => pass += 1,
            TestOutcome::Fail(msg) => {
                eprintln!("FAIL {}: {msg}", test.name);
                fail += 1;
            }
            TestOutcome::Skip(_) => {}
        }
    }
    eprintln!("packed.tdml: pass={pass} fail={fail}");
    assert_eq!(
        fail, 0,
        "packed.tdml: expected all parser cases to pass, got pass={pass} fail={fail}"
    );
}


#[test]
fn section13_separator_sde_smoke() {
    for name in [
        "textStandardGroupingSeparator03",
        "textStandardGroupingSeparator04",
        "textStandardGroupingSeparator07",
        "textStandardGroupingSeparator09",
        "textStandardGroupingSeparator10",
        "textStandardGroupingSeparator12",
        "textStandardDecimalSeparatorOneOnly1",
        "textStandardDecimalSeparator16",
        "textStandardDecimalSeparator17",
    ] {
        match run_named("section13/text_number_props/TextNumberProps.tdml", name) {
            TestOutcome::Pass => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}
