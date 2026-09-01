use super::ast::*;
use super::resolver::SchemaResolver;
use crate::error::{ParseError, Result};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const DFDL_NS: &str = "http://www.ogf.org/dfdl/";

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
    input: &'a str,
    pos: usize,
    doc: SchemaDocument,
    pending_props: DfdlProps,
    resolver: SchemaResolver,
    included: bool,
}

impl<'a> XsdParser<'a> {
    fn new(input: &'a str, resolver: SchemaResolver) -> Self {
        Self {
            input,
            pos: 0,
            doc: SchemaDocument::default(),
            pending_props: DfdlProps::default(),
            resolver,
            included: false,
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

    fn parse_document(&mut self) -> Result<SchemaDocument> {
        loop {
            self.skip_ws_and_comments();
            if self.eof() {
                break;
            }
            if !self.try_consume('<') {
                return Err(ParseError::InvalidXml {
                    message: "expected '<'".into(),
                }
                .into());
            }
            if self.try_consume('!') {
                self.skip_declaration()?;
                continue;
            }
            if self.try_consume('?') {
                self.skip_processing_instruction()?;
                continue;
            }
            if self.try_consume('/') {
                return Err(ParseError::InvalidXml {
                    message: "unexpected closing tag at top level".into(),
                }
                .into());
            }

            let name = self.read_name()?;
            if local_tag(&name) == "schema" {
                self.parse_schema_element()?;
            } else {
                self.skip_element(&name)?;
            }
        }
        Ok(core::mem::take(&mut self.doc))
    }

    fn parse_schema_element(&mut self) -> Result<()> {
        let attrs = self.read_attributes()?;
        self.doc.target_namespace = attrs.get("targetNamespace").cloned();
        self.pending_props = DfdlProps::default();

        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(());
        }
        self.expect('>')?;

        loop {
            self.skip_ws_and_comments();
            if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            }
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let end = self.read_name()?;
                    if local_tag(&end) != "schema" {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("expected </schema>, found </{end}>"),
                        }
                        .into());
                    }
                    self.read_attributes()?;
                    self.expect('>')?;
                    break;
                }

                let tag = self.read_name()?;
                match local_tag(&tag) {
                    "element" => self.parse_global_element()?,
                    "complexType" => self.parse_complex_type(None)?,
                    "simpleType" => self.parse_simple_type(None)?,
                    "include" => self.parse_include()?,
                    "format" => {
                        let props = self.parse_dfdl_element(&tag)?;
                        self.doc.format_defaults.props =
                            merge_props(self.doc.format_defaults.props.clone(), props);
                    }
                    "defineFormat" => {
                        let props = self.parse_dfdl_element(&tag)?;
                        self.doc.format_defaults.props =
                            merge_props(self.doc.format_defaults.props.clone(), props);
                    }
                    "annotation" => {
                        self.pos -= tag.len() + 1;
                        let props = self.parse_annotation()?;
                        self.doc.format_defaults.props =
                            merge_props(self.doc.format_defaults.props.clone(), props);
                    }
                    _ => self.skip_element(&tag)?,
                }
            }
        }
        Ok(())
    }

    fn parse_include(&mut self) -> Result<()> {
        let attrs = self.read_attributes()?;
        let location = attrs
            .get("schemaLocation")
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "include".into(),
                attribute: "schemaLocation".into(),
            })?;
        if self.try_consume('/') {
            self.expect('>')?;
        } else {
            self.expect('>')?;
        }
        let content = self.resolver.resolve(location)?;
        let included = parse_schema_with_resolver(&content, self.resolver.clone())?;
        self.merge_included(included);
        Ok(())
    }

    fn parse_global_element(&mut self) -> Result<()> {
        let attrs = self.read_attributes()?;
        let (xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs(&attrs);
        let name = xsd_attrs
            .get("name")
            .cloned()
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "name".into(),
            })?;
        let mut props = merge_props(core::mem::take(&mut self.pending_props), dfdl_from_attrs);
        merge_occurs(&mut props, &xsd_attrs);

        let type_name = if let Some(t) = xsd_attrs.get("type") {
            TypeName::new(normalize_qname(t))
        } else if self.try_consume('/') {
            self.expect('>')?;
            return Err(ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "type".into(),
            }
            .into());
        } else {
            self.expect('>')?;
            props = self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
            let inline = self.parse_inline_type()?;
            props = merge_props(props, inline.1);
            self.expect_close_tag("element")?;
            self.doc.global_elements.insert(
                name.clone(),
                GlobalElement {
                    name,
                    type_name: inline.0,
                    props,
                },
            );
            return Ok(());
        };

        if self.try_consume('/') {
            self.expect('>')?;
        } else {
            self.expect('>')?;
            props = self.parse_inline_content(props, &["annotation"])?;
            self.expect_close_tag("element")?;
        }

        self.doc.global_elements.insert(
            name.clone(),
            GlobalElement {
                name,
                type_name,
                props,
            },
        );
        Ok(())
    }

    fn parse_complex_type(&mut self, inline_name: Option<String>) -> Result<()> {
        let attrs = self.read_attributes()?;
        let name = inline_name.or_else(|| attrs.get("name").cloned());
        let mut props = core::mem::take(&mut self.pending_props);

        if self.try_consume('/') {
            self.expect('>')?;
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
        self.expect('>')?;

        props = self.parse_inline_content(props, &["sequence", "choice", "annotation"])?;
        let content = self.parse_complex_content()?;
        self.expect_close_tag("complexType")?;

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

    fn parse_simple_type(&mut self, inline_name: Option<String>) -> Result<()> {
        let attrs = self.read_attributes()?;
        let name = inline_name.or_else(|| attrs.get("name").cloned());
        let mut props = core::mem::take(&mut self.pending_props);

        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(());
        }
        self.expect('>')?;

        props = self.parse_inline_content(props, &["restriction", "annotation"])?;
        let base = self.parse_restriction()?;
        self.expect_close_tag("simpleType")?;

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
            self.skip_ws_and_comments();
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "complexType" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        return Ok(ComplexContent::Empty);
                    }
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("unexpected closing tag </{tag}>"),
                    }
                    .into());
                }

                let tag = self.read_name()?;
                match local_tag(&tag) {
                    "sequence" => return Ok(ComplexContent::Sequence(self.parse_sequence()?)),
                    "choice" => return Ok(ComplexContent::Choice(self.parse_choice()?)),
                    "annotation" => {
                        self.skip_rest_of_element(local_tag(&tag))?;
                    }
                    _ => self.skip_element(&tag)?,
                }
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            } else {
                return Err(ParseError::InvalidXml {
                    message: "expected complexType child".into(),
                }
                .into());
            }
        }
    }

    fn parse_sequence(&mut self) -> Result<SequenceDecl> {
        let attrs = self.read_attributes()?;
        let (_xsd, dfdl_from_attrs) = split_dfdl_attrs(&attrs);
        let mut props = merge_props(core::mem::take(&mut self.pending_props), dfdl_from_attrs);
        merge_occurs(&mut props, &attrs);

        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(SequenceDecl {
                props,
                particles: Vec::new(),
            });
        }
        self.expect('>')?;

        props = self.parse_inline_content(props, &["element", "sequence", "choice", "annotation"])?;
        let mut particles = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "sequence" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        break;
                    }
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("unexpected closing tag </{tag}>"),
                    }
                    .into());
                }
                let tag = self.read_name()?;
                match local_tag(&tag) {
                    "element" => particles.push(Particle::Element(self.parse_element_decl()?)),
                    "sequence" => particles.push(Particle::Sequence(self.parse_sequence()?)),
                    "choice" => particles.push(Particle::Choice(self.parse_choice()?)),
                    "annotation" => self.skip_rest_of_element(local_tag(&tag))?,
                    _ => self.skip_element(&tag)?,
                }
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            } else {
                return Err(ParseError::InvalidXml {
                    message: "expected sequence child".into(),
                }
                .into());
            }
        }

        Ok(SequenceDecl { props, particles })
    }

    fn parse_choice(&mut self) -> Result<ChoiceDecl> {
        let attrs = self.read_attributes()?;
        let (_xsd, dfdl_from_attrs) = split_dfdl_attrs(&attrs);
        let mut props = merge_props(core::mem::take(&mut self.pending_props), dfdl_from_attrs);
        merge_occurs(&mut props, &attrs);

        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(ChoiceDecl {
                props,
                branches: Vec::new(),
            });
        }
        self.expect('>')?;

        props = self.parse_inline_content(props, &["element", "sequence", "choice", "annotation"])?;
        let mut branches = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "choice" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        break;
                    }
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("unexpected closing tag </{tag}>"),
                    }
                    .into());
                }
                let tag = self.read_name()?;
                match tag.as_str() {
                    "element" => branches.push(Particle::Element(self.parse_element_decl()?)),
                    "sequence" => branches.push(Particle::Sequence(self.parse_sequence()?)),
                    "choice" => branches.push(Particle::Choice(self.parse_choice()?)),
                    "annotation" => self.skip_rest_of_element("annotation")?,
                    _ => self.skip_element(&tag)?,
                }
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            } else {
                return Err(ParseError::InvalidXml {
                    message: "expected choice child".into(),
                }
                .into());
            }
        }

        Ok(ChoiceDecl { props, branches })
    }

    fn parse_element_decl(&mut self) -> Result<ElementDecl> {
        let attrs = self.read_attributes()?;
        let (xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs(&attrs);
        let name = xsd_attrs
            .get("name")
            .cloned()
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "name".into(),
            })?;
        let default_value = xsd_attrs.get("default").cloned();
        let mut props = merge_props(core::mem::take(&mut self.pending_props), dfdl_from_attrs);
        merge_occurs(&mut props, &xsd_attrs);

        let type_name = if let Some(t) = xsd_attrs.get("type") {
            TypeName::new(normalize_qname(t))
        } else if self.try_consume('/') {
            self.expect('>')?;
            return Err(ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "type".into(),
            }
            .into());
        } else {
            self.expect('>')?;
            props = self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
            let inline = self.parse_inline_type()?;
            self.expect_close_tag("element")?;
            return Ok(ElementDecl {
                name,
                type_name: inline.0,
                props: merge_props(props, inline.1),
                particle: None,
                default_value,
            });
        };

        if self.try_consume('/') {
            self.expect('>')?;
        } else {
            self.expect('>')?;
            props = self.parse_inline_content(props, &["annotation"])?;
            self.expect_close_tag("element")?;
        }

        Ok(ElementDecl {
            name,
            type_name,
            props,
            particle: None,
            default_value,
        })
    }

    fn parse_inline_type(&mut self) -> Result<(TypeName, DfdlProps)> {
        self.skip_ws_and_comments();
        if !self.try_consume('<') {
            return Err(ParseError::InvalidXml {
                message: "expected inline type".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        match local_tag(&tag) {
            "complexType" => {
                let name = alloc::format!("__inline_complex_{}", self.pos);
                self.parse_complex_type(Some(name.clone()))?;
                Ok((TypeName::new(name), DfdlProps::default()))
            }
            "simpleType" => {
                let name = alloc::format!("__inline_simple_{}", self.pos);
                self.parse_simple_type(Some(name.clone()))?;
                Ok((TypeName::new(name), DfdlProps::default()))
            }
            _ => Err(ParseError::UnknownElement { name: tag }.into()),
        }
    }

    fn parse_restriction(&mut self) -> Result<SimpleBase> {
        loop {
            self.skip_ws_and_comments();
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let tag = self.read_name()?;
                    if local_tag(&tag) == "simpleType" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        return Err(ParseError::InvalidXml {
                            message: "simpleType missing restriction".into(),
                        }
                        .into());
                    }
                }
                let tag = self.read_name()?;
                if local_tag(&tag) == "restriction" {
                    let attrs = self.read_attributes()?;
                    let base_name = attrs.get("base").cloned().ok_or_else(|| {
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
                    self.skip_rest_of_element("restriction")?;
                    return Ok(SimpleBase::Restriction {
                        base,
                        max_length: None,
                    });
                }
                self.skip_element(&tag)?;
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            }
        }
    }

    fn parse_inline_content(&mut self, mut props: DfdlProps, allowed: &[&str]) -> Result<DfdlProps> {
        loop {
            self.skip_ws_and_comments();
            if !self.try_consume('<') {
                break;
            }
            if self.try_consume('/') {
                // Caller handles the closing tag: rewind to '<'.
                self.pos -= 2;
                break;
            }
            let tag = self.read_name()?;
            if local_tag(&tag) == "annotation" {
                self.pos -= tag.len() + 1;
                props = merge_props(props, self.parse_annotation()?);
            } else if allowed.iter().any(|a| *a == local_tag(&tag)) {
                self.pos -= tag.len() + 1;
                break;
            } else {
                self.skip_element(&tag)?;
            }
        }
        Ok(props)
    }

    fn parse_annotation(&mut self) -> Result<DfdlProps> {
        if !self.try_consume('<') {
            return Err(ParseError::InvalidXml {
                message: "expected <annotation>".into(),
            }
            .into());
        }
        let tag = self.read_name()?;
        if local_tag(&tag) != "annotation" {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("expected <annotation>, found <{tag}>"),
            }
            .into());
        }

        let mut props = DfdlProps::default();
        let attrs = self.read_attributes()?;
        let _ = attrs;
        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(props);
        }
        self.expect('>')?;

        loop {
            self.skip_ws_and_comments();
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let end = self.read_name()?;
                    if local_tag(&end) == "annotation" {
                        self.read_attributes()?;
                        self.expect('>')?;
                        break;
                    }
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("unexpected closing tag </{end}>"),
                    }
                    .into());
                }
                let child = self.read_name()?;
                if local_tag(&child) == "appinfo" {
                    props = merge_props(props, self.parse_appinfo()?);
                } else {
                    self.skip_element(&child)?;
                }
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            } else {
                return Err(ParseError::InvalidXml {
                    message: "expected annotation child".into(),
                }
                .into());
            }
        }
        Ok(props)
    }

    fn parse_appinfo(&mut self) -> Result<DfdlProps> {
        let attrs = self.read_attributes()?;
        let source = attrs.get("source").map(String::as_str);
        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(DfdlProps::default());
        }
        self.expect('>')?;

        let mut props = DfdlProps::default();
        if source == Some(DFDL_NS) || source.is_none() {
            loop {
                self.skip_ws_and_comments();
                if self.try_consume('<') {
                    if self.try_consume('/') {
                        let tag = self.read_name()?;
                        if local_tag(&tag) == "appinfo" {
                            self.read_attributes()?;
                            self.expect('>')?;
                            break;
                        }
                    }
                    let tag = self.read_name()?;
                    if tag.starts_with("dfdl:") || is_dfdl_local(&tag) {
                        let dfdl_props = self.parse_dfdl_element(&tag)?;
                        props = merge_props(props, dfdl_props);
                    } else {
                        self.skip_element(&tag)?;
                    }
                } else if self.eof() {
                    return Err(ParseError::UnexpectedEof.into());
                }
            }
        } else {
            self.skip_rest_of_element("appinfo")?;
        }
        Ok(props)
    }

    fn parse_dfdl_element(&mut self, tag: &str) -> Result<DfdlProps> {
        let local = local_tag(tag);
        if local == "defineFormat" {
            return self.parse_define_format(tag);
        }

        let attrs = self.read_attributes()?;
        let mut props = props_from_attrs(&attrs)?;

        if local == "format" {
            if let Some(ref_name) = attrs.get("ref") {
                let key = normalize_qname(ref_name);
                if let Some(base) = self.doc.named_formats.get(&key).cloned() {
                    props = merge_props(base, props);
                }
            }
            self.doc.format_defaults.props =
                merge_props(self.doc.format_defaults.props.clone(), props.clone());
        }
        if self.try_consume('/') {
            self.expect('>')?;
        } else {
            self.expect('>')?;
            self.skip_rest_of_element(local)?;
        }
        Ok(props)
    }

    fn parse_define_format(&mut self, tag: &str) -> Result<DfdlProps> {
        let attrs = self.read_attributes()?;
        let format_name = attrs.get("name").cloned();
        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(DfdlProps::default());
        }
        self.expect('>')?;

        let mut props = DfdlProps::default();
        loop {
            self.skip_ws_and_comments();
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let end = self.read_name()?;
                    self.read_attributes()?;
                    self.expect('>')?;
                    if local_tag(&end) == "defineFormat" {
                        break;
                    }
                    continue;
                }
                let child = self.read_name()?;
                if child.starts_with("dfdl:") || is_dfdl_local(&child) {
                    let child_props = self.parse_dfdl_element(&child)?;
                    props = merge_props(props, child_props);
                } else {
                    self.skip_element(&child)?;
                }
            } else if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            }
        }

        if let Some(name) = format_name {
            self.doc.named_formats.insert(name, props.clone());
        }
        Ok(props)
    }

    fn skip_element(&mut self, name: &str) -> Result<()> {
        let attrs = self.read_attributes()?;
        let _ = attrs;
        if self.try_consume('/') {
            self.expect('>')?;
            return Ok(());
        }
        self.expect('>')?;
        self.skip_rest_of_element(name)
    }

    fn skip_rest_of_element(&mut self, name: &str) -> Result<()> {
        let expected = local_tag(name);
        loop {
            self.skip_ws_and_comments();
            if self.eof() {
                return Err(ParseError::UnexpectedEof.into());
            }
            if self.try_consume('<') {
                if self.try_consume('/') {
                    let end = self.read_name()?;
                    self.read_attributes()?;
                    self.expect('>')?;
                    if local_tag(&end) == expected {
                        return Ok(());
                    }
                } else {
                    let inner = self.read_name()?;
                    self.skip_element(&inner)?;
                }
            }
        }
    }

    fn expect_close_tag(&mut self, name: &str) -> Result<()> {
        let expected = local_tag(name);
        self.skip_ws_and_comments();
        if !self.try_consume('<') || !self.try_consume('/') {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("expected </{expected}>"),
            }
            .into());
        }
        let end = self.read_name()?;
        if local_tag(&end) != expected {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("expected </{expected}>, found </{end}>"),
            }
            .into());
        }
        self.read_attributes()?;
        self.expect('>')?;
        Ok(())
    }

    fn skip_declaration(&mut self) -> Result<()> {
        let rest = self.remaining();
        let end = rest.find("-->").ok_or(ParseError::UnexpectedEof)?;
        self.pos += end + 3;
        Ok(())
    }

    fn skip_processing_instruction(&mut self) -> Result<()> {
        // Already consumed '<?'
        let rest = self.remaining();
        let end = rest.find("?>").ok_or(ParseError::UnexpectedEof)?;
        self.pos += end + 2;
        Ok(())
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while self.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
                self.pos += 1;
            }
            if self.remaining().starts_with("<!--") {
                let _ = self.skip_declaration();
            } else {
                break;
            }
        }
    }

    fn read_attributes(&mut self) -> Result<BTreeMap<String, String>> {
        let mut attrs = BTreeMap::new();
        loop {
            self.skip_ws_and_comments();
            if self.try_consume('>') || self.try_consume('/') {
                self.pos -= 1;
                break;
            }
            let name = self.read_name()?;
            self.skip_ws_and_comments();
            if !self.try_consume('=') {
                return Err(ParseError::InvalidXml {
                    message: alloc::format!("expected '=' after attribute `{name}`"),
                }
                .into());
            }
            self.skip_ws_and_comments();
            let value = self.read_quoted_value()?;
            attrs.insert(name, value);
        }
        Ok(attrs)
    }

    fn read_quoted_value(&mut self) -> Result<String> {
        let quote = self.peek().ok_or(ParseError::UnexpectedEof)?;
        if quote != '"' && quote != '\'' {
            return Err(ParseError::InvalidXml {
                message: "expected quoted attribute value".into(),
            }
            .into());
        }
        self.pos += 1;
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch == quote {
                let value = self.input[start..self.pos].to_string();
                self.pos += 1;
                return Ok(decode_xml_entities(&value));
            }
            self.pos += 1;
        }
        Err(ParseError::UnexpectedEof.into())
    }

    fn read_name(&mut self) -> Result<String> {
        let start = self.pos;
        let first = self.peek().ok_or(ParseError::UnexpectedEof)?;
        if !is_name_start(first) {
            return Err(ParseError::InvalidXml {
                message: "expected XML name".into(),
            }
            .into());
        }
        self.pos += 1;
        while let Some(ch) = self.peek() {
            if is_name_char(ch) {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn expect(&mut self, ch: char) -> Result<()> {
        if self.try_consume(ch) {
            Ok(())
        } else {
            Err(ParseError::InvalidXml {
                message: alloc::format!("expected '{ch}'"),
            }
            .into())
        }
    }

    fn try_consume(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.pos += 1;
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
    if overlay.occurs_max.is_some() {
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
    base
}

fn split_dfdl_attrs(attrs: &BTreeMap<String, String>) -> (BTreeMap<String, String>, DfdlProps) {
    let mut xsd = BTreeMap::new();
    let mut dfdl_map = BTreeMap::new();
    for (k, v) in attrs {
        let local = local_tag(k);
        if k.starts_with("dfdl:") || is_dfdl_property(local) {
            dfdl_map.insert(local.to_string(), v.clone());
        } else {
            xsd.insert(k.clone(), v.clone());
        }
    }
    let props = props_from_attrs(&dfdl_map).unwrap_or_default();
    (xsd, props)
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
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textTrimKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryNumberRep" => {
                props.binary_number_rep = Some(match value.as_str() {
                    "binary" => BinaryNumberRep::Binary,
                    "bcd" => BinaryNumberRep::Bcd,
                    "packedBCD" => BinaryNumberRep::PackedBcd,
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
            "ref" | "format" => {}
            "choiceDispatchKey" => props.choice_dispatch_key = Some(value.clone()),
            _ => {}
        }
    }
    Ok(props)
}

fn parse_delimiter_literal(raw: &str) -> Result<String> {
    Ok(raw.to_string())
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

fn decode_xml_entities(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn is_name_start(ch: char) -> bool {
    ch == ':' || ch == '_' || ch.is_ascii_alphabetic()
}

fn is_name_char(ch: char) -> bool {
    is_name_start(ch) || ch.is_ascii_digit() || ch == '-' || ch == '.'
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_sample_schema() {
        let doc = parse_schema(SAMPLE).expect("schema should parse");
        assert!(doc.global_elements.contains_key("Record"));
        assert!(doc.types.contains_key(&TypeName::new("RecordType")));
    }
}
