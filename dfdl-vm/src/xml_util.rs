//! Shared XML utilities built on [`xml_no_std`] (no_std + alloc).

use crate::error::{ParseError, Result};
use alloc::collections::BTreeMap;
use alloc::string::String;
use xml_no_std::name::OwnedName;
use xml_no_std::reader::{EventReader, ParserConfig, XmlEvent};
use xml_no_std::writer::{EmitterConfig, EventWriter};

/// Pull-based XML reader with peek support and helper methods for DFDL parsing.
pub struct XmlReader<'a> {
    reader: EventReader<'a, core::slice::Iter<'a, u8>>,
    peeked: Option<XmlEvent>,
}

impl<'a> XmlReader<'a> {
    pub fn new(input: &'a str) -> Self {
        let config = ParserConfig::new().cdata_to_characters(true);
        Self {
            reader: EventReader::new_with_config(input.as_bytes().iter(), config),
            peeked: None,
        }
    }

    pub fn next(&mut self) -> Result<XmlEvent> {
        if let Some(ev) = self.peeked.take() {
            return Ok(ev);
        }
        map_xml_err(self.reader.next())
    }

    pub fn peek(&mut self) -> Result<&XmlEvent> {
        if self.peeked.is_none() {
            self.peeked = Some(self.next()?);
        }
        Ok(self.peeked.as_ref().expect("peeked"))
    }

    pub fn skip_insignificant_ws(&mut self) -> Result<()> {
        loop {
            match self.peek()? {
                XmlEvent::Whitespace(_) => {
                    let _ = self.next()?;
                }
                XmlEvent::Characters(s) if s.trim().is_empty() => {
                    let _ = self.next()?;
                }
                _ => break,
            }
        }
        Ok(())
    }

    pub fn peek_start_local(&mut self) -> Result<Option<String>> {
        match self.peek()? {
            XmlEvent::StartElement { name, .. } => Ok(Some(name.local_name.clone())),
            _ => Ok(None),
        }
    }

    pub fn peek_is_end(&mut self, local: &str) -> Result<bool> {
        Ok(matches!(
            self.peek()?,
            XmlEvent::EndElement { name } if name.local_name == local
        ))
    }

