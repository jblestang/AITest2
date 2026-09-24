#[cfg(test)]
mod tests {
    use super::super::*;
    use alloc::vec;

    #[test]
    fn parse_ai_tdml() {
        let tdml = include_str!(
            "../../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AI.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse AI tdml");
        assert!(!suite.schemas.is_empty());
        assert!(suite.tests.iter().any(|t| t.name == "AI000"));
    }

    #[test]
    fn ai_schema_compiles() {
        use crate::schema::parse_schema;
        let tdml = include_str!(
            "../../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AI.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse");
        let schema = suite.schemas.get("AI.dfdl.xsd").expect("schema");
        if let Err(e) = parse_schema(&schema.xsd) {
            panic!("schema compile failed: {e}\n---\n{}", schema.xsd);
        }
    }

    #[test]
    fn parse_byte_document_part_as_hex() {
        let tdml = r##"<tdml:testSuite suiteName="t" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:int"/></xs:schema></tdml:defineSchema>
  <tdml:parserTestCase name="bin" root="A" model="s">
    <tdml:document><tdml:documentPart type="byte">00 00 00 05</tdml:documentPart></tdml:document>
    <tdml:infoset><tdml:dfdlInfoset><A>5</A></tdml:dfdlInfoset></tdml:infoset>
  </tdml:parserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        let test = suite.tests.iter().find(|t| t.name == "bin").unwrap();
        assert_eq!(test.documents[0].data, alloc::vec![0, 0, 0, 5]);
    }

    #[test]
    fn parse_utf16be_document_part() {
        let tdml = r##"<tdml:testSuite suiteName="t" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:string"/></xs:schema></tdml:defineSchema>
  <tdml:parserTestCase name="utf16" root="A" model="s">
    <tdml:document><tdml:documentPart type="text" encoding="utf-16be">AB</tdml:documentPart></tdml:document>
    <tdml:infoset><tdml:dfdlInfoset><A>AB</A></tdml:dfdlInfoset></tdml:infoset>
  </tdml:parserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        let test = suite.tests.iter().find(|t| t.name == "utf16").unwrap();
        assert_eq!(test.documents[0].data, alloc::vec![0x00, b'A', 0x00, b'B']);
    }

    #[test]
    fn parse_explicit_tdml() {
        let tdml = include_str!(
            "../../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/ExplicitTests.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse explicit tdml");
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == "Lesson1_lengthKind_explicit")
            .expect("test");
        assert_eq!(test.documents.len(), 1);
        assert!(test.documents[0].data.starts_with(b"000118"));
    }

    #[test]
    fn parse_round_trip_attributes() {
        let tdml = r##"<tdml:testSuite suiteName="t" defaultRoundTrip="onePass" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:string"/></xs:schema></tdml:defineSchema>
  <tdml:parserTestCase name="two" root="A" model="s" roundTrip="twoPass">
    <tdml:document><tdml:documentPart type="text">x</tdml:documentPart></tdml:document>
    <tdml:infoset><tdml:dfdlInfoset><A>x</A></tdml:dfdlInfoset></tdml:infoset>
  </tdml:parserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        assert_eq!(suite.default_round_trip, RoundTrip::OnePass);
        assert_eq!(suite.tests[0].round_trip, RoundTrip::TwoPass);
    }

    #[test]
    fn parse_unparser_test_case() {
        let tdml = r##"<tdml:testSuite suiteName="t" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData" xmlns:ex="http://example.com">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:string"/></xs:schema></tdml:defineSchema>
  <tdml:unparserTestCase name="enc" root="A" model="s">
    <tdml:infoset><tdml:dfdlInfoset><A>hi</A></tdml:dfdlInfoset></tdml:infoset>
    <tdml:errors><tdml:error>bad</tdml:error></tdml:errors>
  </tdml:unparserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        assert_eq!(suite.unparser_tests.len(), 1);
        assert_eq!(
            suite.unparser_tests[0].expected_errors,
            Some(alloc::vec![alloc::string::String::from("bad")])
        );
    }

    #[test]
    fn parse_negative_test_errors() {
        let tdml = include_str!(
            "../../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse pattern tdml");
        let fail = suite
            .tests
            .iter()
            .find(|t| t.name == "lengthKindPatternFail")
            .expect("negative test");
        assert_eq!(
            fail.expected_errors,
            Some(alloc::vec![String::new(), String::new()])
        );
        let no_match = suite
            .tests
            .iter()
            .find(|t| t.name == "lengthKindPattern_02")
            .expect("no-match test");
        assert_eq!(no_match.expected_errors, Some(alloc::vec![String::new()]));
    }

    #[test]
    fn lsb_document_bits_matches_daffodil_bit_order_change() {
        let part1_chunks = bit_chunks_for_part(
            &collect_bit_digits_from_text("01001|011"),
            DocumentByteOrder::Ltr,
        );
        let part2_chunks = bit_chunks_for_part(
            &collect_bit_digits_from_text("010101|00"),
            DocumentByteOrder::Ltr,
        );
        let (bytes, _) =
            assemble_tdml_document_bytes(&[part1_chunks, part2_chunks], DocumentBitOrder::LsbFirst);
        assert_eq!(bytes, vec![0x4B, 0x54]);
    }

    #[test]
    fn parse_hex_document_ignores_separators_and_odd_nibbles() {
        use alloc::vec;
        let data = parse_hex_document("A5-E9-FF-00").unwrap();
        assert_eq!(data, vec![0xA5, 0xE9, 0xFF, 0x00]);
        let odd = parse_hex_document("12345").unwrap();
        assert_eq!(odd, vec![0x01, 0x23, 0x45]);
    }
}
