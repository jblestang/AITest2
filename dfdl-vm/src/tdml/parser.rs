use crate::error::{ParseError, Result};
use crate::xml_util::{attrs_to_map, local_name_str, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

/// Parsed TDML test suite.
#[derive(Debug, Clone, PartialEq)]
pub struct TdmlSuite {
    pub name: String,
    pub schemas: BTreeMap<String, TdmlSchema>,
    pub tests: Vec<ParserTestCase>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct TdmlDocument {
    pub kind: DocumentKind,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Text,
    Hex,
}

/// Parse a TDML test suite document.
pub fn parse_tdml(input: &str) -> Result<TdmlSuite> {
    let mut reader = XmlReader::new(input);
    let attrs = reader.expect_start("testSuite")?;
    let name = attrs
        .get("suiteName")
        .cloned()
        .unwrap_or_else(|| "unnamed".into());

    let mut schemas = BTreeMap::new();
    let mut tests = Vec::new();

    reader.for_each_child("testSuite", |local, attrs, r| match local {
        "defineSchema" => {
            let schema = parse_define_schema(attrs, r)?;
            schemas.insert(schema.name.clone(), schema);
            Ok(())
        }
        "parserTestCase" => {
            tests.push(parse_parser_test_case(attrs, r)?);
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;

    Ok(TdmlSuite {
        name,
        schemas,
        tests,
    })
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

fn parse_parser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<ParserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root = attrs.get("root").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "parserTestCase".into(),
        attribute: "root".into(),
    })?;
    let model = attrs.get("model").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "parserTestCase".into(),
        attribute: "model".into(),
    })?;

    let mut documents = Vec::new();
    let mut expected_infoset = String::new();

    reader.for_each_child("parserTestCase", |local, _, r| match local {
        "document" => {
            documents.push(parse_document(r)?);
            Ok(())
        }
        "infoset" => {
            expected_infoset = r.read_inner_xml()?;
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;

    Ok(ParserTestCase {
        name,
        root,
        model,
        documents,
        expected_infoset,
    })
}

fn parse_document(reader: &mut XmlReader<'_>) -> Result<TdmlDocument> {
    reader.skip_insignificant_ws()?;

    if reader.peek_is_end("document")? {
        reader.expect_end("document")?;
        return Ok(TdmlDocument {
            kind: DocumentKind::Text,
            data: Vec::new(),
        });
    }

    if reader.peek_start_local()? == Some("documentPart".to_string()) {
        let mut kind = DocumentKind::Text;
        let mut data = Vec::new();
        while reader.peek_start_local()? == Some("documentPart".to_string()) {
            let part = parse_document_part(reader)?;
            if data.is_empty() {
                kind = part.kind;
            }
            data.extend(part.data);
            reader.skip_insignificant_ws()?;
        }
        reader.expect_end("document")?;
        return Ok(TdmlDocument { kind, data });
    }

    let text = reader.read_text_until_end("document")?;
    Ok(TdmlDocument {
        kind: DocumentKind::Text,
        data: text.into_bytes(),
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
        Some("hex") => DocumentKind::Hex,
        _ => DocumentKind::Text,
    };
    let text = reader.read_text_until_end("documentPart")?;
    let data = match kind {
        DocumentKind::Text => text.into_bytes(),
        DocumentKind::Hex => parse_hex_document(&text)?,
    };
    Ok(TdmlDocument { kind, data })
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
            panic!("schema compile failed: {e}\n---\n{}", schema.xsd);
        }
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
}
