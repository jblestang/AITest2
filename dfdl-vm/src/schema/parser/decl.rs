use super::*;
use crate::error::{ParseError, Result};
use crate::schema::{ChoiceDecl, GroupDecl, GroupRefDecl, Particle, SequenceDecl};
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec::Vec;

impl<'a> XsdParser<'a> {
    pub(crate) fn parse_global_group(&mut self, attrs: BTreeMap<String, String>) -> Result<()> {
        use xml_no_std::reader::XmlEvent;
        let (xsd, _) = split_dfdl_attrs("group", &attrs, None)?;
        let group_name = xsd
            .get("name")
            .cloned()
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "group".into(),
                attribute: "name".into(),
            })?;
        self.reader.skip_insignificant_ws()?;
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::StartElement { name, .. } if name.local_name == "annotation" => {
                    self.reader.skip_element()?;
                }
                XmlEvent::Comment(_) => {
                    let _ = self.reader.next_event()?;
                }
                _ => break,
            }
        }
        if self.reader.peek_is_end("group")? {
            return Err(ParseError::InvalidXml {
                message: "group must contain a sequence".into(),
            }
            .into());
        }
        self.reader.skip_insignificant_ws()?;
        match self.reader.peek()? {
            XmlEvent::StartElement { name, .. } if name.local_name == "sequence" => {
                let child_attrs = self.reader.take_start_attributes()?;
                let seq = self.parse_sequence(child_attrs)?;
                self.expect_end_local("group")?;
                self.doc.groups.insert(group_name, GroupDecl::Sequence(seq));
                Ok(())
            }
            XmlEvent::StartElement { name, .. } if name.local_name == "choice" => {
                let child_attrs = self.reader.take_start_attributes()?;
                let ch = self.parse_choice(child_attrs)?;
                self.expect_end_local("group")?;
                self.doc.groups.insert(group_name, GroupDecl::Choice(ch));
                Ok(())
            }
            _ => Err(ParseError::InvalidXml {
                message: "group must contain xs:sequence".into(),
            }
            .into()),
        }
    }

    pub(crate) fn parse_group_ref_particle(
        &mut self,
        attrs: BTreeMap<String, String>,
    ) -> Result<GroupRefDecl> {
        let (xsd, dfdl_from_attrs) = split_dfdl_attrs("group", &attrs, None)?;
        let ref_name = xsd
            .get("ref")
            .cloned()
            .ok_or_else(|| ParseError::MissingAttribute {
                element: "group".into(),
                attribute: "ref".into(),
            })?;
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_dfdl_props(pending, dfdl_from_attrs));
        self.reader.skip_insignificant_ws()?;
        if !self.reader.peek_is_end("group")? {
            props = self.parse_inline_content(props, &["annotation"])?;
            self.expect_end_local("group")?;
        } else {
            self.expect_end_local("group")?;
        }
        Ok(GroupRefDecl {
            name: normalize_qname(&ref_name),
            props,
        })
    }

    pub(crate) fn parse_sequence(
        &mut self,
        attrs: BTreeMap<String, String>,
    ) -> Result<SequenceDecl> {
        use xml_no_std::reader::XmlEvent;
        let (_xsd, mut dfdl_from_attrs) = split_dfdl_attrs("sequence", &attrs, None)?;
        for (k, v) in &attrs {
            if local_tag(k) == "hiddenGroupRef" {
                dfdl_from_attrs.hidden_group_ref = Some(v.to_string());
                break;
            }
        }
        if xsd_attr(&attrs, "minOccurs").is_some() {
            self.doc.schema_diagnostics.push(
                "Schema Definition Error: minOccurs attribute cannot appear in element xs:sequence".into(),
            );
        }
        if xsd_attr(&attrs, "maxOccurs").is_some() {
            self.doc.schema_diagnostics.push(
                "Schema Definition Error: maxOccurs attribute cannot appear in element xs:sequence".into(),
            );
        }
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_dfdl_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &attrs);

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("sequence")? {
            self.expect_end_local("sequence")?;
            return Ok(SequenceDecl {
                props,
                particles: Vec::new(),
                had_markup_before_particles: false,
            });
        }

        let had_markup_before_particles = matches!(
            self.reader.peek(),
            Ok(XmlEvent::StartElement { name, .. }) if name.local_name == "annotation"
        );
        props = self.parse_inline_content(
            props,
            &["element", "sequence", "choice", "group", "annotation"],
        )?;
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
                    let (child_attrs, child_namespace) = self.reader.take_start_element()?;
                    match local.as_str() {
                        "element" => particles.push(Particle::Element(
                            self.parse_element_decl(child_attrs, child_namespace)?,
                        )),
                        "sequence" => {
                            particles.push(Particle::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "group" => {
                            particles.push(Particle::GroupRef(
                                self.parse_group_ref_particle(child_attrs)?,
                            ));
                        }
                        "choice" => {
                            particles.push(Particle::Choice(self.parse_choice(child_attrs)?))
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
                            "expected sequence child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }

        Ok(SequenceDecl {
            props,
            particles,
            had_markup_before_particles,
        })
    }

    pub(crate) fn parse_choice(&mut self, attrs: BTreeMap<String, String>) -> Result<ChoiceDecl> {
        use xml_no_std::reader::XmlEvent;
        let (_xsd, dfdl_from_attrs) = split_dfdl_attrs("choice", &attrs, None)?;
        if xsd_attr(&attrs, "minOccurs").is_some() {
            self.doc.schema_diagnostics.push(
                "Attribute 'minOccurs' is not allowed to appear in element 'xs:choice'".into(),
            );
        }
        if xsd_attr(&attrs, "maxOccurs").is_some() {
            self.doc.schema_diagnostics.push(
                "Attribute 'maxOccurs' is not allowed to appear in element 'xs:choice'".into(),
            );
        }
        let pending = core::mem::take(&mut self.pending_props);
        let mut props = self.finalize_props(merge_dfdl_props(pending, dfdl_from_attrs));
        merge_occurs(&mut props, &attrs);

        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end("choice")? {
            self.expect_end_local("choice")?;
            return Ok(ChoiceDecl {
                props,
                branches: Vec::new(),
            });
        }

        props = self.parse_inline_content(
            props,
            &["element", "sequence", "choice", "group", "annotation"],
        )?;
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
                    let (child_attrs, child_namespace) = self.reader.take_start_element()?;
                    match local.as_str() {
                        "element" => branches.push(Particle::Element(
                            self.parse_element_decl(child_attrs, child_namespace)?,
                        )),
                        "sequence" => {
                            branches.push(Particle::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "group" => {
                            branches.push(Particle::GroupRef(
                                self.parse_group_ref_particle(child_attrs)?,
                            ));
                        }
                        "choice" => {
                            branches.push(Particle::Choice(self.parse_choice(child_attrs)?))
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
}
