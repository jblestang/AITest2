use dfdl_vm::tdml::{effective_round_trip, parse_tdml, run_parser_test, run_parser_test_with_options, run_unparser_test, RoundTrip, TestOutcome};

macro_rules! daffodil_tdml {
    ($file:literal) => {
        include_str!(concat!(
            "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/",
            $file
        ))
    };
}

fn assert_named_unparser_test_passes(tdml: &str, test_name: &str) {
    let suite = parse_tdml(tdml).expect("parse tdml");
    let test = suite
        .unparser_tests
        .iter()
        .find(|t| t.name == test_name)
        .unwrap_or_else(|| panic!("unparser test `{test_name}` not found"));
    let result = run_unparser_test(&suite, test).expect("run test");
    match result.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("test `{test_name}` failed: {msg}"),
        TestOutcome::Skip(msg) => panic!("test `{test_name}` skipped: {msg}"),
    }
}

fn assert_decode_encode_roundtrip(tdml: &str, test_name: &str) {
    let suite = parse_tdml(tdml).expect("parse tdml");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == test_name)
        .unwrap_or_else(|| panic!("test `{test_name}` not found"));
    let schema = suite
        .schemas
        .get(&test.model)
        .unwrap_or_else(|| panic!("schema `{}` not found", test.model));
    let spec = dfdl_vm::api::DfdlSpec::from_xsd_root(&schema.xsd, Some(&test.root)).expect("spec");
    let input = &test.documents[0].data;
    let decoded = spec.decode(input).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, *input, "roundtrip mismatch for {test_name}");
}

fn assert_named_test_passes(tdml: &str, test_name: &str) {
    let suite = parse_tdml(tdml).expect("parse tdml");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == test_name)
        .unwrap_or_else(|| panic!("test `{test_name}` not found"));
    let result = run_parser_test(&suite, test).expect("run test");
    match result.outcome {
        TestOutcome::Pass => {}
        TestOutcome::Fail(msg) => panic!("test `{test_name}` failed: {msg}"),
        TestOutcome::Skip(msg) => panic!("test `{test_name}` skipped: {msg}"),
    }
}

// --- AI / pattern / explicit (Section 12) ---

#[test]
fn daffodil_ai_length_kind_pattern() {
    assert_named_test_passes(daffodil_tdml!("AI.tdml"), "AI000");
}

#[test]
fn daffodil_explicit_length_address() {
    assert_named_test_passes(daffodil_tdml!("ExplicitTests.tdml"), "Lesson1_lengthKind_explicit");
}

#[test]
fn daffodil_section12_pattern_suite() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "AI000_rev");
}

#[test]
fn daffodil_length_kind_pattern_alternation() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_01");
}

#[test]
fn daffodil_length_kind_pattern_no_match() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_02");
}

#[test]
fn daffodil_length_kind_pattern_comma() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_03");
}

#[test]
fn daffodil_length_kind_pattern_simple_type() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPattern_04");
}

#[test]
fn daffodil_length_kind_pattern_unicode_fail() {
    assert_named_test_passes(daffodil_tdml!("PatternTests.tdml"), "lengthKindPatternFail");
}

// --- Delimited (Section 12) ---

#[test]
fn daffodil_length_kind_delimited_address() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "Lesson1_lengthKind_delimited");
}

#[test]
fn daffodil_length_kind_delimited_int_terminator() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_00a");
}

#[test]
fn daffodil_length_kind_delimited_newline() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_00nl");
}

#[test]
fn daffodil_length_kind_delimited_unbounded_list() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_01");
}

#[test]
fn daffodil_length_kind_delimited_no_trailing_delim() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_03");
}

#[test]
fn daffodil_length_kind_delimited_nested_unbounded() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "nested_NumSeq_01");
}

#[test]
fn daffodil_length_kind_delimited_mixed_implicit() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_04");
}

// --- AN path delimited (Section 12) ---

