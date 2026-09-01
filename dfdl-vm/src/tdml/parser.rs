use crate::error::{ParseError, Result};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

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
    let mut parser = TdmlParser::new(input);
    parser.parse()
}

struct TdmlParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> TdmlParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn parse(&mut self) -> Result<TdmlSuite> {
        self.skip_to("testSuite")?;
        if !self.try_consume("<") {
            return Err(ParseError::InvalidXml {
                message: "expected <testSuite>".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        if local_tag(&tag) != "testSuite" {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("expected testSuite, found {tag}"),
            }
            .into());
        }
        let attrs = self.read_attributes()?;
        let name = attrs
            .get("suiteName")
            .cloned()
            .unwrap_or_else(|| "unnamed".into());
        self.expect('>')?;

        let mut schemas = BTreeMap::new();
        let mut tests = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.remaining().starts_with("</") {
                let saved = self.pos;
                if self.try_consume("<") && self.try_consume("/") {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "testSuite" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        break;
                    }
                    self.pos = saved;
                }
            }
            if self.try_consume("<") {
                if self.try_consume("/") {
                    let tag = self.read_name()?;
                    self.read_attributes()?;
                    self.expect('>')?;
                    if local_tag(&tag) == "testSuite" {
                        break;
                    }
                    continue;
                }
                if self.try_consume("?") {
                    self.skip_processing_instruction()?;
                    continue;
                }
                if self.try_consume("!") {
                    self.skip_declaration()?;
                    continue;
                }
                let tag = self.read_name()?;
                match local_tag(&tag) {
                    "defineSchema" => {
                        let schema = self.parse_define_schema_body(&tag)?;
                        schemas.insert(schema.name.clone(), schema);
                    }
                    "parserTestCase" => tests.push(self.parse_parser_test_case_body(&tag)?),
                    _ => self.skip_element(&tag)?,
                }
            } else if self.eof() {
                break;
            } else {
                self.pos += 1;
            }
        }

        Ok(TdmlSuite {
            name,
            schemas,
            tests,
        })
    }

    fn parse_define_schema(&mut self) -> Result<TdmlSchema> {
        if !self.try_consume("<") {
            return Err(ParseError::InvalidXml {
                message: "expected <defineSchema>".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        self.parse_define_schema_body(&tag)
    }

    fn parse_define_schema_body(&mut self, _tag: &str) -> Result<TdmlSchema> {
        let attrs = self.read_attributes()?;
        let name = attrs.get("name").cloned().ok_or_else(|| ParseError::MissingAttribute {
            element: "defineSchema".into(),
            attribute: "name".into(),
        })?;
        self.expect('>')?;
        let inner = self.read_until_close("defineSchema")?;
        let xsd = wrap_schema(&inner);
        Ok(TdmlSchema { name, xsd })
    }

    fn parse_parser_test_case(&mut self) -> Result<ParserTestCase> {
        if !self.try_consume("<") {
            return Err(ParseError::InvalidXml {
                message: "expected <parserTestCase>".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        self.parse_parser_test_case_body(&tag)
    }

    fn parse_parser_test_case_body(&mut self, _tag: &str) -> Result<ParserTestCase> {
        let attrs = self.read_attributes()?;
        let name = attrs.get("name").cloned().unwrap_or_default();
        let root = attrs.get("root").cloned().ok_or_else(|| ParseError::MissingAttribute {
            element: "parserTestCase".into(),
            attribute: "root".into(),
        })?;
        let model = attrs.get("model").cloned().ok_or_else(|| ParseError::MissingAttribute {
            element: "parserTestCase".into(),
            attribute: "model".into(),
        })?;
        self.expect('>')?;

        let mut documents = Vec::new();
        let mut expected_infoset = String::new();

        loop {
            self.skip_ws_and_comments();
            if self.try_consume("<") {
                if self.try_consume("/") {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "parserTestCase" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        break;
                    }
                    self.read_attributes()?;
                    self.expect('>')?;
                    continue;
                }
                let tag = self.read_name()?;
                match local_tag(&tag) {
                    "document" => documents.push(self.parse_document_body(&tag)?),
                    "infoset" => expected_infoset = self.parse_infoset_body(&tag)?,
                    _ => self.skip_element(&tag)?,
                }
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            } else {
                self.pos += 1;
            }
        }

        Ok(ParserTestCase {
            name,
            root,
            model,
            documents,
            expected_infoset,
        })
    }

    fn parse_document(&mut self) -> Result<TdmlDocument> {
        if !self.try_consume("<") {
            return Err(ParseError::InvalidXml {
                message: "expected <document>".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        self.parse_document_body(&tag)
    }

    fn parse_document_body(&mut self, _tag: &str) -> Result<TdmlDocument> {
        self.read_attributes()?;
        self.expect('>')?;
        let mut kind = DocumentKind::Text;
        let mut data = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.try_consume("<") {
                if self.try_consume("/") {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "document" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        break;
                    }
                    self.read_attributes()?;
                    self.expect('>')?;
                    continue;
                }
                let tag = self.read_name()?;
                if local_tag(&tag) == "documentPart" {
                    let attrs = self.read_attributes()?;
                    kind = match attrs.get("type").map(|s| s.as_str()) {
                        Some("hex") => DocumentKind::Hex,
                        _ => DocumentKind::Text,
                    };
                    self.expect('>')?;
                    let text = self.read_cdata_or_text()?;
                    data = match kind {
                        DocumentKind::Text => text.into_bytes(),
                        DocumentKind::Hex => parse_hex_document(&text)?,
                    };
                    self.expect_close_tag("documentPart")?;
                } else {
                    self.skip_element(&tag)?;
                }
            } else if self.eof() {
                break;
            }
        }

        Ok(TdmlDocument { kind, data })
    }

    fn parse_infoset(&mut self) -> Result<String> {
        if !self.try_consume("<") {
            return Err(ParseError::InvalidXml {
                message: "expected <infoset>".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        self.parse_infoset_body(&tag)
    }

    fn parse_infoset_body(&mut self, _tag: &str) -> Result<String> {
        self.read_attributes()?;
        self.expect('>')?;
        let content = self.read_until_close("infoset")?;
        Ok(content)
    }

    fn read_cdata_or_text(&mut self) -> Result<String> {
        self.skip_ws_and_comments();
        if self.remaining().starts_with("<![CDATA[") {
            self.pos += "<![CDATA[".len();
            let end = self
                .remaining()
                .find("]]>")
                .ok_or(ParseError::UnexpectedEof)?;
            let text = self.remaining()[..end].to_string();
            self.pos += end + 3;
            return Ok(text);
        }
        let mut out = String::new();
        while !self.eof() {
            if self.remaining().starts_with("</") {
                break;
            }
            out.push(self.read_char()?);
        }
        Ok(out)
    }

    fn read_until_close(&mut self, name: &str) -> Result<String> {
        let mut depth = 1;
        let start = self.pos;
        while !self.eof() {
            if self.try_consume("<") {
                if self.try_consume("/") {
                    let end_start = self.pos - 2;
                    let tag = self.read_name()?;
                    self.read_attributes()?;
                    self.expect('>')?;
                    if local_tag(&tag) == name {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(self.input[start..end_start].trim().to_string());
                        }
                    }
                } else if self.try_consume("!") {
                    self.skip_declaration()?;
                } else {
                    let tag = self.read_name()?;
                    self.read_attributes()?;
                    if self.try_consume("/") {
                        self.expect('>')?;
                    } else {
                        self.expect('>')?;
                        if local_tag(&tag) == name {
                            depth += 1;
                        }
                    }
                }
            } else {
                self.pos += 1;
            }
        }
        Err(ParseError::UnexpectedEof.into())
    }

    fn skip_to(&mut self, name: &str) -> Result<()> {
        loop {
            self.skip_ws_and_comments();
            if self.try_consume("<") {
                if self.try_consume("?") {
                    self.skip_processing_instruction()?;
                    continue;
                }
                if self.try_consume("!") {
                    self.skip_declaration()?;
                    continue;
                }
                let tag = self.read_name()?;
                if local_tag(&tag) == name {
                    self.pos -= tag.len();
                    if self.input.as_bytes().get(self.pos.wrapping_sub(1)) == Some(&b'<') {
                        self.pos -= 1;
                    }
                    return Ok(());
                }
                self.skip_element(&tag)?;
            } else if self.eof() {
                return Err(ParseError::InvalidXml {
                    message: alloc::format!("element `{name}` not found"),
                }
                .into());
            } else {
                self.pos += 1;
            }
        }
    }

    fn skip_element(&mut self, name: &str) -> Result<()> {
        self.read_attributes()?;
        if self.try_consume("/") {
            self.expect('>')?;
            return Ok(());
        }
        self.expect('>')?;
        let mut depth = 1;
        while depth > 0 && !self.eof() {
            if self.try_consume("<") {
                if self.try_consume("/") {
                    let end = self.read_name()?;
                    self.read_attributes()?;
                    self.expect('>')?;
                    if local_tag(&end) == name {
                        depth -= 1;
                    }
                } else if self.try_consume("!") {
                    self.skip_declaration()?;
                } else {
                    let child = self.read_name()?;
                    self.read_attributes()?;
                    if self.try_consume("/") {
                        self.expect('>')?;
                    } else {
                        self.expect('>')?;
                        if !self.try_consume("<") || !self.try_consume("/") {
                            depth += 1;
                            self.pos -= 1;
                        } else {
                            let end = self.read_name()?;
                            self.read_attributes()?;
                            self.expect('>')?;
                            if local_tag(&end) == child {
                                // self-closing handled
                            }
                        }
                    }
                }
            } else {
                self.pos += 1;
            }
        }
        Ok(())
    }

    fn expect_close_tag(&mut self, name: &str) -> Result<()> {
        self.skip_ws_and_comments();
        if !self.try_consume("<") || !self.try_consume("/") {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("expected </{name}>"),
            }
            .into());
        }
        let tag = self.read_name()?;
        if local_tag(&tag) != name {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("expected </{name}>, found </{tag}>"),
            }
            .into());
        }
        self.read_attributes()?;
        self.expect('>')?;
        Ok(())
    }

    fn skip_declaration(&mut self) -> Result<()> {
        while !self.eof() {
            if self.peek() == Some('>') {
                self.pos += 1;
                break;
            }
            self.pos += 1;
        }
        Ok(())
    }

    fn skip_processing_instruction(&mut self) -> Result<()> {
        if let Some(end) = self.remaining().find("?>") {
            self.pos += end + 2;
        } else {
            return Err(ParseError::UnexpectedEof.into());
        }
        Ok(())
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while self.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
                self.pos += 1;
            }
            if self.remaining().starts_with("<!--") {
                if let Some(end) = self.remaining().find("-->") {
                    self.pos += end + 3;
                    continue;
                }
            }
            break;
        }
    }

    fn read_attributes(&mut self) -> Result<BTreeMap<String, String>> {
        let mut attrs = BTreeMap::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == Some('>') || self.peek() == Some('/') {
                break;
            }
            let key = self.read_name()?;
            self.skip_ws_and_comments();
            if self.peek() != Some('=') {
                continue;
            }
            self.pos += 1;
            self.skip_ws_and_comments();
            let quote = self.read_char()?;
            let mut value = String::new();
            while self.peek() != Some(quote) && !self.eof() {
                value.push(self.read_char()?);
            }
            if self.peek() == Some(quote) {
                self.pos += 1;
            }
            attrs.insert(key, value);
        }
        Ok(attrs)
    }

    fn read_name(&mut self) -> Result<String> {
        let start = self.pos;
        while self.peek().map(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ':').unwrap_or(false) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(ParseError::InvalidXml {
                message: "expected name".into(),
            }
            .into());
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn read_char(&mut self) -> Result<char> {
        let c = self.peek().ok_or(ParseError::UnexpectedEof)?;
        self.pos += c.len_utf8();
        Ok(c)
    }

    fn expect(&mut self, ch: char) -> Result<()> {
        if self.peek() == Some(ch) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ParseError::InvalidXml {
                message: alloc::format!("expected '{ch}'"),
            }
            .into())
        }
    }

    fn try_consume(&mut self, s: &str) -> bool {
        if self.remaining().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn remaining(&self) -> &str {
        &self.input[self.pos..]
    }

    fn eof(&self) -> bool {
        self.pos >= self.input.len()
    }
}

fn local_tag(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
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