    /// Advance until the first `StartElement` with the given local name.
    pub fn expect_start(&mut self, local: &str) -> Result<BTreeMap<String, String>> {
        loop {
            match self.next()? {
                XmlEvent::StartElement { name, attributes, .. } if name.local_name == local => {
                    return Ok(attrs_to_map(&attributes));
                }
                XmlEvent::EndDocument => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("element `{local}` not found"),
                    }
                    .into());
                }
                XmlEvent::StartDocument { .. } | XmlEvent::ProcessingInstruction { .. } => {}
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected <{local}>, found {:?}",
                            event_label(&other)
                        ),
                    }
                    .into());
                }
            }
        }
    }

    /// If the next event is `StartElement` with `local`, consume it and return attributes.
    pub fn take_start_if(&mut self, local: &str) -> Result<Option<BTreeMap<String, String>>> {
        match self.peek()? {
            XmlEvent::StartElement { name, .. } if name.local_name == local => {
                let XmlEvent::StartElement { attributes, .. } = self.next()? else {
                    unreachable!();
                };
                Ok(Some(attrs_to_map(&attributes)))
            }
            _ => Ok(None),
        }
    }

    pub fn expect_end(&mut self, local: &str) -> Result<()> {
        match self.next()? {
            XmlEvent::EndElement { name } if name.local_name == local => Ok(()),
            other => Err(ParseError::InvalidXml {
                message: alloc::format!("expected </{local}>, found {:?}", event_label(&other)),
            }
            .into()),
        }
    }

    /// Must be positioned at `StartElement`; skips the entire element subtree.
    pub fn skip_element(&mut self) -> Result<()> {
        match self.next()? {
            XmlEvent::StartElement { .. } => map_xml_err(self.reader.skip()),
            other => Err(ParseError::InvalidXml {
                message: alloc::format!("expected start element, found {:?}", event_label(&other)),
            }
            .into()),
        }
    }

    /// Skip subtree after the opening tag has already been consumed.
    pub fn skip_current_subtree(&mut self) -> Result<()> {
        let mut depth = 1;
        while depth > 0 {
            match self.next()? {
                XmlEvent::StartElement { .. } => depth += 1,
                XmlEvent::EndElement { .. } => depth -= 1,
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Read character content until the matching end tag at the current depth.
    pub fn read_text_until_end(&mut self, local: &str) -> Result<String> {
        let mut out = String::new();
        loop {
            match self.next()? {
                XmlEvent::EndElement { name } if name.local_name == local => return Ok(out),
                XmlEvent::Characters(s) | XmlEvent::CData(s) => out.push_str(&s),
                XmlEvent::Whitespace(s) => out.push_str(&s),
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "unexpected {:?} while reading `{local}` text",
                            event_label(&other)
                        ),
                    }
                    .into());
                }
            }
        }
    }

    /// After the opening tag has been consumed, serialize inner XML until the matching end tag.
    pub fn read_inner_xml(&mut self) -> Result<String> {
        let config = EmitterConfig::new().write_document_declaration(false);
        let mut writer = EventWriter::new_with_config(config);
        let mut depth = 1;
        while depth > 0 {
            let ev = self.next()?;
            match &ev {
                XmlEvent::StartElement { .. } => {
                    depth += 1;
                    if let Some(we) = ev.as_writer_event() {
                        writer
                            .write(we)
                            .map_err(|e| ParseError::InvalidXml {
                                message: alloc::format!("xml write error: {e}"),
                            })?;
                    }
                }
                XmlEvent::EndElement { .. } => {
                    depth -= 1;
                    if depth > 0 {
                        if let Some(we) = ev.as_writer_event() {
                            writer
                                .write(we)
                                .map_err(|e| ParseError::InvalidXml {
                                    message: alloc::format!("xml write error: {e}"),
                                })?;
                        }
                    }
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                _ => {
                    if let Some(we) = ev.as_writer_event() {
                        writer
                            .write(we)
                            .map_err(|e| ParseError::InvalidXml {
                                message: alloc::format!("xml write error: {e}"),
                            })?;
                    }
                }
            }
        }
        String::from_utf8(writer.into_inner().into_bytes()).map_err(|_| {
            ParseError::InvalidXml {
                message: "invalid utf-8 in inner xml".into(),
            }
            .into()
        })
    }

    /// Iterate children until `EndElement` for `parent_local` is reached.
    pub fn for_each_child<F>(&mut self, parent_local: &str, mut f: F) -> Result<()>
    where
        F: FnMut(&str, BTreeMap<String, String>, &mut Self) -> Result<()>,
    {
        loop {
            match self.peek()? {
                XmlEvent::EndElement { name } if name.local_name == parent_local => {
                    let _ = self.next()?;
                    return Ok(());
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let XmlEvent::StartElement { attributes, .. } = self.next()? else {
                        unreachable!();
                    };
                    f(&local, attrs_to_map(&attributes), self)?;
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.next()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "unexpected {:?} in `{parent_local}`",
                            event_label(other)
                        ),
                    }
                    .into());
                }
            }
        }
    }
}

pub fn local_name_str(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

pub fn owned_local_name(name: &OwnedName) -> &str {
    &name.local_name
}

pub fn attr_key(name: &OwnedName) -> String {
    match &name.prefix {
        Some(prefix) => alloc::format!("{prefix}:{}", name.local_name),
        None => name.local_name.clone(),
    }
}

pub fn attrs_to_map(attrs: &[xml_no_std::attribute::OwnedAttribute]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for attr in attrs {
        map.insert(attr_key(&attr.name), attr.value.clone());
    }
    map
}

fn event_label(ev: &XmlEvent) -> &'static str {
    match ev {
        XmlEvent::StartDocument { .. } => "StartDocument",
        XmlEvent::EndDocument => "EndDocument",
        XmlEvent::ProcessingInstruction { .. } => "ProcessingInstruction",
        XmlEvent::StartElement { .. } => "StartElement",
        XmlEvent::EndElement { .. } => "EndElement",
        XmlEvent::CData(_) => "CData",
        XmlEvent::Comment(_) => "Comment",
        XmlEvent::Characters(_) => "Characters",
        XmlEvent::Whitespace(_) => "Whitespace",
    }
}

fn map_xml_err<T>(res: xml_no_std::reader::Result<T>) -> Result<T> {
    res.map_err(|e| ParseError::InvalidXml {
        message: alloc::format!("{e}"),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tdml_define_schema() {
        let xml = r#"<testSuite xmlns:tdml="http://example.com" xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <tdml:defineSchema name="s1"><xs:element name="a"/></tdml:defineSchema>
        </testSuite>"#;
        let mut reader = XmlReader::new(xml);
        reader.expect_start("testSuite").unwrap();
        reader
            .for_each_child("testSuite", |local, attrs, r| {
                if local == "defineSchema" {
                    assert_eq!(attrs.get("name").map(String::as_str), Some("s1"));
                    let inner = r.read_inner_xml().unwrap();
                    assert!(inner.contains("xs:element"));
                }
                Ok(())
            })
            .unwrap();
    }
}