#[test]
fn daffodil_an_path_with_file() {
    assert_named_test_passes(daffodil_tdml!("AN.tdml"), "AN000");
}

#[test]
fn daffodil_an_path_folders_only() {
    assert_named_test_passes(daffodil_tdml!("AN.tdml"), "AN001");
}

// --- Implicit binary (Section 12) ---

#[test]
fn daffodil_implicit_binary_ignored_length() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "implicit_ignored_len");
}

#[test]
fn daffodil_implicit_with_len_sde() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "implicit_with_len");
}

// --- endOfParent NYI negative (Section 12) ---

#[test]
fn daffodil_end_of_parent_nyi_simple() {
    assert_named_test_passes(
        daffodil_tdml!("EndOfParentTests.tdml"),
        "TestEndOfParentNYISimpleTypes",
    );
}

// --- AB implicit CSV matrix (Section 12) ---

#[test]
fn daffodil_ab_implicit_csv_matrix() {
    assert_named_test_passes(daffodil_tdml!("AB.tdml"), "AB000");
}

#[test]
fn daffodil_ab_implicit_csv_postfix_separator() {
    assert_named_test_passes(daffodil_tdml!("AB.tdml"), "AB001");
}

#[test]
fn daffodil_ab_implicit_csv_nillable() {
    assert_named_test_passes(daffodil_tdml!("AB.tdml"), "AB002");
}

// --- Delimited: compound newline, eof, mixed sequences (Section 12) ---

#[test]
fn daffodil_delimited_double_newline_terminator() {
    assert_named_test_passes(
        daffodil_tdml!("DelimitedTests.tdml"),
        "TestDoubleNewLineTerminator",
    );
}

#[test]
fn daffodil_delimited_double_newline_separator() {
    assert_named_test_passes(
        daffodil_tdml!("DelimitedTests.tdml"),
        "TestDoubleNewLineSeparator",
    );
}

#[test]
fn daffodil_delimited_double_newline_separator_basic() {
    assert_named_test_passes(
        daffodil_tdml!("DelimitedTests.tdml"),
        "TestDoubleNewLineSeparatorBasic",
    );
}

#[test]
fn daffodil_delimited_eof_no_enclosing_patterns() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "eofTest1");
}

#[test]
fn daffodil_delimited_fixed_length_suspends_scanning() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "delimited_construct");
}

#[test]
fn daffodil_delimited_mixed_type_sequence() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_05");
}

#[test]
fn daffodil_delimited_nested_mixed_sequence() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_06");
}

#[test]
fn daffodil_delimited_initiator_on_element_ref() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "refInitiator");
}

#[test]
fn daffodil_delimited_initiator_on_element_decl() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "refInitiator2");
}

#[test]
fn daffodil_delimited_optional_nested_ref() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_07");
}

#[test]
fn daffodil_delimited_optional_nested_ref_min_zero() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_08");
}

#[test]
fn daffodil_delimited_explicit_length_complex() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_09");
}

#[test]
fn daffodil_delimited_nested_initiator_terminator() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_11");
}

#[test]
fn daffodil_delimited_empty_nested_group() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_12");
}

#[test]
fn daffodil_delimited_prefix_separator_complex() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "NumSeq_14");
}

#[test]
fn daffodil_delimited_compound_wsp_separator_space() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_01");
}

#[test]
fn daffodil_delimited_compound_wsp_separator_tab() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_02");
}

#[test]
fn daffodil_delimited_unused_trailing_bytes() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_03");
}

#[test]
fn daffodil_delimited_unused_trailing_bytes_no_extra_elem() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "lengthKindDelimited_04");
}

// --- Implicit complex element (Section 12) ---

#[test]
fn daffodil_implicit_complex_element_terminator() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "nested_seq");
}

#[test]
fn daffodil_implicit_complex_element_terminator_max_one() {
    assert_named_test_passes(daffodil_tdml!("implicit.tdml"), "nested_seq_01");
}

