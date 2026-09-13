//! Section 13 (binary/text number representation, nillable, decimal) regression subset.
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TestOutcome};

macro_rules! section13_tdml {
    ($rel:literal) => {
        include_str!(concat!(
            "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section13/",
            $rel
        ))
    };
}

fn assert_section13_case(tdml: &str, name: &str) {
    let suite = parse_tdml(tdml).expect("parse tdml");
    if let Some(unparse_name) = name.strip_prefix("unparse:") {
        let test = suite
            .unparser_tests
            .iter()
            .find(|t| t.name == unparse_name)
            .unwrap_or_else(|| panic!("unparser test `{unparse_name}` not found"));
        let result = run_unparser_test(&suite, test).expect("run unparser test");
        match result.outcome {
            TestOutcome::Pass => {}
            TestOutcome::Fail(msg) => panic!("unparser `{unparse_name}` failed: {msg}"),
            TestOutcome::Skip(msg) => panic!("unparser `{unparse_name}` skipped: {msg}"),
        }
    } else {
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("parser test `{name}` not found"));
        let result = run_parser_test(&suite, test).expect("run parser test");
        match result.outcome {
            TestOutcome::Pass => {}
            TestOutcome::Fail(msg) => panic!("test `{name}` failed: {msg}"),
            TestOutcome::Skip(msg) => panic!("test `{name}` skipped: {msg}"),
        }
    }
}

macro_rules! section13_file_cases {
    ($file:literal, $names:expr) => {{
        let tdml = section13_tdml!($file);
        for name in $names {
            assert_section13_case(tdml, name);
        }
    }};
}

/// Parser/unparser cases that pass today (`packed/packed.tdml` full scan in section13_smoke).
#[test]
fn daffodil_section13_regression_suite() {
    section13_file_cases!("nillable/literal-character-nils.tdml", &["text_01"]);
    section13_file_cases!("decimal/TestDecimalSigned.tdml", &[
        "parseTestDecimalSigned_no_binary",
    ]);
    section13_file_cases!("nillable/literal-value-nils-unparse.tdml", &[
        "unparse:scalar_nonDefaultable_nillable",
        "unparse:scalar_nonDefaultable_nillable_02",
        "unparse:scalar_nonDefaultable_nillable_03",
        "unparse:text_nil_only2",
        "unparse:text_nil_only4",
        "unparse:text_nil_only5",
        "unparse:text_nil_only6",
        "unparse:text_nil_only7",
        "unparse:text_nil_only8",
        "unparse:text_nil_only9",
        "unparse:text_nil_only10",
        "unparse:text_nil_only11",
        "unparse:text_nil_only12",
        "unparse:text_nil_only13",
        "unparse:text_nil_only14",
        "unparse:text_nil_only15",
        "unparse:text_nil_only16",
        "unparse:text_nil_only17",
        "unparse:text_nil_characterClass_01",
        "unparse:text_nil_characterClass_04",
    ]);
    section13_file_cases!("nillable/literal-value-nils.tdml", &[
        "binary_01",
        "text_nil_characterClass_04_parse",
        "nillable_ovc_01",
    ]);
    section13_file_cases!("nillable/nillable.tdml", &[
        "litNil5",
        "litNil6",
        "missing_scalar",
        "litNil7",
        "edifact1a",
        "complexNillable_01",
    ]);
    section13_file_cases!("text_number_props/TextNumberProps.tdml", &[
        "textNumberPattern_negativeIgnored01b",
        "textNumberPattern_negativeIgnored05",
        "textNumberCheckPolicy_strict04",
        "standardZeroRep04b",
        "standardZeroRep08",
    ]);
    section13_file_cases!("text_number_props/TextNumberPropsUnparse.tdml", &[
        "parseDelimitedPaddedString01",
        "parse_int_01",
        "unparse:unparseDelimitedPaddedString02",
        "unparse:unparseDelimitedPaddedString04",
        "unparse:unparsePaddedStringTruncate02",
        "unparse:unparsePaddedStringTruncate04",
        "unparse:unparsePaddedStringTruncate05",
        "unparse:unparsePaddedStringTruncate06",
        "unparse:unparseDelimitedPaddedString06",
        "unparse:unparseDelimitedPaddedString07",
        "unparse:unparseDelimitedPaddedString08",
        "unparse:unparseDelimitedPaddedString09",
        "unparse:unparsePaddedString10",
        "unparse:unparsePaddedString11",
        "unparse:unparseDelimitedPaddedString11",
        "unparse:unparse_tnp_05b",
        "unparse:textStandardZeroRep2",
    ]);
    section13_file_cases!("packed/packed.tdml", &[
        "hexCharset01",
        "packedCharset01",
        "packedCharset02",
        "packedCharset03",
        "DelimitedPackedIntSeq",
        "DelimitedPackedDecSeq",
        "DelimitedBCDIntSeq",
        "DelimitedBCDDecSeq",
        "DelimitedIBM4690IntSeq",
        "bcdCharset01",
        "bcdCharset02",
    ]);
    section13_file_cases!("text_number_props/TextStandardBase.tdml", &[
        "base2_integer_min",
        "base8_integer_min",
        "base16_integer_min",
        "base2_long_min",
        "base8_long_min",
        "base16_long_min",
        "base2_int_min",
        "base8_int_min",
        "base16_int_min",
        "base2_short_min",
        "base8_short_min",
        "base16_short_min",
        "base2_byte_min",
        "base8_byte_min",
        "base16_byte_min",
        "base2_uinteger_min",
        "base8_uinteger_min",
        "base16_uinteger_min",
        "base2_ulong_min",
        "base8_ulong_min",
        "base16_ulong_min",
        "base2_uint_min",
        "base8_uint_min",
        "base16_uint_min",
        "base2_ushort_min",
        "base8_ushort_min",
        "base16_ushort_min",
        "base2_ubyte_min",
        "base8_ubyte_min",
        "base16_ubyte_min",
        "base2_long_max",
        "base16_int_max",
        "base16_ulong_max",
    ]);
}
