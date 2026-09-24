use super::*;
use crate::error::{ParseError, Result};
use crate::schema::{ComplexContent, ElementDecl, Particle, SequenceDecl, TypeDef, TypeName};
use alloc::collections::BTreeMap;
use alloc::string::String;

impl<'a> XsdParser<'a> {
    pub(crate) fn parse_global_element(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<()> {
        let suppress = take_daf_suppress_warnings(&attrs);
        let prev_suppress =
            core::mem::replace(&mut self.suppress_schema_definition_warnings, suppress);
        let name_hint = attrs.get("name").cloned();
        let prev_scope = self.warning_scope.clone();
        self.warning_scope = name_hint;
        let parse_result = self.parse_global_element_inner(attrs, namespace);
        self.warning_scope = prev_scope;
        self.suppress_schema_definition_warnings = prev_suppress;
        parse_result
    }

    fn insert_global_element(&mut self, element: GlobalElement) {
        let key = format_storage_key(&element.name, self.doc.target_namespace.as_deref());
        self.doc.global_elements.insert(key, element);
    }

    fn schema_context_snapshot(&self) -> (crate::schema::DfdlProps, Option<String>) {
        (
            self.doc.format_defaults.props.clone(),
            self.doc.schema_source_label.clone(),
        )
    }

    fn parse_global_element_inner(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<()> {
        let type_prefix_map = self.element_type_prefix_map(&namespace);
        let (xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs_with_variables(
            "element",
            &attrs,
            Some(&mut self.doc.schema_diagnostics),
            Some(&self.doc.variables),
            self.doc.schema_source_label.as_deref(),
        )?;
        if xsd_attrs.contains_key("ref") {
            self.reader.skip_insignificant_ws()?;
            self.expect_end_local("element")?;
            return Ok(());
        }
        let name = xsd_attrs
            .get("name")
            .cloned()
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "element".into(),
                attribute: "name".into(),
            })?;
        record_global_element_xsd_diagnostics(&name, &xsd_attrs, &mut self.doc.schema_diagnostics);
        if name.contains(':') {
            self.reader.skip_insignificant_ws()?;
            if self.reader.peek_is_end("element")? {
                self.expect_end_local("element")?;
                return Ok(());
            }
        }
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_dfdl_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &xsd_attrs);
        if !props.max_occurs_specified {
            props.max_occurs_specified = true;
            props.occurs_max = Some(1);
        }
        if xsd_attrs.get("nillable").is_some_and(|v| v == "true") {
            props.nillable = Some(true);
        }
        if let Some(s) = &self.suppress_schema_definition_warnings {
            props.suppress_schema_definition_warnings = Some(s.clone());
        }

        let type_xsd_qname = xsd_attrs.get("type").cloned();
        let type_qname_scope = type_xsd_qname.as_ref().map(|_| type_prefix_map.clone());
        let type_name = if let Some(t) = xsd_attrs.get("type") {
            resolve_type_qname_in_schema(&self.doc, t, Some(&type_prefix_map))
                .unwrap_or_else(|_| TypeName::new(normalize_qname(t)))
        } else {
            self.reader.skip_insignificant_ws()?;
            if self.reader.peek_is_end("element")? {
                self.expect_end_local("element")?;
                let (format_context, source_label) = self.schema_context_snapshot();
                props = self.finalize_props(props);
                self.insert_global_element(GlobalElement {
                    name,
                    type_qname_prefixed: false,
                    type_xsd_qname: None,
                    type_qname_scope: None,
                    type_name: TypeName::new("xs:string"),
                    props,
                    format_context,
                    source_label,
                });
                return Ok(());
            }
            props =
                self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
            if self.reader.peek_is_end("element")? {
                self.expect_end_local("element")?;
                let (format_context, source_label) = self.schema_context_snapshot();
                props = self.finalize_props(props);
                self.insert_global_element(GlobalElement {
                    name,
                    type_qname_prefixed: true,
                    type_xsd_qname: None,
                    type_qname_scope: None,
                    type_name: TypeName::new("xs:string"),
                    props,
                    format_context,
                    source_label,
                });
                return Ok(());
            }
            let inline = self.parse_inline_type()?;
            props = merge_dfdl_props(props, inline.1);
            self.expect_end_local("element")?;
            let (format_context, source_label) = self.schema_context_snapshot();
            props = self.finalize_props(props);
            self.insert_global_element(GlobalElement {
                name,
                type_qname_prefixed: true,
                type_xsd_qname: None,
                type_qname_scope: None,
                type_name: inline.0,
                props,
                format_context,
                source_label,
            });
            return Ok(());
        };

        self.reader.skip_insignificant_ws()?;
        let mut resolved_type = type_name;
        let type_xsd_qname = xsd_attrs.get("type").cloned();
        if self.reader.peek_is_end("element")? {
            self.expect_end_local("element")?;
        } else {
            props = self.parse_inline_content(props, &["annotation", "simpleType"])?;
            self.reader.skip_insignificant_ws()?;
            if !self.reader.peek_is_end("element")? {
                let inline = self.parse_inline_type()?;
                resolved_type = inline.0;
                props = merge_dfdl_props(props, inline.1);
            }
            self.expect_end_local("element")?;
        }

        let type_qname_prefixed = xsd_attrs.get("type").is_some_and(|t| t.contains(':'));
        let (format_context, source_label) = self.schema_context_snapshot();
        props = self.finalize_props(props);
        self.insert_global_element(GlobalElement {
            name,
            type_qname_prefixed,
            type_xsd_qname,
            type_qname_scope,
            type_name: resolved_type,
            props,
            format_context,
            source_label,
        });
        Ok(())
    }