#[test]
fn daffodil_delimited_binary_fail() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "binary_delimited_fail");
}

#[test]
fn daffodil_delimited_terminator_check() {
    assert_named_test_passes(daffodil_tdml!("DelimitedTests.tdml"), "delimsCheck");
}

// --- Prefixed lengthKind (Section 12) ---

#[test]
fn daffodil_prefixed_text_string_bytes() {
    assert_named_test_passes(daffodil_tdml!("PrefixedTests.tdml"), "pl_text_string_txt_bytes");
}

#[test]
fn daffodil_prefixed_text_string_bytes_includes() {
    assert_named_test_passes(
        daffodil_tdml!("PrefixedTests.tdml"),
        "pl_text_string_txt_bytes_includes",
    );
}

#[test]
fn daffodil_prefixed_text_string_bits() {
    assert_named_test_passes(daffodil_tdml!("PrefixedTests.tdml"), "pl_text_string_txt_bits");
}

#[test]
fn daffodil_prefixed_text_string_bits_includes() {
    assert_named_test_passes(
        daffodil_tdml!("PrefixedTests.tdml"),
        "pl_text_string_txt_bits_includes",
    );
}

#[test]
fn daffodil_prefixed_text_string_binary_prefix() {
    assert_named_test_passes(daffodil_tdml!("PrefixedTests.tdml"), "pl_text_string_bin_bytes");
}

#[test]
fn daffodil_prefixed_text_int_bytes() {
    assert_named_test_passes(daffodil_tdml!("PrefixedTests.tdml"), "pl_text_int_txt_bytes");
}

#[test]
fn daffodil_prefixed_text_int_bits() {
    assert_named_test_passes(daffodil_tdml!("PrefixedTests.tdml"), "pl_text_int_txt_bits");
}

#[test]
fn daffodil_prefixed_nested_prefix_length_type() {
    assert_named_test_passes(daffodil_tdml!("PrefixedTests.tdml"), "pl_text_string_pl_txt_bytes");
}

