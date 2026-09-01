use crate::error::{ParseError, Result};
use crate::length_validate::DaffodilTunables;
use crate::schema::expand_entities;
use crate::vm::encoding::encode_document_text;
use crate::xml_util::{attrs_to_map, local_name_str, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundTrip {
    /// Use the suite-level `defaultRoundTrip` attribute.
    Inherit,
    Disabled,
    OnePass,
    TwoPass,
}

/// Parsed TDML test suite.
#[derive(Debug, Clone, PartialEq)]
pub struct TdmlSuite {
    pub name: String,
    pub schemas: BTreeMap<String, TdmlSchema>,
    pub configs: BTreeMap<String, DaffodilTunables>,
    pub tests: Vec<ParserTestCase>,
    pub unparser_tests: Vec<UnparserTestCase>,
    pub default_round_trip: RoundTrip,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TdmlSchema {
    pub name: String,
    pub xsd: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParserTestCase {
    pub name: String,
    pub root: String,
    pub model: String,
    pub documents: Vec<TdmlDocument>,
    pub expected_infoset: String,
    /// When set, compile/decode/encode must fail and error text must contain each message.
    pub expected_errors: Option<Vec<String>>,
    pub config: Option<String>,
    pub round_trip: RoundTrip,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnparserTestCase {
    pub name: String,
    pub root: String,
    pub model: String,
    pub infoset: String,
    /// When set, encode must fail and error text must contain each message.
    pub expected_errors: Option<Vec<String>>,
    pub config: Option<String>,
    pub documents: Vec<TdmlDocument>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TdmlDocument {
    pub kind: DocumentKind,
    pub data: Vec<u8>,
    /// Significant bits in the last byte when the document ends mid-byte.
    pub last_byte_bit_count: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Text,
    Hex,
    Bits,
}

/// Parse a TDML test suite document.
pub fn parse_tdml(input: &str) -> Result<TdmlSuite> {
    let mut reader = XmlReader::new(input);
    let attrs = reader.expect_start("testSuite")?;
    let name = attrs
        .get("suiteName")
        .cloned()
        .unwrap_or_else(|| "unnamed".into());
    let default_round_trip = parse_round_trip(attrs.get("defaultRoundTrip").map(String::as_str));

    let mut schemas = BTreeMap::new();
    let mut configs = BTreeMap::new();
    let mut tests = Vec::new();
    let mut unparser_tests = Vec::new();

    reader.for_each_child("testSuite", |local, attrs, r| match local {
        "defineSchema" => {
            let schema = parse_define_schema(attrs, r)?;
            schemas.insert(schema.name.clone(), schema);
            Ok(())
        }
        "defineConfig" => {
            let config = parse_define_config(attrs, r)?;
            configs.insert(config.0.clone(), config.1);
            Ok(())
        }
        "parserTestCase" => {
            tests.push(parse_parser_test_case(attrs, r)?);
            Ok(())
        }
        "unparserTestCase" => {
            unparser_tests.push(parse_unparser_test_case(attrs, r)?);
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;

    Ok(TdmlSuite {
        name,
        schemas,
        configs,
        tests,
        unparser_tests,
        default_round_trip,
    })
}

fn parse_round_trip(value: Option<&str>) -> RoundTrip {
    match value {
        Some("false") | Some("none") => RoundTrip::Disabled,
        Some("onePass") => RoundTrip::OnePass,
        Some("twoPass") => RoundTrip::TwoPass,
        _ => RoundTrip::Inherit,
    }
}

pub fn effective_round_trip(test: RoundTrip, suite_default: RoundTrip) -> RoundTrip {
    if test != RoundTrip::Inherit {
        return test;
    }
    if suite_default != RoundTrip::Inherit {
        return suite_default;
    }
    RoundTrip::Disabled
}

fn parse_define_schema(attrs: BTreeMap<String, String>, reader: &mut XmlReader<'_>) -> Result<TdmlSchema> {
    let name = attrs.get("name").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "defineSchema".into(),
        attribute: "name".into(),
    })?;
    let inner = reader.read_inner_xml()?;
    Ok(TdmlSchema {
        name,
        xsd: wrap_schema(&inner),
    })
}

fn parse_define_config(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<(String, DaffodilTunables)> {
    let name = attrs.get("name").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "defineConfig".into(),
        attribute: "name".into(),
    })?;
    let mut tunables = DaffodilTunables::default();
    reader.for_each_child("defineConfig", |local, _, r| match local {
        "tunables" => {
            r.for_each_child("tunables", |local, _, r| {
                if local == "allowSignedIntegerLength1Bit" {
                    let text = r.read_text_until_end("allowSignedIntegerLength1Bit")?;
                    tunables.allow_signed_integer_length1_bit = text.trim() != "false";
                } else {
                    r.skip_current_subtree()?;
                }
                Ok(())
            })?;
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;
    Ok((name, tunables))
}

fn parse_parser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<ParserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root_from_attr = attrs.get("root").cloned();
    let model = attrs.get("model").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "parserTestCase".into(),
        attribute: "model".into(),
    })?;
    let round_trip = parse_round_trip(attrs.get("roundTrip").map(String::as_str));
    let config = attrs.get("config").cloned();

    let mut documents = Vec::new();
    let mut expected_infoset = String::new();
    let mut expected_errors = None;

    reader.for_each_child("parserTestCase", |local, _, r| match local {
        "document" => {
            documents.push(parse_document(r)?);
            Ok(())
        }
        "infoset" => {
            expected_infoset = r.read_inner_xml()?;
            Ok(())
        }
        "errors" => {
            expected_errors = Some(parse_errors(r)?);
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;

    let root = root_from_attr.unwrap_or_else(|| {
        super::infoset::infer_root_element_name(&expected_infoset).unwrap_or_else(|| name.clone())
    });

    Ok(ParserTestCase {
        name,
        root,
        model,
        documents,
        expected_infoset,
        expected_errors,
        config,
        round_trip,
    })
}

fn parse_unparser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<UnparserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root = attrs.get("root").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "unparserTestCase".into(),
        attribute: "root".into(),
    })?;
    let model = attrs.get("model").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "unparserTestCase".into(),
        attribute: "model".into(),
    })?;
    let config = attrs.get("config").cloned();

    let mut infoset = String::new();
    let mut expected_errors = None;
    let mut documents = Vec::new();

    reader.for_each_child("unparserTestCase", |local, _, r| match local {
        "infoset" => {
            infoset = r.read_inner_xml()?;
            Ok(())
        }
        "document" => {
            documents.push(parse_document(r)?);
            Ok(())
        }
        "errors" => {
            expected_errors = Some(parse_errors(r)?);
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;

    Ok(UnparserTestCase {
        name,
        root,
        model,
        infoset,
        expected_errors,
        config,
        documents,
    })
}

fn parse_document(reader: &mut XmlReader<'_>) -> Result<TdmlDocument> {
    reader.skip_insignificant_ws()?;

    if reader.peek_is_end("document")? {
        reader.expect_end("document")?;
        return Ok(TdmlDocument {
            kind: DocumentKind::Text,
            data: Vec::new(),
            last_byte_bit_count: None,
        });
    }

    if reader.peek_start_local()? == Some("documentPart".to_string()) {
        let mut kind = DocumentKind::Text;
        let mut data = Vec::new();
        let mut last_byte_bit_count = None;
        while reader.peek_start_local()? == Some("documentPart".to_string()) {
            let part = parse_document_part(reader)?;
            if data.is_empty() {
                kind = part.kind;
            }
            data.extend(part.data);
            last_byte_bit_count = part.last_byte_bit_count;
            reader.skip_insignificant_ws()?;
        }
        reader.expect_end("document")?;
        return Ok(TdmlDocument {
            kind,
            data,
            last_byte_bit_count,
        });
    }

    let text = reader.read_text_until_end("document")?;
    Ok(TdmlDocument {
        kind: DocumentKind::Text,
        data: text.into_bytes(),
        last_byte_bit_count: None,
    })
}

fn parse_document_part(reader: &mut XmlReader<'_>) -> Result<TdmlDocument> {
    let XmlEvent::StartElement { attributes, .. } = reader.next_event()? else {
        return Err(ParseError::InvalidXml {
            message: "expected documentPart".into(),
        }
        .into());
    };
    let attrs = attrs_to_map(&attributes);
    let kind = match attrs.get("type").map(String::as_str) {
        Some("hex") | Some("byte") => DocumentKind::Hex,
        Some("bits") => DocumentKind::Bits,
        _ => DocumentKind::Text,
    };
    let replace_entities = attrs
        .get("replaceDFDLEntities")
        .map(|v| v == "true")
        .unwrap_or(false);
    let encoding = attrs.get("encoding").map(String::as_str);
    let text = reader.read_text_until_end("documentPart")?;
    let (data, last_byte_bit_count) = match kind {
        DocumentKind::Text => {
            let data = if replace_entities {
                expand_entities(&text)
            } else if let Some(enc) = encoding {
                encode_document_text(&text, enc).map_err(|e| ParseError::InvalidXml {
                    message: alloc::format!("documentPart encoding: {e}"),
                })?
            } else {
                text.into_bytes()
            };
            (data, None)
        }
        DocumentKind::Hex => (parse_hex_document(&text)?, None),
        DocumentKind::Bits => {
            let (data, bit_count) = parse_bits_document_with_count(&text)?;
            (data, Some(bit_count))
        }
    };
    Ok(TdmlDocument {
        kind,
        data,
        last_byte_bit_count,
    })
}

fn parse_errors(reader: &mut XmlReader<'_>) -> Result<Vec<String>> {
    let mut messages = Vec::new();
    reader.for_each_child("errors", |local, _, r| {
        if local == "error" {
            let text = r.read_text_until_end("error")?;
            messages.push(text.trim().to_string());
        } else {
            r.skip_current_subtree()?;
        }
        Ok(())
    })?;
    Ok(messages)
}

fn wrap_schema(inner: &str) -> String {
    alloc::format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
{inner}
</xs:schema>"#
    )
}

fn parse_hex_document(text: &str) -> Result<Vec<u8>> {
    let hex: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if hex.len() % 2 != 0 {
        return Err(ParseError::InvalidXml {
            message: "invalid hex document".into(),
        }
        .into());
    }
    let mut out = Vec::new();
    for chunk in hex.as_bytes().chunks(2) {
        let hi = (chunk[0] as char).to_digit(16).ok_or_else(|| ParseError::InvalidXml {
            message: "invalid hex document".into(),
        })?;
        let lo = (chunk[1] as char).to_digit(16).ok_or_else(|| ParseError::InvalidXml {
            message: "invalid hex document".into(),
        })?;
        out.push((hi << 4 | lo) as u8);
    }
    Ok(out)
}

fn parse_bits_document_with_count(text: &str) -> Result<(Vec<u8>, u8)> {
    let mut bits = Vec::new();
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        match c {
            '0' => bits.push(0),
            '1' => bits.push(1),
            other => {
                return Err(ParseError::InvalidXml {
                    message: alloc::format!("invalid bits document character `{other}`"),
                }
                .into())
            }
        }
    }
    let trailing = (bits.len() % 8) as u8;
    let mut out = Vec::new();
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (i, bit) in chunk.iter().enumerate() {
            byte |= bit << (7 - i);
        }
        out.push(byte);
    }
    Ok((out, trailing as u8))
}

#[allow(dead_code)]
fn local_tag(name: &str) -> &str {
    local_name_str(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ai_tdml() {
        let tdml = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AI.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse AI tdml");
        assert!(!suite.schemas.is_empty());
        assert!(suite.tests.iter().any(|t| t.name == "AI000"));
    }

    #[test]
    fn ai_schema_compiles() {
        use crate::schema::parse_schema;
        let tdml = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/AI.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse");
        let schema = suite.schemas.get("AI.dfdl.xsd").expect("schema");
        if let Err(e) = parse_schema(&schema.xsd) {
            assert!(false, "schema compile failed: {e}\n---\n{}", schema.xsd);
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
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/ExplicitTests.tdml"
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
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
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
}