    pub(crate) fn parse_element_decl(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<ElementDecl> {
        let suppress = take_daf_suppress_warnings(&attrs);
        let prev_suppress =
            core::mem::replace(&mut self.suppress_schema_definition_warnings, suppress);
        let result = self.parse_element_decl_inner(attrs, namespace);
        self.suppress_schema_definition_warnings = prev_suppress;
        result
    }

    fn parse_element_decl_inner(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<ElementDecl> {
        let type_prefix_map = self.element_type_prefix_map(&namespace);
        let (xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs_with_variables(
            "element",
            &attrs,
            Some(&mut self.doc.schema_diagnostics),
            Some(&self.doc.variables),
            self.doc.schema_source_label.as_deref(),
        )?;
        record_local_element_xsd_diagnostics(&xsd_attrs, &mut self.doc.schema_diagnostics);
        let is_ref = xsd_attrs.contains_key("ref");
        let has_element_name_attr = xsd_attrs.contains_key("name");
        let element_ref = xsd_attrs.get("ref").cloned();
        let _ = (is_ref, has_element_name_attr);
        let name = match xsd_attrs.get("name").cloned() {
            Some(n) => n,
            None if is_ref => xsd_attrs
                .get("ref")
                .cloned()
                .map(|r| normalize_qname(&r))
                .ok_or_else(|| ParseError::MissingAttribute {
                    element: "element".into(),
                    attribute: "ref".into(),
                })?,
            None => {
                let msg = "Schema Definition Error: Local element declaration must have a 'name' attribute on the element";
                self.doc.schema_diagnostics.push(msg.into());
                self.doc.schema_diagnostics.push("'name'".into());
                self.doc.schema_diagnostics.push("element".into());
                return Err(crate::error::SchemaError::InvalidProperty {
                    message: msg.into(),
                }
                .into());
            }
        };
        let default_value = xsd_attrs.get("default").cloned();
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_dfdl_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &xsd_attrs);
        if !props.max_occurs_specified {
            props.max_occurs_specified = true;
            props.occurs_max = Some(1);
        }
        if xsd_attrs.get("nillable").is_some_and(|v| v == "true") {
            props.nillable = Some(true);
        }
        if let Some(s) = &self.suppress_schema_definition_warnings {
            props.suppress_schema_definition_warnings = Some(s.clone());
        }

        let type_xsd_qname = xsd_attrs.get("type").cloned();
        let type_qname_scope = type_xsd_qname.as_ref().map(|_| type_prefix_map.clone());
        let type_name = if let Some(t) = xsd_attrs.get("type") {
            resolve_type_qname_in_schema(&self.doc, t, Some(&type_prefix_map))
                .unwrap_or_else(|_| TypeName::new(normalize_qname(t)))
        } else if is_ref {
            let ref_qname = element_ref.as_deref().unwrap_or(name.as_str());
            let global = if let Some(g) = get_global_element(&self.doc, ref_qname) {
                g.clone()
            } else if let Some(g) = unique_global_element_by_local(&self.doc, &name) {
                g.clone()
            } else {
                let stub = GlobalElement {
                    name: name.clone(),
                    type_qname_prefixed: false,
                    type_xsd_qname: None,
                    type_qname_scope: None,
                    type_name: TypeName::new("xs:string"),
                    props: crate::schema::DfdlProps::default(),
                    format_context: crate::schema::DfdlProps::default(),
                    source_label: None,
                };
                let key = resolve_global_element_storage_key(&self.doc, ref_qname);
                self.doc.global_elements.insert(key, stub.clone());
                stub
            };
            props = merge_dfdl_props(global.props.clone(), props);
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
            props =
                self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
            let inline = self.parse_inline_type()?;
            self.expect_end_local("element")?;
            let mut props = self.finalize_props(merge_dfdl_props(props, inline.1));
            if let Some(ref d) = default_value {
                if props.default_value.is_none() {
                    props.default_value = Some(d.clone());
                }
            }
            return Ok(ElementDecl {
                name,
                element_ref: None,
                has_element_name_attr,
                type_name: inline.0,
                type_xsd_qname: None,
                type_qname_scope: None,
                props,
                particle: None,
                default_value,
            });
        };

        self.reader.skip_insignificant_ws()?;
        let mut resolved_type = type_name;
        if self.reader.peek_is_end("element")? {
            self.expect_end_local("element")?;
        } else {
            props = self.parse_inline_content(props, &["annotation", "simpleType"])?;
            self.reader.skip_insignificant_ws()?;
            if !self.reader.peek_is_end("element")? {
                let inline = self.parse_inline_type()?;
                resolved_type = inline.0;
                props = merge_dfdl_props(props, inline.1);
            }
            self.expect_end_local("element")?;
        }

        if let Some(ref d) = default_value {
            if props.default_value.is_none() {
                props.default_value = Some(d.clone());
            }
        }
        Ok(ElementDecl {
            name,
            element_ref,
            has_element_name_attr,
            type_name: resolved_type,
            type_xsd_qname,
            type_qname_scope,
            props: self.finalize_props(props),
            particle: None,
            default_value,
        })
    }

    pub(crate) fn parse_complex_type(
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
                let (format_context, source_label) = self.schema_context_snapshot();
                self.doc.types.insert(
                    TypeName::new(type_name.clone()),
                    TypeDef::Complex {
                        name: TypeName::new(type_name),
                        content: ComplexContent::Empty,
                        props,
                        format_context,
                        source_label,
                    },
                );
            }
            return Ok(());
        }

        props = self.parse_inline_content(props, &["sequence", "choice", "group", "annotation"])?;
        let content = self.parse_complex_content()?;
        self.expect_end_local("complexType")?;

        if let Some(type_name) = name {
            let (format_context, source_label) = self.schema_context_snapshot();
            self.doc.types.insert(
                TypeName::new(type_name.clone()),
                TypeDef::Complex {
                    name: TypeName::new(type_name),
                    content,
                    props,
                    format_context,
                    source_label,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn parse_simple_type(
        &mut self,
        inline_name: Option<String>,
        attrs: BTreeMap<String, String>,
    ) -> Result<()> {
        let (_xsd_attrs, dfdl_from_attrs) = split_dfdl_attrs("simpleType", &attrs, None)?;
        let name = inline_name.or_else(|| attrs.get("name").cloned());
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_dfdl_props(pending, dfdl_from_attrs));

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("simpleType")? {
            self.expect_end_local("simpleType")?;
            return Ok(());
        }

        props = self.parse_inline_content(props, &["restriction", "union", "annotation"])?;
        let base = self.parse_simple_base()?;
        self.expect_end_local("simpleType")?;

        if let Some(type_name) = name {
            if props.length.is_none() {
                props.length = self.doc.format_defaults.props.length;
            }
            let (format_context, source_label) = self.schema_context_snapshot();
            self.doc.types.insert(
                TypeName::new(type_name.clone()),
                TypeDef::Simple {
                    name: TypeName::new(type_name),
                    base,
                    props,
                    format_context,
                    source_label,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn parse_complex_content(&mut self) -> Result<ComplexContent> {
        use xml_no_std::reader::XmlEvent;
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
                        "group" => {
                            let gr = self.parse_group_ref_particle(child_attrs)?;
                            return Ok(ComplexContent::Sequence(SequenceDecl {
                                props: crate::schema::DfdlProps::default(),
                                particles: alloc::vec![Particle::GroupRef(gr)],
                                had_markup_before_particles: false,
                            }));
                        }
                        "annotation" => self.skip_element_body("annotation")?,
                        _ => {
                            self.doc.schema_diagnostics.push(alloc::format!(
                                "Schema Definition Error: unrecognized element `{local}`"
                            ));
                            self.doc.schema_diagnostics.push(local.clone());
                            self.skip_element_body(&local)?;
                            return Ok(ComplexContent::Empty);
                        }
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
}