#[test]
fn daffodil_prefixed_extended_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_text_string_txt_chars",
        "pl_text_string_txt_chars_includes",
        "pl_text_string_txt_chars_padding",
        "pl_text_string_bin_bits",
        "pl_text_int_bin_bytes",
        "pl_text_int_bin_bits",
        "pl_text_int_txt_bytes_includes",
        "pl_text_int_txt_bits_includes",
        "pl_text_int_txt_bytes_plbits",
        "pl_text_int_txt_bytes_plchars",
        "pl_text_int_txt_bits_plbytes",
        "pl_text_int_txt_bits_plchars",
        "pl_text_int_txt_chars_plbits",
        "pl_text_int_txt_chars_plbytes",
        "pl_text_int_bin_bytes_plbits",
        "pl_text_int_bin_bytes_plchars",
        "pl_text_int_bin_bits_plbytes",
        "pl_text_int_bin_bits_plchars",
        "pl_text_string_txt_bytes_nil",
        "pl_text_string_txt_bytes_neg_len",
        "pl_text_string_txt_bytes_not_enough_data",
        "pl_text_string_txt_bytes_not_enough_prefix_data",
        "pl_text_bool_txt_bytes",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_bin_representation_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_bin_int_txt_bytes",
        "pl_bin_int_txt_bits",
        "pl_bin_int_bin_bytes",
        "pl_bin_int_bin_bits",
        "pl_bin_int_txt_bytes_includes",
        "pl_bin_int_txt_bits_includes",
        "pl_bin_int_bin_bytes_includes",
        "pl_bin_int_bin_bits_includes",
        "pl_bin_int_bin_bytes_packed",
        "pl_bin_int_bin_bits_packed",
        "pl_bin_int_bin_bytes_bcd",
        "pl_bin_int_bin_bits_bcd",
        "pl_bin_hex_txt_bytes",
        "pl_bin_hex_bin_bytes",
        "pl_bin_bool_txt_bytes",
        "pl_bin_bool_bin_bytes",
        "pl_bin_bool_bin_bits",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_decimal_and_datetime_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_text_dec_txt_bytes",
        "pl_text_dec_txt_bits",
        "pl_text_date_txt_bytes",
        "pl_text_date_txt_bits",
        "pl_text_dec_txt_chars",
        "pl_text_bool_txt_chars",
        "pl_bin_dec_txt_bytes",
        "pl_bin_dec_txt_bits",
        "pl_bin_dec_bin_bytes",
        "pl_bin_dec_bin_bits",
        "pl_bin_dec_bin_bytes_packed",
        "pl_bin_dec_bin_bits_packed",
        "pl_bin_dec_bin_bytes_bcd",
        "pl_bin_dec_bin_bits_bcd",
        "pl_bin_dec_bin_bytes_ibm4690",
        "pl_bin_dec_bin_bits_ibm4690",
        "pl_bin_date_bin_bytes_packed",
        "pl_bin_date_bin_bits_packed",
        "pl_bin_date_bin_bytes_bcd",
        "pl_bin_date_bin_bits_bcd",
        "pl_bin_date_bin_bytes_ibm4690",
        "pl_bin_date_bin_bits_ibm4690",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_complex_content_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_complex_bin_bytes",
        "pl_complex_bin_bits",
        "pl_complex_bin_bytes_suspension",
        "pl_complex_bin_bytes_suspension_includes",
        "plSlash1_data",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_input_value_calc_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_implicit_1",
        "pl_complexContentLengthBytes_1",
        "pl_complexValueLengthBytes_1",
        "pl_complexContentLengthBits_1",
        "pl_complexValueLengthBits_1",
        "pl_simpleContentLengthBytes_1",
        "pl_simpleValueLengthBytes_1",
        "pl_simpleValueLengthBytes_2",
        "pl_simpleValueLengthBytes_3",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_backtrack_suite() {
    assert_named_test_passes(
        daffodil_tdml!("PrefixedTests.tdml"),
        "pl_text_string_txt_bytes_not_enough_prefix_data_includes_backtrack",
    );
}

#[test]
fn daffodil_prefixed_character_units_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_simpleContentLengthCharacters_1",
        "pl_complexContentLengthCharacters_1",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_remaining_types_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_text_string_txt_bits_nil",
        "pl_text_string_txt_chars_nil",
        "pl_text_string_bin_bytes_nil",
        "pl_text_string_bin_bits_nil",
        "pl_text_int_txt_chars",
        "pl_text_int_txt_chars_includes",
        "pl_text_int_bin_bytes_includes",
        "pl_text_int_bin_bits_includes",
        "pl_text_dec_bin_bytes",
        "pl_text_dec_bin_bits",
        "pl_text_date_txt_chars",
        "pl_text_date_bin_bytes",
        "pl_text_date_bin_bits",
        "pl_text_bool_txt_bits",
        "pl_text_bool_bin_bytes",
        "pl_text_bool_bin_bits",
        "pl_bin_hex_txt_bits",
        "pl_bin_hex_bin_bits",
        "pl_bin_bool_txt_bits",
        "pl_bin_int_bin_bytes_ibm4690",
        "pl_bin_int_bin_bits_ibm4690",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_sde_error_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_complex_err",
        "pl_delimited_err",
        "pl_endofparent_err",
        "pl_pattern_err",
        "pl_expression_err",
        "pl_ovc_err",
        "pl_initiator_err",
        "pl_terminator_err",
        "pl_alignment_err",
        "pl_leadingskip_err",
        "pl_trailingskip_err",
        "pl_lengthunits_err",
        "pl_nest_err",
        "pl_decimal_err",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_negative_runtime_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_text_string_txt_bytes_neg_len",
        "pl_text_string_txt_bytes_not_enough_data",
        "pl_text_string_txt_bytes_not_enough_prefix_data",
        "invalidUnsignedIntBitLength",
        "invalidUnsignedShortBitLength",
        "invalidUnsignedByteBitLength",
        "invalidUnsignedLongBitLength",
        "invalidUnsignedLongByteLength",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_bits_nonnegative_integer() {
    assert_named_test_passes(
        daffodil_tdml!("PrefixedTests.tdml"),
        "lengthUnitsBitsForNonNegativeInteger_prefixed",
    );
}

