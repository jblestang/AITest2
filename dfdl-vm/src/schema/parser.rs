use super::ast::*;
use super::resolver::SchemaResolver;
use crate::error::{ParseError, Result};
use crate::xml_util::{attrs_to_map, local_name_str, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

const DFDL_NS: &str = "http://www.ogf.org/dfdl/";
const DFDL_NS_V1: &str = "http://www.ogf.org/dfdl/dfdl-1.0/";

fn is_dfdl_appinfo_source(source: Option<&str>) -> bool {
    matches!(source, None | Some(DFDL_NS) | Some(DFDL_NS_V1))
}

/// Options controlling XSD parsing and include resolution.
#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    pub base_dir: Option<String>,
}

/// Parse an XSD document with DFDL annotations into a [`SchemaDocument`].
pub fn parse_schema(input: &str) -> Result<SchemaDocument> {
    parse_schema_with_options(input, &ParseOptions::default())
}

/// Parse with include resolution via bundled/general format schemas.
pub fn parse_schema_with_options(input: &str, options: &ParseOptions) -> Result<SchemaDocument> {
    let mut resolver = SchemaResolver::new();
    if let Some(base) = &options.base_dir {
        resolver = resolver.with_base_dir(base.clone());
    }
    parse_schema_with_resolver(input, resolver)
}

/// Parse using a custom [`SchemaResolver`] for `xs:include` / `xs:import`.
pub fn parse_schema_with_resolver(input: &str, resolver: SchemaResolver) -> Result<SchemaDocument> {
    let mut parser = XsdParser::new(input, resolver);
    parser.parse_document()
}

struct XsdParser<'a> {
    reader: XmlReader<'a>,
    inline_counter: usize,
    doc: SchemaDocument,
    pending_props: DfdlProps,
    resolver: SchemaResolver,
    /// True while parsing `dfdl:defineFormat`; nested `dfdl:format` must not alter schema defaults.
    in_define_format: bool,
}

impl<'a> XsdParser<'a> {
    fn new(input: &'a str, resolver: SchemaResolver) -> Self {
        Self {
            reader: XmlReader::new(input),
            inline_counter: 0,
            doc: SchemaDocument::default(),
            pending_props: DfdlProps::default(),
            resolver,
            in_define_format: false,
        }
    }

    fn merge_included(&mut self, other: SchemaDocument) {
        for (k, v) in other.types {
            self.doc.types.insert(k, v);
        }
        for (k, v) in other.global_elements {
            self.doc.global_elements.insert(k, v);
        }
        for (k, v) in other.named_formats {
            self.doc.named_formats.insert(k, v);
        }
        self.doc.format_defaults.props =
            merge_props(self.doc.format_defaults.props.clone(), other.format_defaults.props);
    }

    fn consume_start(&mut self) -> Result<(String, Option<String>, BTreeMap<String, String>)> {
        let XmlEvent::StartElement { name, attributes, .. } = self.reader.next_event()? else {
            return Err(ParseError::InvalidXml {
                message: "expected start element".into(),
            }
            .into());
        };
        Ok((
            name.local_name.clone(),
            name.prefix.clone(),
            attrs_to_map(&attributes),
        ))
    }

    fn expect_end_local(&mut self, local: &str) -> Result<()> {
        self.reader.skip_insignificant_ws()?;
        self.reader.expect_end(local)
    }

    fn skip_element_body(&mut self, local: &str) -> Result<()> {
        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end(local)? {
            self.expect_end_local(local)
        } else {
            self.reader.skip_current_subtree()
        }
    }

    fn is_dfdl_element(prefix: Option<&str>, local: &str) -> bool {
        prefix == Some("dfdl") || is_dfdl_local(local)
    }