#[test]
fn daffodil_prefixed_signed_invalid_bit_length_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "invalidLongBitLength",
        "invalidIntBitLength",
        "invalidShortBitLength",
        "invalidByteBitLength",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_facet_validation_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_check_prefix_facets_before_use1",
        "pl_check_prefix_facets_before_use2",
        "pl_check_prefix_facets_before_use5",
        "pl_check_prefix_facets_before_use6",
        "pl_check_prefix_facets_before_use7",
        "pl_check_prefix_facets_before_use8",
        "pl_check_prefix_facets_before_use9",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_facet_compile_error_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_check_prefix_for_annotations",
        "pl_complexContentLengthCharacters_utf8_1",
    ] {
        assert_named_test_passes(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_complex_roundtrip_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_complex_bin_bytes",
        "pl_complex_bin_bits",
        "pl_complex_bin_bytes_suspension",
        "pl_complex_bin_bytes_suspension_includes",
        "plSlash1_data",
    ] {
        assert_decode_encode_roundtrip(tdml, name);
    }
}

#[test]
fn daffodil_prefixed_onepass_roundtrip_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    let suite = parse_tdml(tdml).expect("parse tdml");
    let known_encode_gaps = [
        // Variable-width nByteTextInt prefix text width not preserved in infoset
        "pl_text_string_pl_txt_bytes",
        // minLength + center pad + prefixIncludesPrefixLength encode
        "pl_simpleValueLengthBytes_1",
        "pl_simpleValueLengthBytes_3",
        // UTF-16BE character-unit prefix encode (DAFFODIL-2029)
        "pl_complexContentLengthCharacters_1",
        // Choice/backtrack encode
        "pl_text_string_txt_bytes_not_enough_prefix_data_includes_backtrack",
    ];
    for test in &suite.tests {
        if test.expected_errors.is_some() {
            continue;
        }
        if known_encode_gaps.contains(&test.name.as_str()) {
            continue;
        }
        let rt = effective_round_trip(test.round_trip, suite.default_round_trip);
        if rt != RoundTrip::OnePass {
            continue;
        }
        let result = run_parser_test_with_options(&suite, test, true).expect("run test");
        match result.outcome {
            TestOutcome::Pass => {}
            TestOutcome::Fail(msg) => panic!("onePass test `{}` failed: {msg}", test.name),
            TestOutcome::Skip(msg) => panic!("onePass test `{}` skipped: {msg}", test.name),
        }
    }
}

#[test]
fn daffodil_prefixed_twopass_roundtrip_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    let suite = parse_tdml(tdml).expect("parse tdml");
    for test in &suite.tests {
        if test.round_trip != RoundTrip::TwoPass {
            continue;
        }
        let result = run_parser_test_with_options(&suite, test, true).expect("run test");
        match result.outcome {
            TestOutcome::Pass => {}
            TestOutcome::Fail(msg) => panic!("twoPass test `{}` failed: {msg}", test.name),
            TestOutcome::Skip(msg) => panic!("twoPass test `{}` skipped: {msg}", test.name),
        }
    }
}

#[test]
fn daffodil_prefixed_facet_unparser_suite() {
    let tdml = daffodil_tdml!("PrefixedTests.tdml");
    for name in [
        "pl_check_prefix_facets_before_use3",
        "pl_check_prefix_facets_before_use4",
    ] {
        assert_named_unparser_test_passes(tdml, name);
    }
}