    fn parse_document(&mut self) -> Result<SchemaDocument> {
        loop {
            match self.reader.next_event()? {
                XmlEvent::StartElement { name, attributes, .. } => {
                    if local_name_str(&name.local_name) == "schema" {
                        self.parse_schema_element(attrs_to_map(&attributes))?;
                    } else {
                        self.reader.skip_current_subtree()?;
                    }
                }
                XmlEvent::EndDocument => break,
                XmlEvent::StartDocument { .. }
                | XmlEvent::ProcessingInstruction { .. }
                | XmlEvent::Comment(_)
                | XmlEvent::Whitespace(_)
                | XmlEvent::Characters(_) => {}
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "unexpected top-level {:?}",
                            event_kind(&other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(core::mem::take(&mut self.doc))
    }

    fn parse_schema_element(&mut self, attrs: BTreeMap<String, String>) -> Result<()> {
        self.doc.target_namespace = attrs.get("targetNamespace").cloned();
        self.pending_props = DfdlProps::default();

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("schema")? {
            return self.expect_end_local("schema");
        }

        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "schema" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let prefix = name.prefix.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    match local.as_str() {
                        "element" => self.parse_global_element(child_attrs)?,
                        "complexType" => self.parse_complex_type(None, child_attrs)?,
                        "simpleType" => self.parse_simple_type(None, child_attrs)?,
                        "include" => self.parse_include(child_attrs)?,
                        "format" => {
                            let props =
                                self.parse_dfdl_element(&local, prefix.as_deref(), child_attrs)?;
                            self.doc.format_defaults.props =
                                merge_props(self.doc.format_defaults.props.clone(), props);
                        }
                        "defineFormat" => {
                            let _ = self.parse_dfdl_element(&local, prefix.as_deref(), child_attrs)?;
                        }
                        "annotation" => {
                            let props = self.parse_annotation(child_attrs)?;
                            self.doc.format_defaults.props =
                                merge_props(self.doc.format_defaults.props.clone(), props);
                        }
                        _ => self.skip_element_body(&local)?,
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected schema child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(())
    }

    fn parse_include(&mut self, attrs: BTreeMap<String, String>) -> Result<()> {
        let location = attrs
            .get("schemaLocation")
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "include".into(),
                attribute: "schemaLocation".into(),
            })?;
        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("include")? {
            self.expect_end_local("include")?;
        }
        let content = self.resolver.resolve(location)?;
        let included = parse_schema_with_resolver(&content, self.resolver.clone())?;
        self.merge_included(included);
        Ok(())
    }

    fn parse_global_element(&mut self, attrs: BTreeMap<String, String>) -> Result<()> {
        let (xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs("element", &attrs);
        let name = xsd_attrs
            .get("name")
            .cloned()
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "name".into(),
            })?;
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &xsd_attrs);

        let type_name = if let Some(t) = xsd_attrs.get("type") {
            TypeName::new(normalize_qname(t))
        } else {
            self.reader.skip_insignificant_ws()?;
            if self.reader.peek_is_end("element")? {
                self.expect_end_local("element")?;
                return Err(ParseError::MissingAttribute {
                    element: "element".into(),
                    attribute: "type".into(),
                }
                .into());
            }
            props = self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
            let inline = self.parse_inline_type()?;
            props = merge_props(props, inline.1);
            self.expect_end_local("element")?;
            self.doc.global_elements.insert(
                name.clone(),
                GlobalElement {
                    name,
                    type_name: inline.0,
                    props: self.finalize_props(props),
                },
            );
            return Ok(());
        };

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("element")? {
            self.expect_end_local("element")?;
        } else {
            props = self.parse_inline_content(props, &["annotation"])?;
            self.expect_end_local("element")?;
        }

        self.doc.global_elements.insert(
            name.clone(),
            GlobalElement {
                name,
                type_name,
                props: self.finalize_props(props),
            },
        );
        Ok(())
    }

    fn parse_complex_type(
        &mut self,
        inline_name: Option<String>,
        attrs: BTreeMap<String, String>,
    ) -> Result<()> {
        let name = inline_name.or_else(|| attrs.get("name").cloned());
        let mut props = core::mem::take(&mut self.pending_props);

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("complexType")? {
            self.expect_end_local("complexType")?;
            if let Some(type_name) = name {
                self.doc.types.insert(
                    TypeName::new(type_name.clone()),
                    TypeDef::Complex {
                        name: TypeName::new(type_name),
                        content: ComplexContent::Empty,
                        props,
                    },
                );
            }
            return Ok(());
        }

        props = self.parse_inline_content(props, &["sequence", "choice", "annotation"])?;
        let content = self.parse_complex_content()?;
        self.expect_end_local("complexType")?;

        if let Some(type_name) = name {
            self.doc.types.insert(
                TypeName::new(type_name.clone()),
                TypeDef::Complex {
                    name: TypeName::new(type_name),
                    content,
                    props,
                },
            );
        }
        Ok(())
    }

    fn parse_simple_type(
        &mut self,
        inline_name: Option<String>,
        attrs: BTreeMap<String, String>,
    ) -> Result<()> {
        let (_xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs("simpleType", &attrs);
        let name = inline_name.or_else(|| attrs.get("name").cloned());
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_props(pending, dfdl_from_attrs));

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("simpleType")? {
            self.expect_end_local("simpleType")?;
            return Ok(());
        }

        props = self.parse_inline_content(props, &["restriction", "annotation"])?;
        let base = self.parse_restriction()?;
        self.expect_end_local("simpleType")?;

        if let Some(type_name) = name {
            self.doc.types.insert(
                TypeName::new(type_name.clone()),
                TypeDef::Simple {
                    name: TypeName::new(type_name),
                    base,
                    props,
                },
            );
        }
        Ok(())
    }

    fn parse_complex_content(&mut self) -> Result<ComplexContent> {
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "complexType" => {
                    return Ok(ComplexContent::Empty);
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    match local.as_str() {
                        "sequence" => {
                            return Ok(ComplexContent::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "choice" => {
                            return Ok(ComplexContent::Choice(self.parse_choice(child_attrs)?))
                        }
                        "annotation" => self.skip_element_body("annotation")?,
                        _ => self.skip_element_body(&local)?,
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected complexType child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
    }

    fn parse_sequence(&mut self, attrs: BTreeMap<String, String>) -> Result<SequenceDecl> {
        let (_xsd, dfdl_from_attrs) = split_dfdl_attrs("sequence", &attrs);
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &attrs);

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("sequence")? {
            self.expect_end_local("sequence")?;
            return Ok(SequenceDecl {
                props,
                particles: Vec::new(),
            });
        }

        props = self.parse_inline_content(props, &["element", "sequence", "choice", "annotation"])?;
        let mut particles = Vec::new();
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "sequence" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    match local.as_str() {
                        "element" => particles.push(Particle::Element(self.parse_element_decl(child_attrs)?)),
                        "sequence" => {
                            particles.push(Particle::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "choice" => particles.push(Particle::Choice(self.parse_choice(child_attrs)?)),
                        "annotation" => self.skip_element_body("annotation")?,
                        _ => self.skip_element_body(&local)?,
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected sequence child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }

        Ok(SequenceDecl { props, particles })
    }

    fn parse_choice(&mut self, attrs: BTreeMap<String, String>) -> Result<ChoiceDecl> {
        let (_xsd, dfdl_from_attrs) = split_dfdl_attrs("choice", &attrs);
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &attrs);

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("choice")? {
            self.expect_end_local("choice")?;
            return Ok(ChoiceDecl {
                props,
                branches: Vec::new(),
            });
        }

        props = self.parse_inline_content(props, &["element", "sequence", "choice", "annotation"])?;
        let mut branches = Vec::new();
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "choice" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    match local.as_str() {
                        "element" => branches.push(Particle::Element(self.parse_element_decl(child_attrs)?)),
                        "sequence" => {
                            branches.push(Particle::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "choice" => branches.push(Particle::Choice(self.parse_choice(child_attrs)?)),
                        "annotation" => self.skip_element_body("annotation")?,
                        _ => self.skip_element_body(&local)?,
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected choice child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }

        Ok(ChoiceDecl { props, branches })
    }

    fn parse_element_decl(&mut self, attrs: BTreeMap<String, String>) -> Result<ElementDecl> {
        let (xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs("element", &attrs);
        let is_ref = xsd_attrs.contains_key("ref");
        let name = xsd_attrs
            .get("name")
            .cloned()
            .or_else(|| xsd_attrs.get("ref").cloned().map(|r| normalize_qname(&r)))
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "name".into(),
            })?;
        let default_value = xsd_attrs.get("default").cloned();
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &xsd_attrs);

        let type_name = if let Some(t) = xsd_attrs.get("type") {
            TypeName::new(normalize_qname(t))
        } else if is_ref {
            let global = self
                .doc
                .global_elements
                .get(&name)
                .ok_or_else(|| ParseError::UnknownElement {
                    name: name.clone(),
                })?;
            props = merge_props(global.props.clone(), props);
            global.type_name.clone()
        } else {
            self.reader.skip_insignificant_ws()?;
            if self.reader.peek_is_end("element")? {
                self.expect_end_local("element")?;
                return Err(ParseError::MissingAttribute {
                    element: "element".into(),
                    attribute: "type".into(),
                }
                .into());
            }
            props = self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
            let inline = self.parse_inline_type()?;
            self.expect_end_local("element")?;
            return Ok(ElementDecl {
                name,
                type_name: inline.0,
                props: self.finalize_props(merge_props(props, inline.1)),
                particle: None,
                default_value,
            });
        };

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("element")? {
            self.expect_end_local("element")?;
        } else {
            props = self.parse_inline_content(props, &["annotation"])?;
            self.expect_end_local("element")?;
        }

        Ok(ElementDecl {
            name,
            type_name,
            props: self.finalize_props(props),
            particle: None,
            default_value,
        })
    }

    fn parse_inline_type(&mut self) -> Result<(TypeName, DfdlProps)> {
        self.reader.skip_insignificant_ws()?;
        let (local, _, attrs) = self.consume_start()?;
        match local.as_str() {
            "complexType" => {
                let name = alloc::format!("__inline_complex_{}", self.inline_counter);
                self.inline_counter += 1;
                self.parse_complex_type(Some(name.clone()), attrs)?;
                Ok((TypeName::new(name), DfdlProps::default()))
            }
            "simpleType" => {
                let name = alloc::format!("__inline_simple_{}", self.inline_counter);
                self.inline_counter += 1;
                self.parse_simple_type(Some(name.clone()), attrs)?;
                Ok((TypeName::new(name), DfdlProps::default()))
            }
            _ => Err(ParseError::UnknownElement { name: local }.into()),
        }
    }

    fn parse_restriction(&mut self) -> Result<SimpleBase> {
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "simpleType" => {
                    return Err(ParseError::InvalidXml {
                        message: "simpleType missing restriction".into(),
                    }
                    .into());
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    if local == "restriction" {
                        let base_name = child_attrs.get("base").cloned().ok_or_else(|| {
                            ParseError::MissingAttribute {
                                element: "restriction".into(),
                                attribute: "base".into(),
                            }
                        })?;
                        let base = BuiltinType::from_xsd(&normalize_qname(&base_name)).ok_or_else(|| {
                            ParseError::UnknownType {
                                name: base_name.clone(),
                            }
                        })?;
                        self.skip_element_body("restriction")?;
                        return Ok(SimpleBase::Restriction {
                            base,
                            max_length: None,
                        });
                    }
                    self.skip_element_body(&local)?;
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected restriction, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
    }

    fn parse_inline_content(&mut self, mut props: DfdlProps, allowed: &[&str]) -> Result<DfdlProps> {
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { .. } => break,
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    if local == "annotation" {
                        let child_attrs = self.reader.take_start_attributes()?;
                        props = merge_props(props, self.parse_annotation(child_attrs)?);
                    } else if allowed.iter().any(|a| *a == local.as_str()) {
                        break;
                    } else {
                        let _ = self.reader.next_event()?;
                        self.reader.skip_current_subtree()?;
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "unexpected {:?} in inline content",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(props)
    }

    fn parse_annotation(&mut self, attrs: BTreeMap<String, String>) -> Result<DfdlProps> {
        let _ = attrs;
        let mut props = DfdlProps::default();

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("annotation")? {
            self.expect_end_local("annotation")?;
            return Ok(props);
        }

        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "annotation" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    if local == "appinfo" {
                        props = merge_props(props, self.parse_appinfo(child_attrs)?);
                    } else {
                        self.skip_element_body(&local)?;
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected annotation child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(props)
    }

    fn parse_appinfo(&mut self, attrs: BTreeMap<String, String>) -> Result<DfdlProps> {
        let source = attrs.get("source").map(String::as_str);

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("appinfo")? {
            self.expect_end_local("appinfo")?;
            return Ok(DfdlProps::default());
        }

        let mut props = DfdlProps::default();
        if is_dfdl_appinfo_source(source) {
            loop {
                self.reader.skip_insignificant_ws()?;
                match self.reader.peek()? {
                    XmlEvent::EndElement { name } if name.local_name == "appinfo" => {
                        let _ = self.reader.next_event()?;
                        break;
                    }
                    XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                    XmlEvent::StartElement { name, .. } => {
                        let local = name.local_name.clone();
                        let prefix = name.prefix.clone();
                        let child_attrs = self.reader.take_start_attributes()?;
                        if Self::is_dfdl_element(prefix.as_deref(), &local) {
                            let dfdl_props =
                                self.parse_dfdl_element(&local, prefix.as_deref(), child_attrs)?;
                            props = merge_props(props, dfdl_props);
                        } else {
                            self.skip_element_body(&local)?;
                        }
                    }
                    XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                        let _ = self.reader.next_event()?;
                    }
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "expected appinfo child, found {:?}",
                                event_kind(other)
                            ),
                        }
                        .into());
                    }
                }
            }
        } else {
            self.reader.skip_current_subtree()?;
        }
        Ok(props)
    }

    fn finalize_props(&self, mut props: DfdlProps) -> DfdlProps {
        if let Some(ref_name) = props.format_ref.take() {
            let key = format_ref_key(&ref_name);
            if let Some(base) = self.doc.named_formats.get(&key) {
                props = merge_props(base.clone(), props);
            }
        }
        props
    }

    fn parse_dfdl_element(
        &mut self,
        local: &str,
        _prefix: Option<&str>,
        attrs: BTreeMap<String, String>,
    ) -> Result<DfdlProps> {
        if local == "defineFormat" {
            return self.parse_define_format(attrs);
        }

        let mut props = props_from_attrs(&attrs)?;

        if local == "format" {
            if let Some(ref_name) = attrs.get("ref") {
                let key = normalize_qname(ref_name);
                if let Some(base) = self.doc.named_formats.get(&key).cloned() {
                    props = merge_props(base, props);
                }
            }
            if !self.in_define_format {
                self.doc.format_defaults.props =
                    merge_props(self.doc.format_defaults.props.clone(), props.clone());
            }
        }

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end(local)? {
            self.expect_end_local(local)?;
        } else {
            let text = self.read_element_text_content(local)?;
            if !text.is_empty() {
                let mut body = BTreeMap::new();
                body.insert(local.to_string(), text);
                props = merge_props(props, props_from_attrs(&body)?);
            }
        }
        Ok(props)
    }

    fn read_element_text_content(&mut self, end_local: &str) -> Result<String> {
        let mut text = String::new();
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == end_local => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::Characters(s) | XmlEvent::CData(s) => {
                    text.push_str(s);
                    let _ = self.reader.next_event()?;
                }
                XmlEvent::Whitespace(s) => {
                    let ws = s.clone();
                    let _ = self.reader.next_event()?;
                    if !ws.chars().all(char::is_whitespace) {
                        text.push_str(&ws);
                    }
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected text in `{end_local}`, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(text.trim().to_string())
    }

    fn parse_define_format(&mut self, attrs: BTreeMap<String, String>) -> Result<DfdlProps> {
        let format_name = attrs.get("name").cloned();

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("defineFormat")? {
            self.expect_end_local("defineFormat")?;
            return Ok(DfdlProps::default());
        }

        let mut props = DfdlProps::default();
        self.in_define_format = true;
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "defineFormat" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => {
                    self.in_define_format = false;
                    return Err(ParseError::UnexpectedEof.into());
                }
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let prefix = name.prefix.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    if Self::is_dfdl_element(prefix.as_deref(), &local) {
                        let child_props =
                            self.parse_dfdl_element(&local, prefix.as_deref(), child_attrs)?;
                        props = merge_props(props, child_props);
                    } else {
                        self.skip_element_body(&local)?;
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    self.in_define_format = false;
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected defineFormat child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        self.in_define_format = false;

        if let Some(name) = format_name {
            self.doc.named_formats.insert(name, props.clone());
        }
        Ok(props)
    }
}

fn event_kind(ev: &XmlEvent) -> &'static str {
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

fn merge_occurs(props: &mut DfdlProps, attrs: &BTreeMap<String, String>) {
    if let Some(min) = attrs.get("minOccurs") {
        if let Ok(v) = min.parse() {
            props.occurs_min = Some(v);
        }
    }
    if let Some(max) = attrs.get("maxOccurs") {
        props.max_occurs_specified = true;
        if max == "unbounded" {
            props.occurs_max = None;
        } else if let Ok(v) = max.parse() {
            props.occurs_max = Some(v);
        }
    }
}

fn merge_props(mut base: DfdlProps, overlay: DfdlProps) -> DfdlProps {
    if overlay.representation.is_some() {
        base.representation = overlay.representation;
    }
    if overlay.byte_order.is_some() {
        base.byte_order = overlay.byte_order;
    }
    if overlay.bit_order.is_some() {
        base.bit_order = overlay.bit_order;
    }
    if overlay.length_kind.is_some() {
        base.length_kind = overlay.length_kind;
    }
    if overlay.length.is_some() {
        base.length = overlay.length;
    }
    if overlay.length_units.is_some() {
        base.length_units = overlay.length_units;
    }
    if overlay.encoding.is_some() {
        base.encoding = overlay.encoding;
    }
    if overlay.text_trim_kind.is_some() {
        base.text_trim_kind = overlay.text_trim_kind;
    }
    if overlay.binary_number_rep.is_some() {
        base.binary_number_rep = overlay.binary_number_rep;
    }
    if overlay.binary_float_rep.is_some() {
        base.binary_float_rep = overlay.binary_float_rep;
    }
    if overlay.initiator.is_some() {
        base.initiator = overlay.initiator;
    }
    if overlay.terminator.is_some() {
        base.terminator = overlay.terminator;
    }
    if overlay.separator.is_some() {
        base.separator = overlay.separator;
    }
    if overlay.occurs_min.is_some() {
        base.occurs_min = overlay.occurs_min;
    }
    if overlay.max_occurs_specified {
        base.max_occurs_specified = true;
        base.occurs_max = overlay.occurs_max;
    } else if overlay.occurs_max.is_some() {
        base.occurs_max = overlay.occurs_max;
    }
    if overlay.choice_dispatch_key.is_some() {
        base.choice_dispatch_key = overlay.choice_dispatch_key;
    }
    if overlay.length_pattern.is_some() {
        base.length_pattern = overlay.length_pattern;
    }
    if overlay.separator_position.is_some() {
        base.separator_position = overlay.separator_position;
    }
    if overlay.text_boolean_true_rep.is_some() {
        base.text_boolean_true_rep = overlay.text_boolean_true_rep;
    }
    if overlay.text_boolean_false_rep.is_some() {
        base.text_boolean_false_rep = overlay.text_boolean_false_rep;
    }
    if overlay.default_value.is_some() {
        base.default_value = overlay.default_value;
    }
    if overlay.alignment.is_some() {
        base.alignment = overlay.alignment;
    }
    if overlay.leading_skip.is_some() {
        base.leading_skip = overlay.leading_skip;
    }
    if overlay.trailing_skip.is_some() {
        base.trailing_skip = overlay.trailing_skip;
    }
    if overlay.sequence_kind.is_some() {
        base.sequence_kind = overlay.sequence_kind;
    }
    if overlay.fill_byte.is_some() {
        base.fill_byte = overlay.fill_byte;
    }
    if overlay.format_ref.is_some() {
        base.format_ref = overlay.format_ref;
    }
    if overlay.text_number_pad_character.is_some() {
        base.text_number_pad_character = overlay.text_number_pad_character;
    }
    if overlay.prefix_length_type.is_some() {
        base.prefix_length_type = overlay.prefix_length_type;
    }
    if overlay.prefix_includes_prefix_length.is_some() {
        base.prefix_includes_prefix_length = overlay.prefix_includes_prefix_length;
    }
    base
}

fn split_dfdl_attrs(
    element_local: &str,
    attrs: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, DfdlProps) {
    let mut xsd = BTreeMap::new();
    let mut dfdl_map = BTreeMap::new();
    for (k, v) in attrs {
        let local = local_tag(k);
        if k.starts_with("dfdl:")
            || (is_dfdl_property(local) && !is_xsd_local_attr(element_local, local))
        {
            dfdl_map.insert(local.to_string(), v.clone());
        } else {
            xsd.insert(k.clone(), v.clone());
        }
    }
    let props = props_from_attrs(&dfdl_map).unwrap_or_default();
    (xsd, props)
}

fn is_xsd_local_attr(element: &str, attr: &str) -> bool {
    matches!(element, "element" | "attribute" | "group" | "attributeGroup")
        && matches!(
            attr,
            "ref" | "name" | "type" | "minOccurs" | "maxOccurs" | "default" | "fixed" | "form"
                | "substitutionGroup"
        )
}

fn is_dfdl_property(name: &str) -> bool {
    matches!(
        name,
        "representation"
            | "byteOrder"
            | "bitOrder"
            | "lengthKind"
            | "length"
            | "lengthUnits"
            | "lengthPattern"
            | "encoding"
            | "textTrimKind"
            | "textNumberPadCharacter"
            | "binaryNumberRep"
            | "binaryFloatRep"
            | "initiator"
            | "terminator"
            | "separator"
            | "separatorPosition"
            | "textBooleanTrueRep"
            | "textBooleanFalseRep"
            | "alignment"
            | "leadingSkip"
            | "trailingSkip"
            | "sequenceKind"
            | "fillByte"
            | "ref"
            | "format"
            | "prefixLengthType"
            | "prefixIncludesPrefixLength"
    )
}

fn props_from_attrs(attrs: &BTreeMap<String, String>) -> Result<DfdlProps> {
    let mut props = DfdlProps::default();
    for (key, value) in attrs {
        let key = local_tag(key);
        match key {
            "representation" => {
                props.representation = Some(match value.as_str() {
                    "binary" => Representation::Binary,
                    "text" => Representation::Text,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown representation `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "byteOrder" => {
                props.byte_order = Some(match value.as_str() {
                    "bigEndian" => ByteOrder::BigEndian,
                    "littleEndian" => ByteOrder::LittleEndian,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown byteOrder `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "bitOrder" => {
                props.bit_order = Some(match value.as_str() {
                    "mostSignificantBitFirst" => BitOrder::MostSignificantBitFirst,
                    "leastSignificantBitFirst" => BitOrder::LeastSignificantBitFirst,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown bitOrder `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "lengthKind" => {
                props.length_kind = Some(match value.as_str() {
                    "implicit" => LengthKind::Implicit,
                    "explicit" => LengthKind::Explicit,
                    "fixed" => LengthKind::Fixed,
                    "delimited" => LengthKind::Delimited,
                    "prefixed" => LengthKind::Prefixed,
                    "pattern" => LengthKind::Pattern,
                    "endOfParent" => LengthKind::EndOfParent,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown lengthKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "lengthPattern" => props.length_pattern = Some(value.clone()),
            "length" => {
                props.length = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid length `{value}`"),
                })?);
            }
            "lengthUnits" => {
                props.length_units = Some(match value.as_str() {
                    "bytes" => LengthUnits::Bytes,
                    "bits" => LengthUnits::Bits,
                    "characters" => LengthUnits::Characters,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown lengthUnits `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "encoding" => props.encoding = Some(value.clone()),
            "textTrimKind" => {
                props.text_trim_kind = Some(match value.as_str() {
                    "none" => TextTrimKind::None,
                    "trim" => TextTrimKind::Trim,
                    "left" => TextTrimKind::Left,
                    "right" => TextTrimKind::Right,
                    "padChar" => TextTrimKind::PadChar,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textTrimKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberPadCharacter" => {
                props.text_number_pad_character =
                    Some(crate::schema::expand_entities_str(value));
            }
            "binaryNumberRep" => {
                props.binary_number_rep = Some(match value.as_str() {
                    "binary" => BinaryNumberRep::Binary,
                    "bcd" => BinaryNumberRep::Bcd,
                    "packed" | "packedBCD" => BinaryNumberRep::PackedBcd,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binaryNumberRep `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryFloatRep" => {
                props.binary_float_rep = Some(match value.as_str() {
                    "ieee" => BinaryFloatRep::Ieee,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binaryFloatRep `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "initiator" => props.initiator = Some(parse_delimiter_literal(value)?),
            "terminator" => props.terminator = Some(parse_delimiter_literal(value)?),
            "separator" => props.separator = Some(parse_delimiter_literal(value)?),
            "separatorPosition" => {
                props.separator_position = Some(match value.as_str() {
                    "infix" => SeparatorPosition::Infix,
                    "prefix" => SeparatorPosition::Prefix,
                    "postfix" => SeparatorPosition::Postfix,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown separatorPosition `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textBooleanTrueRep" => props.text_boolean_true_rep = Some(value.clone()),
            "textBooleanFalseRep" => props.text_boolean_false_rep = Some(value.clone()),
            "alignment" => {
                props.alignment = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid alignment `{value}`"),
                })?);
            }
            "leadingSkip" => {
                props.leading_skip = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid leadingSkip `{value}`"),
                })?);
            }
            "trailingSkip" => {
                props.trailing_skip = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid trailingSkip `{value}`"),
                })?);
            }
            "sequenceKind" => {
                props.sequence_kind = Some(match value.as_str() {
                    "ordered" => SequenceKind::Ordered,
                    "unordered" => SequenceKind::Unordered,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown sequenceKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "fillByte" => props.fill_byte = Some(parse_byte_literal(value)?),
            "ref" => props.format_ref = Some(format_ref_key(value)),
            "prefixLengthType" => {
                props.prefix_length_type = Some(TypeName::new(normalize_qname(value)));
            }
            "prefixIncludesPrefixLength" => {
                props.prefix_includes_prefix_length = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "unknown prefixIncludesPrefixLength `{other}`"
                            ),
                        }
                        .into())
                    }
                });
            }
            "format" => {}
            "choiceDispatchKey" => props.choice_dispatch_key = Some(value.clone()),
            _ => {}
        }
    }
    Ok(props)
}

fn parse_delimiter_literal(raw: &str) -> Result<String> {
    Ok(crate::schema::expand_entities_str(raw))
}

fn parse_byte_literal(raw: &str) -> Result<Vec<u8>> {
    if let Some(hex) = raw.strip_prefix("0x") {
        let mut out = Vec::new();
        let bytes = hex.as_bytes();
        if bytes.len() % 2 != 0 {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("invalid hex literal `{raw}`"),
            }
            .into());
        }
        for chunk in bytes.chunks(2) {
            let hi = (chunk[0] as char).to_digit(16).ok_or_else(|| ParseError::InvalidXml {
                message: alloc::format!("invalid hex literal `{raw}`"),
            })?;
            let lo = (chunk[1] as char).to_digit(16).ok_or_else(|| ParseError::InvalidXml {
                message: alloc::format!("invalid hex literal `{raw}`"),
            })?;
            out.push((hi << 4 | lo) as u8);
        }
        Ok(out)
    } else {
        Ok(raw.as_bytes().to_vec())
    }
}


fn is_dfdl_local(tag: &str) -> bool {
    matches!(
        tag,
        "format" | "element" | "sequence" | "choice" | "simpleType" | "group"
    )
}

fn local_tag(tag: &str) -> &str {
    strip_prefix(tag)
}

fn strip_prefix(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

fn normalize_qname(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

fn format_ref_key(name: &str) -> String {
    normalize_qname(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ComplexContent, Particle, TypeDef};

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/">
  <xs:element name="Record" type="tns:RecordType">
    <xs:annotation>
      <xs:appinfo source="http://www.ogf.org/dfdl/">
        <dfdl:element representation="binary" byteOrder="bigEndian"/>
      </xs:appinfo>
    </xs:annotation>
  </xs:element>
  <xs:complexType name="RecordType">
    <xs:sequence>
      <xs:element name="id" type="xs:unsignedInt">
        <xs:annotation>
          <xs:appinfo source="http://www.ogf.org/dfdl/">
            <dfdl:element representation="binary" byteOrder="bigEndian" lengthKind="implicit"/>
          </xs:appinfo>
        </xs:annotation>
      </xs:element>
      <xs:element name="flags" type="xs:unsignedByte">
        <xs:annotation>
          <xs:appinfo source="http://www.ogf.org/dfdl/">
            <dfdl:element representation="binary" byteOrder="bigEndian" lengthKind="implicit"/>
          </xs:appinfo>
        </xs:annotation>
      </xs:element>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#;

    #[test]
    fn parse_minimal_schema() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:int"/></xs:schema>"#;
        parse_schema(xsd).expect("minimal");
    }

    #[test]
    fn parse_schema_with_complex_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="Record" type="RecordType"/>
  <xs:complexType name="RecordType">
    <xs:sequence>
      <xs:element name="id" type="xs:unsignedInt"/>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#;
        parse_schema(xsd).expect("complex");
    }

    #[test]
    fn parse_schema_with_annotation() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:dfdl="http://www.ogf.org/dfdl/">
  <xs:element name="Record" type="xs:int">
    <xs:annotation>
      <xs:appinfo source="http://www.ogf.org/dfdl/">
        <dfdl:element representation="binary" byteOrder="bigEndian"/>
      </xs:appinfo>
    </xs:annotation>
  </xs:element>
</xs:schema>"#;
        parse_schema(xsd).expect("annotation");
    }

    #[test]
    fn parse_schema_with_annotated_fields() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:dfdl="http://www.ogf.org/dfdl/">
  <xs:element name="Record" type="RecordType"/>
  <xs:complexType name="RecordType">
    <xs:sequence>
      <xs:element name="id" type="xs:unsignedInt">
        <xs:annotation>
          <xs:appinfo source="http://www.ogf.org/dfdl/">
            <dfdl:element representation="binary" byteOrder="bigEndian" lengthKind="implicit"/>
          </xs:appinfo>
        </xs:annotation>
      </xs:element>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#;
        parse_schema(xsd).expect("annotated fields");
    }

    #[test]
    fn parse_schema_full_record_without_xml_decl() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:dfdl="http://www.ogf.org/dfdl/">
  <xs:element name="Record" type="RecordType">
    <xs:annotation>
      <xs:appinfo source="http://www.ogf.org/dfdl/">
        <dfdl:element representation="binary" byteOrder="bigEndian"/>
      </xs:appinfo>
    </xs:annotation>
  </xs:element>
  <xs:complexType name="RecordType">
    <xs:sequence>
      <xs:element name="id" type="xs:unsignedInt">
        <xs:annotation>
          <xs:appinfo source="http://www.ogf.org/dfdl/">
            <dfdl:element representation="binary" byteOrder="bigEndian" lengthKind="implicit"/>
          </xs:appinfo>
        </xs:annotation>
      </xs:element>
      <xs:element name="flags" type="xs:unsignedByte">
        <xs:annotation>
          <xs:appinfo source="http://www.ogf.org/dfdl/">
            <dfdl:element representation="binary" byteOrder="bigEndian" lengthKind="implicit"/>
          </xs:appinfo>
        </xs:annotation>
      </xs:element>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#;
        parse_schema(xsd).expect("full record");
    }

    #[test]
    fn parse_schema_with_xml_decl() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:int"/></xs:schema>"#;
        parse_schema(xsd).expect("xml decl");
    }

    #[test]
    fn parse_schema_with_tns_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="Record" type="tns:RecordType"/><xs:complexType name="RecordType"><xs:sequence/></xs:complexType></xs:schema>"#;
        parse_schema(xsd).expect("tns type");
    }

    #[test]
    fn define_format_does_not_clobber_schema_format_defaults() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="ex:GeneralFormat" lengthKind="delimited" representation="text"/>
  <dfdl:defineFormat name="trimmed">
    <dfdl:format ref="ex:GeneralFormat" textTrimKind="padChar"/>
  </dfdl:defineFormat>
  <xs:element name="A" type="xs:int"/>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        assert_eq!(
            doc.format_defaults.props.length_kind,
            Some(LengthKind::Delimited),
            "defineFormat inner format must not reset schema format defaults"
        );
        assert!(doc.named_formats.contains_key("trimmed"));
    }

    #[test]
    fn format_delimited_overrides_general_format() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="ex:GeneralFormat" lengthKind="delimited"
    lengthUnits="bytes" encoding="ascii" separator="" initiator=""
    terminator="" occursCountKind="implicit" ignoreCase="no"
    textNumberRep="standard" representation="text" initiatedContent="no" />
  <xs:element name="A" type="xs:int"/>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        assert_eq!(
            doc.format_defaults.props.length_kind,
            Some(LengthKind::Delimited),
            "format lengthKind=delimited should override GeneralFormat implicit default"
        );
    }

    #[test]
    fn element_ref_unbounded_overrides_global_max() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
  <xs:element name="item" type="xs:int"/>
  <xs:element name="wrap">
    <xs:complexType>
      <xs:sequence>
        <xs:element ref="ex:item" maxOccurs="unbounded"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        let wrap = doc.global_elements.get("wrap").expect("wrap");
        if let TypeDef::Complex { content, .. } = doc.resolve_type(&wrap.type_name).unwrap() {
            if let ComplexContent::Sequence(seq) = content {
                if let Particle::Element(el) = &seq.particles[0] {
                    assert!(el.props.max_occurs_specified);
                    assert_eq!(el.props.occurs_max, None);
                } else {
                    panic!("expected element particle");
                }
            } else {
                panic!("expected sequence");
            }
        } else {
            panic!("expected complex type");
        }
    }

    #[test]
    fn parse_sample_schema() {
        let doc = parse_schema(SAMPLE).expect("schema should parse");
        assert!(doc.global_elements.contains_key("Record"));
        assert!(doc.types.contains_key(&TypeName::new("RecordType")));
    }
}
