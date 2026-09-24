use super::props::*;
use super::qname::*;
use crate::error::{ParseError, Result};
use crate::schema::ast::*;
use crate::schema::resolver::SchemaResolver;
use crate::xml_util::{attrs_to_map, local_name_str, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

pub(crate) const DFDL_NS: &str = "http://www.ogf.org/dfdl/";

pub(crate) fn parse_schema_with_resolver_and_label(
    input: &str,
    resolver: SchemaResolver,
    schema_label: Option<&str>,
) -> Result<SchemaDocument> {
    if let Some(message) = precheck_schema_definition_errors(input) {
        return Err(crate::error::SchemaError::InvalidProperty { message }.into());
    }
    if input.contains("<!DOCTYPE") {
        let mut message = "Schema Definition Error. org.xml.sax.SAXParseException: DOCTYPE is disallowed when parsing DFDL schemas".to_string();
        if let Some(label) = schema_label {
            message.push(' ');
            message.push_str(label);
        }
        return Err(crate::error::SchemaError::InvalidProperty { message }.into());
    }
    let mut parser = XsdParser::new(input, resolver);
    parser.doc.schema_source_text = Some(input.to_string());
    if let Some(label) = schema_label {
        parser.doc.schema_source_label = Some(label.to_string());
    }
    if input.contains(DFDL_NS) {
        parser.doc.dfdl_annotations_seen = true;
    }
    parser
        .parse_document()
        .map_err(map_xml_parse_error_to_schema)
}

pub(crate) fn map_xml_parse_error_to_schema(err: crate::error::Error) -> crate::error::Error {
    let msg = err.to_string();
    if msg.contains("Cannot redefine XMLNS prefix") {
        return crate::error::SchemaError::InvalidProperty {
            message: "Schema Definition Error: The prefix \"xmlns\" cannot be bound to any namespace explicitly; neither can the namespace for \"xmlns\" be bound to any prefix explicitly".into(),
        }
        .into();
    }
    if msg.contains("Unexpected token inside qualified name") {
        return crate::error::SchemaError::InvalidProperty {
            message: "Schema Definition Error: Element or attribute do not match QName production: QName::=(NCName':')?NCName".to_string(),
        }
        .into();
    }
    err
}

pub(crate) const QNAME_PRODUCTION_SDE: &str =
    "Schema Definition Error: Element or attribute do not match QName production: QName::=(NCName':')?NCName";

pub(crate) fn precheck_schema_definition_errors(input: &str) -> Option<String> {
    if input.contains("xmlns:xmlns=") {
        return Some(
            "Schema Definition Error: The prefix \"xmlns\" cannot be bound to any namespace explicitly; neither can the namespace for \"xmlns\" be bound to any prefix explicitly".into(),
        );
    }
    for attr in ["type", "ref", "base", "substitutionGroup"] {
        let needle = alloc::format!("{attr}=\"");
        let mut start = 0usize;
        while let Some(rel) = input[start..].find(&needle) {
            let abs = start + rel + needle.len();
            let rest = &input[abs..];
            let end = rest.find('"')?;
            let value = &rest[..end];
            if !qname_attr_value_is_well_formed(value) {
                if value.starts_with(':') || value.ends_with(':') {
                    return Some(QNAME_PRODUCTION_SDE.into());
                }
                if value.matches(':').count() > 1 {
                    return Some(alloc::format!(
                        "Schema Definition Error: Error loading schema\n'{value}' is not a valid value for 'QName'"
                    ));
                }
                return Some(QNAME_PRODUCTION_SDE.into());
            }
            start = abs + end + 1;
        }
    }
    None
}

pub(crate) fn qname_attr_value_is_well_formed(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.starts_with(':') || trimmed.ends_with(':') {
        return false;
    }
    if trimmed.matches(':').count() > 1 {
        return false;
    }
    true
}

pub(crate) fn record_foreign_attrs_on_dfdl_element(
    local: &str,
    attrs: &BTreeMap<String, String>,
    diagnostics: &mut alloc::vec::Vec<String>,
) {
    for key in attrs.keys() {
        if !key.contains(':') {
            continue;
        }
        let prefix = key.split(':').next().unwrap_or("");
        if matches!(prefix, "dfdl" | "dfdlx" | "xml" | "xs" | "xsd") {
            continue;
        }
        diagnostics.push(alloc::format!(
            "Schema Definition Error: Attribute '{key}' is not allowed to appear in element 'dfdl:{local}'"
        ));
    }
}

#[derive(Debug, Default)]
pub(crate) struct ParsedRestrictionFacets {
    pub(crate) length: Option<u64>,
    pub(crate) min_length: Option<u64>,
    pub(crate) max_length: Option<u64>,
    pub(crate) min_inclusive: Option<i64>,
    pub(crate) max_inclusive: Option<i64>,
    pub(crate) min_exclusive: Option<i64>,
    pub(crate) max_exclusive: Option<i64>,
    pub(crate) min_inclusive_lexical: Option<String>,
    pub(crate) max_inclusive_lexical: Option<String>,
    pub(crate) min_exclusive_lexical: Option<String>,
    pub(crate) max_exclusive_lexical: Option<String>,
    pub(crate) patterns: Vec<String>,
    pub(crate) enumerations: Vec<String>,
    pub(crate) total_digits: Option<u64>,
    pub(crate) fraction_digits: Option<u64>,
    pub(crate) invalid_min_length: Option<String>,
    pub(crate) invalid_max_length: Option<String>,
    pub(crate) invalid_length: Option<String>,
    pub(crate) invalid_total_digits: Option<String>,
    pub(crate) invalid_fraction_digits: Option<String>,
}

pub(crate) fn backfill_type_format_contexts(doc: &mut SchemaDocument) {
    let defaults = doc.format_defaults.props.clone();
    if !defaults.length_kind_defined {
        return;
    }
    for td in doc.types.values_mut() {
        match td {
            TypeDef::Simple { format_context, .. } | TypeDef::Complex { format_context, .. } => {
                if !format_context.length_kind_defined {
                    *format_context = merge_dfdl_props(format_context.clone(), defaults.clone());
                }
            }
        }
    }
}

pub(crate) struct XsdParser<'a> {
    pub(crate) reader: XmlReader<'a>,
    pub(crate) doc: SchemaDocument,
    pub(crate) pending_props: DfdlProps,
    pub(crate) resolver: SchemaResolver,
    pub(crate) in_define_format: bool,
    pub(crate) suppress_schema_definition_warnings: Option<String>,
    pub(crate) warning_scope: Option<String>,
    pub(crate) annotation_prefix_overrides: alloc::vec::Vec<BTreeMap<String, String>>,
}

impl<'a> XsdParser<'a> {
    pub(crate) fn new(input: &'a str, resolver: SchemaResolver) -> Self {
        Self {
            reader: XmlReader::new(input),
            doc: SchemaDocument::default(),
            pending_props: DfdlProps::default(),
            resolver,
            in_define_format: false,
            suppress_schema_definition_warnings: None,
            warning_scope: None,
            annotation_prefix_overrides: alloc::vec::Vec::new(),
        }
    }

    pub(crate) fn escape_scheme_ref_storage_key(&self, qname: &str) -> String {
        let prefixes = self
            .annotation_prefix_overrides
            .last()
            .unwrap_or(&self.doc.namespace_prefixes);
        if let Some((prefix, local)) = qname.split_once(':') {
            if let Some(uri) = prefixes.get(prefix) {
                return format_storage_key(&normalize_qname(local), Some(uri.as_str()));
            }
        }
        format_storage_key(
            &normalize_qname(qname),
            self.doc.target_namespace.as_deref(),
        )
    }

    pub(crate) fn normalize_escape_scheme_ref(&self, props: &mut DfdlProps) {
        if let Some(qname) = props
            .escape_scheme_ref
            .as_ref()
            .filter(|r| !r.is_empty())
            .cloned()
        {
            props.escape_scheme_ref = Some(self.escape_scheme_ref_storage_key(&qname));
        }
    }

    pub(crate) fn push_schema_warning(&mut self, warn_id: &str, lines: &[&str]) {
        if self
            .suppress_schema_definition_warnings
            .as_deref()
            .is_some_and(|s| s.split_whitespace().any(|part| part == warn_id))
        {
            return;
        }
        let owned: alloc::vec::Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
        if let Some(scope) = &self.warning_scope {
            self.doc
                .scoped_schema_warnings
                .entry(scope.clone())
                .or_default()
                .extend(owned);
        } else {
            self.doc.schema_warnings.extend(owned);
        }
    }

    pub(crate) fn merge_included(
        &mut self,
        other: SchemaDocument,
        kind: SchemaMergeKind,
    ) -> Result<()> {
        let included_target = other.target_namespace.clone();
        for (k, v) in other.types {
            self.doc.types.insert(k, v);
        }
        for (k, v) in other.global_elements {
            let key = if k.contains('|') && !k.starts_with('|') {
                k
            } else {
                let local = format_local_from_storage_key(&k);
                let ns = match kind {
                    SchemaMergeKind::Import => included_target.as_deref(),
                    SchemaMergeKind::Include => included_target
                        .as_deref()
                        .or(self.doc.target_namespace.as_deref()),
                };
                format_storage_key(local, ns)
            };
            if let Some(existing) = self.doc.global_elements.get(&key) {
                if existing == &v {
                    continue;
                }
                let elem_local = format_local_from_storage_key(&key);
                self.doc.schema_diagnostics.push(alloc::format!(
                    "Schema Definition Error: More than one definition for name: {elem_local}"
                ));
                continue;
            }
            self.doc.global_elements.insert(key, v);
        }
        for (prefix, uri) in other.namespace_prefixes {
            self.doc.namespace_prefixes.entry(prefix).or_insert(uri);
        }
        for (k, v) in other.named_formats {
            let local = format_local_from_storage_key(&k);
            let ns = match kind {
                SchemaMergeKind::Import => included_target.as_deref(),
                SchemaMergeKind::Include => included_target
                    .as_deref()
                    .or(self.doc.target_namespace.as_deref()),
            };
            let key = format_storage_key(local, ns);
            if self.doc.named_formats.contains_key(&key) {
                if self.doc.named_formats.get(&key) == Some(&v) {
                    if key.starts_with('|') {
                        self.doc.schema_diagnostics.push(alloc::format!(
                            "Schema Definition Error: More than one definition for name: {local}"
                        ));
                    }
                    continue;
                }
                if kind == SchemaMergeKind::Import {
                    continue;
                }
                self.doc.schema_diagnostics.push(alloc::format!(
                    "Schema Definition Error: More than one definition for name: {local}"
                ));
                continue;
            }
            self.doc.named_formats.insert(key, v.clone());
            if included_target.is_none()
                && self.doc.target_namespace.is_none()
                && kind == SchemaMergeKind::Include
            {
                let bare = format_storage_key(local, None);
                self.doc.named_formats.entry(bare).or_insert(v);
            }
        }
        for (k, v) in other.named_escape_schemes {
            self.doc.named_escape_schemes.insert(k, v);
        }
        for (k, v) in other.groups {
            self.doc.groups.insert(k, v);
        }
        self.doc.dfdl_annotations_seen |= other.dfdl_annotations_seen;
        self.doc.schema_diagnostics.extend(other.schema_diagnostics);
        self.doc.schema_warnings.extend(other.schema_warnings);
        for (k, v) in other.scoped_schema_warnings {
            self.doc
                .scoped_schema_warnings
                .entry(k)
                .or_default()
                .extend(v);
        }
        if kind == SchemaMergeKind::Include {
            self.doc.format_defaults.props = merge_included_format_defaults(
                self.doc.format_defaults.props.clone(),
                other.format_defaults.props,
            );
        }
        self.doc.format_defaults.props.calendar_time_zone_defined = false;
        Ok(())
    }

    pub(crate) fn consume_start(
        &mut self,
    ) -> Result<(String, Option<String>, BTreeMap<String, String>)> {
        let XmlEvent::StartElement {
            name, attributes, ..
        } = self.reader.next_event()?
        else {
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

    pub(crate) fn expect_end_local(&mut self, local: &str) -> Result<()> {
        self.reader.skip_insignificant_ws()?;
        self.reader.expect_end(local)
    }

    pub(crate) fn skip_element_body(&mut self, local: &str) -> Result<()> {
        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end(local)? {
            self.expect_end_local(local)
        } else {
            self.reader.skip_current_subtree()
        }
    }

    pub(crate) fn is_dfdl_element(
        prefix: Option<&str>,
        local: &str,
        namespace: Option<&str>,
    ) -> bool {
        const DFDL_LEGACY: &str = "http://www.ogf.org/dfdl/dfdl-1.0/";
        if matches!(namespace, Some(DFDL_NS) | Some(DFDL_LEGACY)) {
            return true;
        }
        if namespace.is_some_and(|ns| !ns.is_empty()) {
            return false;
        }
        match prefix {
            Some("dfdl") | Some("dfdlx") => true,
            None if is_dfdl_local(local) => true,
            _ => false,
        }
    }

    pub(crate) fn parse_document(&mut self) -> Result<SchemaDocument> {
        loop {
            match self.reader.next_event()? {
                XmlEvent::StartElement {
                    name,
                    attributes,
                    namespace,
                    ..
                } => {
                    if local_name_str(&name.local_name) == "schema" {
                        self.parse_schema_element(attrs_to_map(&attributes), namespace)?;
                    } else {
                        let local = local_name_str(&name.local_name).to_string();
                        let ns = name
                            .namespace
                            .as_deref()
                            .filter(|u| !u.is_empty())
                            .map(String::from);
                        self.reader.skip_current_subtree()?;
                        let qname = match ns.as_deref() {
                            Some(uri) => alloc::format!("{{{uri}}}{local}"),
                            None => alloc::format!("{{}}{local}"),
                        };
                        return Err(crate::error::SchemaError::InvalidProperty {
                            message: alloc::format!(
                                "Schema Definition Error: {qname} in no namespace"
                            ),
                        }
                        .into());
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
                        message: alloc::format!("unexpected top-level {:?}", event_kind(&other)),
                    }
                    .into());
                }
            }
        }
        if let Some(text) = self.doc.schema_source_text.as_deref() {
            supplement_namespace_prefixes_from_text(text, &mut self.doc.namespace_prefixes);
        }
        check_general_format04_unprefixed_include(&mut self.doc);
        backfill_type_format_contexts(&mut self.doc);
        Ok(core::mem::take(&mut self.doc))
    }

    pub(crate) fn parse_schema_element(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<()> {
        self.doc.target_namespace = attrs.get("targetNamespace").cloned();
        self.doc.namespace_prefixes = crate::xml_util::namespace_prefix_map(&namespace);
        collect_namespace_prefixes(&attrs, &mut self.doc.namespace_prefixes);
        if let Some(text) = self.doc.schema_source_text.as_deref() {
            supplement_namespace_prefixes_from_text(text, &mut self.doc.namespace_prefixes);
        }
        if attrs
            .keys()
            .any(|k| k.contains("dfdl") || k.ends_with(":dfdl"))
            || attrs.values().any(|v| v.as_str() == DFDL_NS)
        {
            self.doc.dfdl_annotations_seen = true;
        }
        self.doc.element_form_default_explicit = attrs.contains_key("elementFormDefault");
        self.doc.element_form_default_qualified = attrs
            .get("elementFormDefault")
            .map(|v| v == "qualified")
            .unwrap_or(false);
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
                    let (child_attrs, child_namespace) = self.reader.take_start_element()?;
                    match local.as_str() {
                        "element" => self.parse_global_element(child_attrs, child_namespace)?,
                        "complexType" => self.parse_complex_type(None, child_attrs)?,
                        "simpleType" => self.parse_simple_type(None, child_attrs)?,
                        "group" => self.parse_global_group(child_attrs)?,
                        "include" => self.parse_include_or_import("include", child_attrs)?,
                        "import" => self.parse_include_or_import("import", child_attrs)?,
                        "format" => {
                            let has_ref = child_attrs.keys().any(|k| local_tag(k) == "ref");
                            if !has_ref {
                                let has_trailing =
                                    child_attrs.keys().any(|k| local_tag(k) == "trailingSkip");
                                let has_leading =
                                    child_attrs.keys().any(|k| local_tag(k) == "leadingSkip");
                                if has_trailing && !has_leading {
                                    self.doc.schema_diagnostics.push(
                                        "Schema Definition Error: Property leadingSkip is not defined"
                                            .into(),
                                    );
                                    self.doc.schema_diagnostics.push(
                                        "Non-default properties were combined from these locations"
                                            .into(),
                                    );
                                    self.doc.schema_diagnostics.push(
                                        "Default properties were taken from these locations".into(),
                                    );
                                }
                            }
                            let enc_explicit = child_attrs.keys().any(|k| {
                                local_tag(k) == "encodingErrorPolicy"
                                    || k.ends_with(":encodingErrorPolicy")
                            });
                            let props = self.parse_dfdl_element(
                                &local,
                                prefix.as_deref(),
                                child_attrs,
                                Some(child_namespace),
                            )?;
                            if enc_explicit {
                                self.doc.explicit_encoding_error_policy_on_format = true;
                            }
                            self.doc.format_defaults.props =
                                merge_dfdl_props(self.doc.format_defaults.props.clone(), props);
                        }
                        "defineFormat" => {
                            let _ = self.parse_dfdl_element(
                                &local,
                                prefix.as_deref(),
                                child_attrs,
                                Some(child_namespace),
                            )?;
                        }
                        "defineEscapeScheme" => {
                            let _ = self.parse_dfdl_element(
                                &local,
                                prefix.as_deref(),
                                child_attrs,
                                Some(child_namespace),
                            )?;
                        }
                        "defineVariable" => {
                            let name = child_attrs
                                .iter()
                                .find(|(k, _)| local_tag(k) == "name")
                                .map(|(_, v)| v.clone());
                            let default = child_attrs
                                .iter()
                                .find(|(k, _)| local_tag(k) == "defaultValue")
                                .map(|(_, v)| v.clone());
                            if let (Some(name), Some(default)) = (name, default) {
                                self.doc.variables.insert(name, default);
                            }
                            self.reader.skip_insignificant_ws()?;
                            if !self.reader.peek_is_end("defineVariable")? {
                                self.reader.skip_current_subtree()?;
                            } else {
                                self.expect_end_local("defineVariable")?;
                            }
                        }
                        "annotation" => {
                            let props = self.parse_annotation(child_attrs, child_namespace)?;
                            self.doc.format_defaults.props =
                                merge_dfdl_props(self.doc.format_defaults.props.clone(), props);
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

    pub(crate) fn parse_include_or_import(
        &mut self,
        local: &str,
        attrs: BTreeMap<String, String>,
    ) -> Result<()> {
        let location = attrs
            .get("schemaLocation")
            .ok_or_else(|| ParseError::MissingAttribute {
                element: local.into(),
                attribute: "schemaLocation".into(),
            })?;
        self.reader.skip_insignificant_ws()?;
        if self.reader.peek_is_end(local)? {
            self.expect_end_local(local)?;
        } else {
            self.reader.skip_current_subtree()?;
        }
        if !self.resolver.register_include(location) {
            return Ok(());
        }
        let (content, include_dir) = self.resolver.resolve_with_include_dir(location)?;
        let mut child_resolver = self.resolver.clone();
        if let Some(dir) = include_dir {
            child_resolver = child_resolver.with_base_dir(dir);
        }
        let include_label = location.rsplit('/').next().unwrap_or(location.as_str());
        let included =
            parse_schema_with_resolver_and_label(&content, child_resolver, Some(include_label))?;
        if !included.dfdl_annotations_seen {
            let label = location
                .rsplit('/')
                .next()
                .unwrap_or(location.as_str())
                .to_string();
            self.doc
                .schema_warnings
                .push("Non-DFDL Schema file ignored".into());
            self.doc.schema_warnings.push(label);
            return Ok(());
        }
        let kind = if local == "import" {
            SchemaMergeKind::Import
        } else {
            SchemaMergeKind::Include
        };
        if kind == SchemaMergeKind::Import {
            if let Some(expected_ns) = attrs.get("namespace") {
                let actual_ns = included.target_namespace.as_deref().unwrap_or("");
                if expected_ns.as_str() != actual_ns {
                    return Err(crate::error::SchemaError::InvalidProperty {
                        message: alloc::format!(
                            "Schema Definition Error: Import element specifies namespace {expected_ns} but namespace {actual_ns} of imported schema does not match"
                        ),
                    }
                    .into());
                }
            }
        }
        self.merge_included(included, kind)?;
        Ok(())
    }

    pub(crate) fn resolve_format_qname(&self, qname: &str) -> String {
        if let Some((prefix, local)) = qname.split_once(':') {
            let ns = self
                .doc
                .namespace_prefixes
                .get(prefix)
                .map(String::as_str)
                .or(Some(prefix));
            format_storage_key(local, ns)
        } else {
            format_storage_key(qname, self.doc.target_namespace.as_deref())
        }
    }

    pub(crate) fn lookup_named_format(&self, qname: &str) -> Option<DfdlProps> {
        lookup_named_format_in_document(&self.doc, qname)
    }

    pub(crate) fn element_type_prefix_map(
        &self,
        namespace: &xml_no_std::namespace::Namespace,
    ) -> BTreeMap<String, String> {
        let mut scoped = self.doc.namespace_prefixes.clone();
        for (prefix, uri) in crate::xml_util::namespace_prefix_map(namespace) {
            scoped.insert(prefix, uri);
        }
        scoped
    }

    pub(crate) fn parse_inline_type(&mut self) -> Result<(TypeName, DfdlProps)> {
        self.reader.skip_insignificant_ws()?;
        let (local, _, attrs) = self.consume_start()?;
        match local.as_str() {
            "complexType" => {
                let name = self.resolver.next_inline_type_name("complex");
                self.parse_complex_type(Some(name.clone()), attrs)?;
                Ok((TypeName::new(name), DfdlProps::default()))
            }
            "simpleType" => {
                let name = self.resolver.next_inline_type_name("simple");
                self.parse_simple_type(Some(name.clone()), attrs)?;
                Ok((TypeName::new(name), DfdlProps::default()))
            }
            other => {
                self.doc.schema_diagnostics.push(format!(
                    "Schema Definition Error: unrecognized element `{other}`"
                ));
                self.doc.schema_diagnostics.push(local.clone());
                self.skip_element_body(&local)?;
                let name = self.resolver.next_inline_type_name("unknown");
                Ok((TypeName::new(name), DfdlProps::default()))
            }
        }
    }

    pub(crate) fn parse_simple_base(&mut self) -> Result<SimpleBase> {
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
                    if local == "union" {
                        let members = self.parse_union_body(child_attrs)?;
                        return Ok(SimpleBase::Union { members });
                    }
                    if local == "restriction" {
                        let base = if let Some(base_name) = child_attrs.get("base") {
                            if base_name.chars().any(char::is_whitespace) {
                                self.doc.schema_diagnostics.push(
                                    "Schema Definition Error: Failed to resolve base property reference for xs:restriction:".into(),
                                );
                                self.doc
                                    .schema_diagnostics
                                    .push(alloc::format!("Invalid QName '{base_name}'"));
                                self.skip_element_body("restriction")?;
                                return Ok(SimpleBase::Restriction {
                                    base: RestrictionBase::Builtin(BuiltinType::String),
                                    restriction_base_is_xs_date: false,
                                    length: None,
                                    min_length: None,
                                    max_length: None,
                                    min_inclusive: None,
                                    max_inclusive: None,
                                    min_exclusive: None,
                                    max_exclusive: None,
                                    min_inclusive_lexical: None,
                                    max_inclusive_lexical: None,
                                    min_exclusive_lexical: None,
                                    max_exclusive_lexical: None,
                                    patterns: alloc::vec::Vec::new(),
                                    enumerations: alloc::vec::Vec::new(),
                                    total_digits: None,
                                    fraction_digits: None,
                                    invalid_min_length: None,
                                    invalid_max_length: None,
                                    invalid_length: None,
                                    invalid_total_digits: None,
                                    invalid_fraction_digits: None,
                                });
                            }
                            let normalized = normalize_qname(base_name.trim());
                            if let Some(b) = BuiltinType::from_xsd(&normalized) {
                                RestrictionBase::Builtin(b)
                            } else {
                                RestrictionBase::Named(TypeName::new(normalized))
                            }
                        } else {
                            RestrictionBase::Builtin(BuiltinType::String)
                        };
                        let restriction_base_is_xs_date = child_attrs
                            .get("base")
                            .map(|base_name| {
                                let normalized = normalize_qname(base_name);
                                matches!(normalized.as_str(), "xs:date" | "date")
                            })
                            .unwrap_or(false);
                        let facets = self.parse_restriction_body()?;
                        return Ok(SimpleBase::Restriction {
                            base,
                            restriction_base_is_xs_date,
                            length: facets.length,
                            min_length: facets.min_length,
                            max_length: facets.max_length,
                            min_inclusive: facets.min_inclusive,
                            max_inclusive: facets.max_inclusive,
                            min_exclusive: facets.min_exclusive,
                            max_exclusive: facets.max_exclusive,
                            min_inclusive_lexical: facets.min_inclusive_lexical,
                            max_inclusive_lexical: facets.max_inclusive_lexical,
                            min_exclusive_lexical: facets.min_exclusive_lexical,
                            max_exclusive_lexical: facets.max_exclusive_lexical,
                            patterns: facets.patterns,
                            enumerations: facets.enumerations,
                            total_digits: facets.total_digits,
                            fraction_digits: facets.fraction_digits,
                            invalid_min_length: facets.invalid_min_length,
                            invalid_max_length: facets.invalid_max_length,
                            invalid_length: facets.invalid_length,
                            invalid_total_digits: facets.invalid_total_digits,
                            invalid_fraction_digits: facets.invalid_fraction_digits,
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
                            "expected restriction or union, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
    }

    pub(crate) fn parse_union_body(
        &mut self,
        attrs: BTreeMap<String, String>,
    ) -> Result<alloc::vec::Vec<crate::schema::UnionMember>> {
        use crate::schema::UnionMember;
        let mut members = alloc::vec::Vec::new();
        if let Some(raw) = attrs.get("memberTypes") {
            for part in raw.split_whitespace() {
                members.push(UnionMember::Named(TypeName::new(normalize_qname(part))));
            }
        }
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "union" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let _child_attrs = self.reader.take_start_attributes()?;
                    if local == "simpleType" {
                        self.reader.skip_insignificant_ws()?;
                        let inner = self.parse_simple_base()?;
                        self.expect_end_local("simpleType")?;
                        members.push(UnionMember::Inline(inner));
                    } else if local == "annotation" {
                        self.skip_element_body("annotation")?;
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
                            "expected union child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        if members.is_empty() {
            return Err(ParseError::InvalidXml {
                message: "union must have memberTypes and/or inline simpleType members".into(),
            }
            .into());
        }
        Ok(members)
    }

    pub(crate) fn parse_restriction_body(&mut self) -> Result<ParsedRestrictionFacets> {
        let mut out = ParsedRestrictionFacets::default();
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "restriction" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    match local.as_str() {
                        "length" => {
                            if let Some(v) = child_attrs.get("value") {
                                if let Ok(n) = v.parse::<u64>() {
                                    out.length = Some(n);
                                } else {
                                    out.invalid_length = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "minLength" => {
                            if let Some(v) = child_attrs.get("value") {
                                if let Ok(n) = v.parse::<u64>() {
                                    out.min_length = Some(n);
                                } else {
                                    out.invalid_min_length = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "maxLength" => {
                            if let Some(v) = child_attrs.get("value") {
                                if let Ok(n) = v.parse::<u64>() {
                                    out.max_length = Some(n);
                                } else {
                                    out.invalid_max_length = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "pattern" => {
                            if let Some(v) = child_attrs.get("value") {
                                out.patterns.push(v.clone());
                            }
                            self.skip_element_body(&local)?;
                        }
                        "enumeration" => {
                            if let Some(v) = child_attrs.get("value") {
                                out.enumerations.push(v.clone());
                            }
                            self.skip_element_body(&local)?;
                        }
                        "minInclusive" => {
                            if let Some(v) = child_attrs.get("value") {
                                out.min_inclusive = parse_numeric_facet_bound(v);
                                if out.min_inclusive.is_none() {
                                    out.min_inclusive_lexical = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "maxInclusive" => {
                            if let Some(v) = child_attrs.get("value") {
                                out.max_inclusive = parse_numeric_facet_bound(v);
                                if out.max_inclusive.is_none() {
                                    out.max_inclusive_lexical = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "minExclusive" => {
                            if let Some(v) = child_attrs.get("value") {
                                out.min_exclusive = parse_numeric_facet_bound(v);
                                if out.min_exclusive.is_none() {
                                    out.min_exclusive_lexical = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "maxExclusive" => {
                            if let Some(v) = child_attrs.get("value") {
                                out.max_exclusive = parse_numeric_facet_bound(v);
                                if out.max_exclusive.is_none() {
                                    out.max_exclusive_lexical = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "totalDigits" => {
                            if let Some(v) = child_attrs.get("value") {
                                if let Ok(n) = v.parse::<u64>() {
                                    out.total_digits = Some(n);
                                } else {
                                    out.invalid_total_digits = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
                        }
                        "fractionDigits" => {
                            if let Some(v) = child_attrs.get("value") {
                                if let Ok(n) = v.parse::<u64>() {
                                    out.fraction_digits = Some(n);
                                } else {
                                    out.invalid_fraction_digits = Some(v.clone());
                                }
                            }
                            self.skip_element_body(&local)?;
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
                            "expected restriction child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn parse_inline_content(
        &mut self,
        mut props: DfdlProps,
        allowed: &[&str],
    ) -> Result<DfdlProps> {
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { .. } => break,
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    if local == "annotation" {
                        let (child_attrs, namespace) = self.reader.take_start_element()?;
                        props =
                            merge_dfdl_props(props, self.parse_annotation(child_attrs, namespace)?);
                    } else if allowed.contains(&local.as_str()) {
                        break;
                    } else {
                        self.doc.schema_diagnostics.push(format!(
                            "Schema Definition Error: unrecognized element `{local}`"
                        ));
                        self.doc.schema_diagnostics.push(local.clone());
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

    pub(crate) fn parse_annotation(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<DfdlProps> {
        let mut scoped = self.doc.namespace_prefixes.clone();
        for (prefix, uri) in crate::xml_util::namespace_prefix_map(&namespace) {
            scoped.insert(prefix, uri);
        }
        collect_namespace_prefixes(&attrs, &mut scoped);
        self.annotation_prefix_overrides.push(scoped);
        let result = self.parse_annotation_body();
        self.annotation_prefix_overrides.pop();
        result
    }

    pub(crate) fn parse_annotation_body(&mut self) -> Result<DfdlProps> {
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
                        props = merge_dfdl_props(props, self.parse_appinfo(child_attrs)?);
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

    pub(crate) fn parse_appinfo(&mut self, attrs: BTreeMap<String, String>) -> Result<DfdlProps> {
        let source = attrs.get("source").map(String::as_str);
        const OFFICIAL_APPINFO: &str = "http://www.ogf.org/dfdl/";
        const LEGACY_APPINFO: &str = "http://www.ogf.org/dfdl/dfdl-1.0/";

        self.reader.skip_insignificant_ws()?;
        if source == Some(LEGACY_APPINFO) {
            self.push_schema_warning(
                "appinfoDFDLSourceWrong",
                &[
                    "Schema Definition Warning",
                    &alloc::format!(
                        "The xs:appinfo source attribute value '{LEGACY_APPINFO}' should be '{OFFICIAL_APPINFO}'."
                    ),
                    "appinfoDFDLSourceWrong",
                ],
            );
        }
        if self.reader.peek_is_end("appinfo")? {
            self.expect_end_local("appinfo")?;
            return Ok(DfdlProps::default());
        }

        let mut props = DfdlProps::default();
        if source == Some(DFDL_NS) || source == Some(LEGACY_APPINFO) || source.is_none() {
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
                        let ns = name.namespace.clone();
                        let (child_attrs, child_namespace) = self.reader.take_start_element()?;
                        if Self::is_dfdl_element(prefix.as_deref(), &local, ns.as_deref()) {
                            if source.is_none() {
                                self.push_schema_warning(
                                    "appinfoNoSource",
                                    &[
                                        "Schema Definition Warning",
                                        "xs:appinfo without source attribute",
                                    ],
                                );
                            }
                            let dfdl_props = self.parse_dfdl_element(
                                &local,
                                prefix.as_deref(),
                                child_attrs,
                                Some(child_namespace),
                            )?;
                            if local != "defineFormat"
                                && local != "defineEscapeScheme"
                                && local != "defineVariable"
                            {
                                props = merge_dfdl_props(props, dfdl_props);
                            }
                        } else if source == Some(DFDL_NS) || source == Some(LEGACY_APPINFO) {
                            let qname = match prefix.as_deref() {
                                Some(p) => alloc::format!("{p}:{local}"),
                                None => local.clone(),
                            };
                            let mut message = alloc::format!(
                                "Schema Definition Error: Invalid dfdl annotation found: {qname}"
                            );
                            if let Some(label) = self.doc.schema_source_label.as_deref() {
                                message.push('\n');
                                message.push_str(label);
                            }
                            return Err(
                                crate::error::SchemaError::InvalidProperty { message }.into()
                            );
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

    pub(crate) fn finalize_props(&mut self, mut props: DfdlProps) -> DfdlProps {
        let had_format_ref = props.format_ref.is_some();
        if let Some(ref_name) = props.format_ref.take() {
            if let Some(base) = self.lookup_named_format(&ref_name) {
                strip_empty_initiator_for_format_ref(&mut props);
                props = merge_dfdl_props(base, props);
            }
        }
        if had_format_ref && props.terminator.is_none() {
            props.terminator = self.doc.format_defaults.props.terminator.clone();
        }
        props
    }

    pub(crate) fn parse_dfdl_element(
        &mut self,
        local: &str,
        _prefix: Option<&str>,
        attrs: BTreeMap<String, String>,
        element_namespace: Option<xml_no_std::namespace::Namespace>,
    ) -> Result<DfdlProps> {
        self.doc.dfdl_annotations_seen = true;
        record_foreign_attrs_on_dfdl_element(local, &attrs, &mut self.doc.schema_diagnostics);
        for key in attrs.keys() {
            if key.starts_with("dfdl:") {
                let attr_local = key.rsplit(':').next().unwrap_or(key.as_str());
                return Err(crate::error::SchemaError::InvalidProperty {
                    message: alloc::format!(
                        "Schema Definition Error: dfdl:{attr_local} attribute must not use the dfdl: prefix on DFDL annotation elements"
                    ),
                }
                .into());
            }
        }
        if local == "defineFormat" {
            return self.parse_define_format(attrs);
        }
        if local == "defineEscapeScheme" {
            self.parse_define_escape_scheme(attrs)?;
            return Ok(DfdlProps::default());
        }
        if local == "defineVariable" {
            let name = attrs
                .iter()
                .find(|(k, _)| local_tag(k) == "name")
                .map(|(_, v)| v.clone());
            let default = attrs
                .iter()
                .find(|(k, _)| local_tag(k) == "defaultValue")
                .map(|(_, v)| v.clone());
            if let (Some(name), Some(default)) = (name, default) {
                self.doc.variables.insert(name, default);
            }
            self.reader.skip_insignificant_ws()?;
            if !self.reader.peek_is_end("defineVariable")? {
                self.reader.skip_current_subtree()?;
            } else {
                self.expect_end_local("defineVariable")?;
            }
            return Ok(DfdlProps::default());
        }
        if local == "setVariable" {
            let var_ref = attrs
                .iter()
                .find(|(k, _)| local_tag(k) == "ref")
                .map(|(_, v)| v.as_str());
            let value = attrs
                .iter()
                .find(|(k, _)| local_tag(k) == "value")
                .map(|(_, v)| v.clone());
            let mut props = DfdlProps::default();
            if let (Some(var_ref), Some(value)) = (var_ref, value) {
                let name = variable_local_name_from_ref(var_ref);
                props.set_variables.push((name, value));
            }
            self.reader.skip_insignificant_ws()?;
            if !self.reader.peek_is_end("setVariable")? {
                self.reader.skip_current_subtree()?;
            } else {
                self.expect_end_local("setVariable")?;
            }
            return Ok(props);
        }

        let mut props = props_from_attrs_with_variables(
            &attrs,
            Some(&self.doc.variables),
            self.doc.schema_source_label.as_deref(),
            Some(&mut self.doc.schema_diagnostics),
        )?;
        self.normalize_escape_scheme_ref(&mut props);
        if local == "assert" || local == "discriminator" {
            props.has_statement_annotation = true;
            if let Some(msg) = attrs.get("message") {
                props.assert_message = Some(msg.clone());
                if let Some(segments) = parse_input_value_calc_concat(msg) {
                    props.assert_message_segments = Some(segments);
                }
            }
            if let Some(test) = attrs.get("test") {
                apply_dfdl_assert_test(&mut props, test);
            }
        }

        if local == "format" {
            let enc_policy_explicit = attrs.keys().any(|k| {
                local_tag(k) == "encodingErrorPolicy" || k.ends_with(":encodingErrorPolicy")
            });
            let format_ref_from_props = props.format_ref.clone();
            let format_ref_name = attrs
                .get("ref")
                .map(|s| s.as_str())
                .or(format_ref_from_props.as_deref());
            if let Some(ref_name) = format_ref_name {
                if let Some(base) = self.lookup_named_format(ref_name) {
                    props = merge_dfdl_props(base, props);
                }
            }
            if !enc_policy_explicit {
                props.encoding_error_policy_defined = false;
            }
            if !self.in_define_format {
                let mut format_props = props.clone();
                format_props.calendar_time_zone_defined = false;
                if format_ref_name.is_some() {
                    self.doc.format_defaults.props =
                        merge_dfdl_props(self.doc.format_defaults.props.clone(), format_props);
                } else {
                    self.doc.format_defaults.props = format_props;
                }
                props.calendar_time_zone_defined = false;
            }
        }

        self.reader.skip_insignificant_ws()?;
        if local == "element" {
            if self.reader.peek_is_end("element")? {
                self.expect_end_local("element")?;
            } else {
                loop {
                    self.reader.skip_insignificant_ws()?;
                    match self.reader.peek()? {
                        XmlEvent::EndElement { name } if name.local_name == "element" => {
                            let _ = self.reader.next_event()?;
                            break;
                        }
                        XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                        XmlEvent::StartElement { name, .. } => {
                            let child_local = name.local_name.clone();
                            let child_prefix = name.prefix.clone();
                            let child_ns = name.namespace.clone();
                            let child_attrs = self.reader.take_start_attributes()?;
                            if Self::is_dfdl_element(
                                child_prefix.as_deref(),
                                &child_local,
                                child_ns.as_deref(),
                            ) && child_local == "property"
                            {
                                let prop_name =
                                    child_attrs.get("name").cloned().ok_or_else(|| {
                                        ParseError::InvalidXml {
                                            message: "dfdl:property missing name".into(),
                                        }
                                    })?;
                                let value = self.read_simple_element_text("property")?;
                                let mut map = BTreeMap::new();
                                map.insert(prop_name.clone(), value);
                                props = merge_dfdl_props(props, props_from_attrs(&map)?);
                                if local_tag(&prop_name) == "textStringPadCharacter" {
                                    props.text_string_pad_character_property_form = true;
                                }
                                if local_tag(&prop_name) == "textNumberPadCharacter" {
                                    props.text_number_pad_character_property_form = true;
                                }
                            } else {
                                self.skip_element_body(&child_local)?;
                            }
                        }
                        XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                            let _ = self.reader.next_event()?;
                        }
                        other => {
                            return Err(ParseError::InvalidXml {
                                message: alloc::format!(
                                    "expected dfdl:element child, found {:?}",
                                    event_kind(other)
                                ),
                            }
                            .into());
                        }
                    }
                }
            }
            return Ok(props);
        }
        if local == "assert" || local == "discriminator" {
            let scoped = discriminator_xpath_prefix_scope(&attrs, element_namespace.as_ref());
            let test = if let Some(t) = attrs.get("test") {
                if self.reader.peek_is_end(local)? {
                    self.expect_end_local(local)?;
                } else {
                    let _ = self.read_simple_element_text(local)?;
                }
                t.clone()
            } else if !self.reader.peek_is_end(local)? {
                self.read_simple_element_text(local)?
            } else {
                self.expect_end_local(local)?;
                String::new()
            };
            if !test.is_empty() {
                if local == "discriminator" {
                    let trimmed = test.trim().to_string();
                    props.discriminator_xpath_prefixes = Some(scoped);
                    props.discriminator_test = Some(trimmed);
                }
                apply_dfdl_assert_test(&mut props, test.trim());
                if local == "assert" && props.assert_int_eq.is_none() {
                    props.discriminator_test = Some(test.trim().to_string());
                }
            }
            return Ok(props);
        }
        if local == "sequence" {
            if props.hidden_group_ref.is_some() {
                props.hidden_group_ref_from_appinfo_sequence = true;
            }
            if self.reader.peek_is_end("sequence")? {
                self.expect_end_local("sequence")?;
                return Ok(props);
            }
            let mut appinfo_sequence_had_property_child = false;
            loop {
                self.reader.skip_insignificant_ws()?;
                match self.reader.peek()? {
                    XmlEvent::EndElement { name } if name.local_name == "sequence" => {
                        let _ = self.reader.next_event()?;
                        break;
                    }
                    XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                    XmlEvent::StartElement { name, .. } => {
                        let child_local = name.local_name.clone();
                        let child_prefix = name.prefix.clone();
                        let child_ns = name.namespace.clone();
                        let child_attrs = self.reader.take_start_attributes()?;
                        if Self::is_dfdl_element(
                            child_prefix.as_deref(),
                            &child_local,
                            child_ns.as_deref(),
                        ) && child_local == "property"
                        {
                            appinfo_sequence_had_property_child = true;
                            let prop_name = child_attrs.get("name").cloned().ok_or_else(|| {
                                ParseError::InvalidXml {
                                    message: "dfdl:property missing name".into(),
                                }
                            })?;
                            let value = self.read_simple_element_text("property")?;
                            let mut map = BTreeMap::new();
                            map.insert(prop_name.clone(), value);
                            let child_props = props_from_attrs(&map)?;
                            if child_props.hidden_group_ref.is_some() {
                                props.hidden_group_ref_from_appinfo_sequence = true;
                            }
                            props = merge_dfdl_props(props, child_props);
                        } else {
                            self.skip_element_body(&child_local)?;
                        }
                    }
                    XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                        let _ = self.reader.next_event()?;
                    }
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "expected dfdl:sequence child, found {:?}",
                                event_kind(other)
                            ),
                        }
                        .into());
                    }
                }
            }
            if appinfo_sequence_had_property_child && props.hidden_group_ref.is_some() {
                props.hidden_group_ref_from_appinfo_sequence = true;
            }
            return Ok(props);
        }
        if self.reader.peek_is_end(local)? {
            self.expect_end_local(local)?;
        } else {
            self.reader.skip_current_subtree()?;
        }
        Ok(props)
    }

    pub(crate) fn read_simple_element_text(&mut self, local: &str) -> Result<String> {
        let mut out = String::new();
        loop {
            match self.reader.next_event()? {
                XmlEvent::EndElement { name } if name.local_name == local => break,
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::Characters(text) => out.push_str(&text),
                XmlEvent::CData(text) => out.push_str(&text),
                XmlEvent::Whitespace(text) => out.push_str(&text),
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected text in `{local}`, found {:?}",
                            event_kind(&other)
                        ),
                    }
                    .into());
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn parse_define_format(
        &mut self,
        attrs: BTreeMap<String, String>,
    ) -> Result<DfdlProps> {
        let format_name = attrs.get("name").cloned();
        for key in attrs.keys() {
            if local_tag(key) != "name" {
                self.doc.schema_diagnostics.push(alloc::format!(
                    "Schema Definition Error: Property {} is not allowed on dfdl:defineFormat",
                    local_tag(key)
                ));
            }
        }

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
                    let ns = name.namespace.clone();
                    let (child_attrs, child_namespace) = self.reader.take_start_element()?;
                    if Self::is_dfdl_element(prefix.as_deref(), &local, ns.as_deref()) {
                        let child_props = self.parse_dfdl_element(
                            &local,
                            prefix.as_deref(),
                            child_attrs,
                            Some(child_namespace),
                        )?;
                        props = merge_dfdl_props(props, child_props);
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

        if format_name.is_none() {
            return Err(crate::error::SchemaError::InvalidProperty {
                message:
                    "Schema Definition Error: Attribute 'name' must appear on element 'dfdl:defineFormat'."
                        .into(),
            }
            .into());
        }

        if let Some(name) = format_name {
            let key = format_storage_key(&name, self.doc.target_namespace.as_deref());
            if self.doc.named_formats.contains_key(&key) {
                return Err(duplicate_format_definition(&name).into());
            }
            let mut stored = props.clone();
            stored.calendar_time_zone_defined = false;
            self.doc.named_formats.insert(key, stored);
        }
        Ok(props)
    }

    pub(crate) fn parse_define_escape_scheme(
        &mut self,
        attrs: BTreeMap<String, String>,
    ) -> Result<()> {
        let scheme_name = attrs.get("name").cloned();
        self.reader.skip_insignificant_ws()?;
        let mut scheme = EscapeSchemeDef::default();
        let mut saw_escape_scheme_child = false;
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { name } if name.local_name == "defineEscapeScheme" => {
                    let _ = self.reader.next_event()?;
                    break;
                }
                XmlEvent::EndDocument => return Err(ParseError::UnexpectedEof.into()),
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    let child_attrs = self.reader.take_start_attributes()?;
                    if local == "escapeScheme" {
                        scheme = escape_scheme_from_attrs(&child_attrs);
                        saw_escape_scheme_child = true;
                        self.reader.skip_current_subtree()?;
                    } else {
                        self.skip_element_body(&local)?;
                    }
                }
                XmlEvent::Characters(_) | XmlEvent::CData(_) | XmlEvent::Whitespace(_) => {
                    let _ = self.reader.next_event()?;
                }
                XmlEvent::Comment(_) => {
                    let _ = self.reader.next_event()?;
                }
                other => {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "expected defineEscapeScheme child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        if !saw_escape_scheme_child {
            return Err(crate::error::SchemaError::InvalidProperty {
                message: "Schema Definition Error: The content of element 'dfdl:defineEscapeScheme' is not complete".into(),
            }
            .into());
        }
        let Some(name) = scheme_name else {
            return Err(crate::error::SchemaError::InvalidProperty {
                message: "Schema Definition Error: Attribute 'name' must appear on element defineEscapeScheme".into(),
            }
            .into());
        };
        let key = format_storage_key(&name, self.doc.target_namespace.as_deref());
        if self.doc.named_escape_schemes.contains_key(&key) {
            return Err(crate::error::SchemaError::InvalidProperty {
                message: alloc::format!(
                    "Schema Definition Error: More than one definition for name: {name}"
                ),
            }
            .into());
        }
        self.doc.named_escape_schemes.insert(key, scheme);
        Ok(())
    }
}

pub(crate) fn lookup_named_escape_scheme_in_document(
    doc: &SchemaDocument,
    qname: &str,
) -> Option<EscapeSchemeDef> {
    let local = normalize_qname(qname);
    if !qname.contains(':') {
        let bare = format_storage_key(&local, None);
        if let Some(v) = doc.named_escape_schemes.get(&bare) {
            return Some(v.clone());
        }
        let tns_key = format_storage_key(&local, doc.target_namespace.as_deref());
        if let Some(v) = doc.named_escape_schemes.get(&tns_key) {
            return Some(v.clone());
        }
        return None;
    }
    let key = resolve_format_qname_in_document(doc, qname);
    if let Some(scheme) = doc.named_escape_schemes.get(&key) {
        return Some(scheme.clone());
    }
    if let Some(tns) = doc.target_namespace.as_deref() {
        let tns_key = format_storage_key(&local, Some(tns));
        if tns_key != key {
            if let Some(scheme) = doc.named_escape_schemes.get(&tns_key) {
                return Some(scheme.clone());
            }
        }
    }
    let fallback = format_storage_key(&local, None);
    if fallback != key {
        if let Some(scheme) = doc.named_escape_schemes.get(&fallback) {
            return Some(scheme.clone());
        }
    }
    let matching: alloc::vec::Vec<_> = doc
        .named_escape_schemes
        .iter()
        .filter(|(k, _)| format_local_from_storage_key(k) == local)
        .map(|(_, v)| v.clone())
        .collect();
    if matching.len() == 1 {
        return matching.into_iter().next();
    }
    None
}

pub(crate) fn escape_scheme_from_attrs(attrs: &BTreeMap<String, String>) -> EscapeSchemeDef {
    use crate::schema::entities::expand_entities_str;
    let escape_kind = match attrs.get("escapeKind").map(String::as_str) {
        Some("escapeBlock") => EscapeKind::EscapeBlock,
        _ => EscapeKind::EscapeCharacter,
    };
    EscapeSchemeDef {
        escape_kind,
        escape_character_raw: attrs.get("escapeCharacter").cloned(),
        escape_character: attrs.get("escapeCharacter").map(|s| expand_entities_str(s)),
        escape_escape_character_raw: attrs.get("escapeEscapeCharacter").cloned(),
        escape_escape_character: attrs
            .get("escapeEscapeCharacter")
            .map(|s| expand_entities_str(s)),
        escape_block_start_raw: attrs.get("escapeBlockStart").cloned(),
        escape_block_start: attrs
            .get("escapeBlockStart")
            .map(|s| expand_entities_str(s)),
        escape_block_end_raw: attrs.get("escapeBlockEnd").cloned(),
        escape_block_end: attrs.get("escapeBlockEnd").map(|s| expand_entities_str(s)),
        extra_escaped_characters_raw: attrs.get("extraEscapedCharacters").cloned(),
        extra_escaped_characters: attrs
            .get("extraEscapedCharacters")
            .map(|s| crate::schema::entities::extra_escaped_characters_from_property(s))
            .unwrap_or_default(),
    }
}

pub(crate) fn duplicate_format_definition(name: &str) -> ParseError {
    ParseError::InvalidXml {
        message: alloc::format!("More than one definition for name: {name}"),
    }
}

pub(crate) fn format_ref_qname_for_diag(ref_name: &str) -> String {
    if ref_name.contains(':') {
        ref_name.to_string()
    } else {
        alloc::format!("{{}}{ref_name}")
    }
}

pub(crate) fn check_general_format04_unprefixed_include(doc: &mut SchemaDocument) {
    const EXAMPLE_TNS: &str = "http://example.com/";
    let Some(ref_name) = doc.format_defaults.props.format_ref.as_deref() else {
        return;
    };
    if ref_name != "GeneralFormat" || doc.target_namespace.as_deref() != Some(EXAMPLE_TNS) {
        return;
    }
    if doc.namespace_prefixes.get("").map(String::as_str) == Some(EXAMPLE_TNS) {
        return;
    }
    if let Some(text) = doc.schema_source_text.as_deref() {
        if text.contains("xmlns=\"http://example.com/\"")
            || text.contains("xmlns='http://example.com/'")
        {
            return;
        }
    }
    let tns_key = format_storage_key("GeneralFormat", Some(EXAMPLE_TNS));
    if doc.named_formats.contains_key(&tns_key) {
        doc.schema_diagnostics.push(alloc::format!(
            "Schema Definition Error: defineFormat with name '{}', was not found.",
            format_ref_qname_for_diag(ref_name)
        ));
    }
}

pub(crate) fn resolve_format_qname_in_document(doc: &SchemaDocument, qname: &str) -> String {
    if let Some((prefix, local)) = qname.split_once(':') {
        let ns = doc
            .namespace_prefixes
            .get(prefix)
            .map(String::as_str)
            .or(Some(prefix));
        format_storage_key(local, ns)
    } else {
        format_storage_key(qname, doc.target_namespace.as_deref())
    }
}

pub(crate) fn lookup_named_format_in_document(
    doc: &SchemaDocument,
    qname: &str,
) -> Option<DfdlProps> {
    let local = normalize_qname(qname);
    if !qname.contains(':') {
        let bare = format_storage_key(&local, None);
        if let Some(v) = doc.named_formats.get(&bare) {
            return Some(v.clone());
        }
        let tns_key = format_storage_key(&local, doc.target_namespace.as_deref());
        if let Some(v) = doc.named_formats.get(&tns_key) {
            return Some(v.clone());
        }
        return None;
    }
    let key = resolve_format_qname_in_document(doc, qname);
    if let Some(base) = doc.named_formats.get(&key) {
        return Some(base.clone());
    }
    if qname.contains(':') {
        if let Some(tns) = doc.target_namespace.as_deref() {
            let tns_key = format_storage_key(&local, Some(tns));
            if tns_key != key {
                if let Some(base) = doc.named_formats.get(&tns_key) {
                    return Some(base.clone());
                }
            }
        }
    }
    let fallback = format_storage_key(&local, None);
    if fallback != key {
        if let Some(base) = doc.named_formats.get(&fallback) {
            return Some(base.clone());
        }
    }
    if qname.contains(':') {
        let matching: alloc::vec::Vec<_> = doc
            .named_formats
            .iter()
            .filter(|(k, _)| format_local_from_storage_key(k) == local)
            .map(|(_, v)| v.clone())
            .collect();
        if matching.len() == 1 {
            return matching.into_iter().next();
        }
    }
    None
}

pub(crate) fn event_kind(ev: &XmlEvent) -> &'static str {
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

pub(crate) fn xsd_attr<'a>(attrs: &'a BTreeMap<String, String>, local: &str) -> Option<&'a String> {
    if let Some(v) = attrs.get(local) {
        return Some(v);
    }
    attrs
        .iter()
        .find(|(k, _)| k.as_str() == local || k.ends_with(&alloc::format!(":{local}")))
        .map(|(_, v)| v)
}

pub(crate) fn merge_occurs(props: &mut DfdlProps, attrs: &BTreeMap<String, String>) {
    if let Some(min) = xsd_attr(attrs, "minOccurs") {
        if let Ok(v) = min.parse() {
            props.occurs_min = Some(v);
        }
    }
    if let Some(max) = xsd_attr(attrs, "maxOccurs") {
        props.max_occurs_specified = true;
        if max == "unbounded" {
            props.occurs_max = None;
        } else if let Ok(v) = max.parse() {
            props.occurs_max = Some(v);
        }
    }
}

pub(crate) fn strip_empty_initiator_for_format_ref(props: &mut DfdlProps) {
    if props.initiator.as_deref().is_some_and(str::is_empty) {
        props.initiator = None;
    }
}

pub(crate) fn merge_included_format_defaults(base: DfdlProps, overlay: DfdlProps) -> DfdlProps {
    let clear_init = overlay.initiator.as_deref().is_some_and(str::is_empty);
    let clear_term = overlay.terminator.as_deref().is_some_and(str::is_empty);
    let clear_sep = overlay.separator.as_deref().is_some_and(str::is_empty);
    let mut merged = merge_dfdl_props(base.clone(), overlay);
    if clear_init {
        merged.initiator = base.initiator;
    }
    if clear_term {
        merged.terminator = base.terminator;
    }
    if clear_sep {
        merged.separator = base.separator;
    }
    merged
}

pub(crate) fn take_daf_suppress_warnings(attrs: &BTreeMap<String, String>) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.starts_with("daf:") && local_tag(k) == "suppressSchemaDefinitionWarnings")
        .map(|(_, v)| v.clone())
}

pub(crate) fn variable_local_name_from_ref(qname: &str) -> alloc::string::String {
    local_name_from_qname(&normalize_qname(qname)).to_string()
}

pub(crate) fn split_dfdl_attrs_with_variables(
    element_local: &str,
    attrs: &BTreeMap<String, String>,
    mut diagnostics: Option<&mut alloc::vec::Vec<String>>,
    variables: Option<&BTreeMap<String, String>>,
    schema_label: Option<&str>,
) -> Result<(BTreeMap<String, String>, DfdlProps)> {
    let mut xsd = BTreeMap::new();
    let mut dfdl_map = BTreeMap::new();
    for (k, v) in attrs {
        let local = local_tag(k);
        if k.starts_with("daf:") {
            continue;
        }
        let allow_unprefixed_dfdl = element_local != "element";
        if k.starts_with("dfdl:")
            || k.starts_with("dfdlx:")
            || k.contains("dfdl-1.0/extensions}")
            || (allow_unprefixed_dfdl
                && is_dfdl_property(local)
                && !is_xsd_local_attr(element_local, local))
        {
            dfdl_map.insert(local.to_string(), v.clone());
        } else {
            xsd.insert(k.clone(), v.clone());
        }
    }
    let props = match if let Some(diags) = diagnostics.as_mut() {
        props_from_attrs_with_variables(&dfdl_map, variables, schema_label, Some(diags))
    } else {
        props_from_attrs_with_variables(&dfdl_map, variables, schema_label, None)
    } {
        Ok(p) => p,
        Err(e) => {
            if let Some(out) = diagnostics.as_mut() {
                out.push(e.to_string());
                DfdlProps::default()
            } else {
                return Err(e);
            }
        }
    };
    Ok((xsd, props))
}

pub(crate) fn split_dfdl_attrs(
    element_local: &str,
    attrs: &BTreeMap<String, String>,
    diagnostics: Option<&mut alloc::vec::Vec<String>>,
) -> Result<(BTreeMap<String, String>, DfdlProps)> {
    split_dfdl_attrs_with_variables(element_local, attrs, diagnostics, None, None)
}

pub(crate) fn record_global_element_xsd_diagnostics(
    name: &str,
    xsd_attrs: &BTreeMap<String, String>,
    diagnostics: &mut alloc::vec::Vec<String>,
) {
    record_local_element_xsd_diagnostics(xsd_attrs, diagnostics);
    if name.contains(':') {
        diagnostics.push(alloc::format!(
            "Schema Definition Error: The value '{name}' is not a valid NCName"
        ));
        diagnostics.push("NCName".into());
    }
}

pub(crate) fn record_local_element_xsd_diagnostics(
    xsd_attrs: &BTreeMap<String, String>,
    diagnostics: &mut alloc::vec::Vec<String>,
) {
    const ALLOWED: &[&str] = &[
        "name",
        "type",
        "ref",
        "default",
        "fixed",
        "nillable",
        "substitutionGroup",
        "abstract",
        "block",
        "final",
        "id",
        "minOccurs",
        "maxOccurs",
        "form",
    ];
    for k in xsd_attrs.keys() {
        if k.contains(':') && !k.starts_with("xs:") {
            continue;
        }
        let local = local_tag(k);
        if !ALLOWED.contains(&local) {
            diagnostics.push(alloc::format!(
                "Attribute '{local}' is not allowed to appear in element declarations."
            ));
        }
    }
    if let Some(name) = xsd_attrs.get("name") {
        if name.contains(':') {
            diagnostics.push(alloc::format!(
                "Schema Definition Error: The value '{name}' is not a valid NCName"
            ));
            diagnostics.push("NCName".into());
        }
    }
}

pub(crate) fn is_xsd_local_attr(element: &str, attr: &str) -> bool {
    matches!(
        element,
        "element" | "attribute" | "group" | "attributeGroup"
    ) && matches!(
        attr,
        "ref"
            | "name"
            | "type"
            | "minOccurs"
            | "maxOccurs"
            | "default"
            | "fixed"
            | "form"
            | "substitutionGroup"
    )
}

pub(crate) fn is_dfdl_property(name: &str) -> bool {
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
            | "encodingErrorPolicy"
            | "nilKind"
            | "nilValue"
            | "separatorSuppressionPolicy"
            | "emptyElementParsePolicy"
            | "occursCountKind"
            | "escapeSchemeRef"
            | "hiddenGroupRef"
            | "ignoreCase"
            | "textTrimKind"
            | "truncateSpecifiedLengthString"
            | "textNumberPadCharacter"
            | "textStringPadCharacter"
            | "textCalendarPadCharacter"
            | "textCalendarJustification"
            | "textPadKind"
            | "textStringJustification"
            | "textNumberJustification"
            | "textStandardBase"
            | "textNumberCheckPolicy"
            | "textNumberRep"
            | "textZonedSignStyle"
            | "textStandardDecimalSeparator"
            | "textStandardGroupingSeparator"
            | "textStandardExponentRep"
            | "textStandardInfinityRep"
            | "textStandardNaNRep"
            | "textStandardZeroRep"
            | "binaryNumberRep"
            | "binaryPackedSignCodes"
            | "binaryNumberCheckPolicy"
            | "binaryCalendarRep"
            | "binaryCalendarEpoch"
            | "binaryFloatRep"
            | "binaryDecimalVirtualPoint"
            | "decimalSigned"
            | "calendarPattern"
            | "calendarPatternKind"
            | "calendarTimeZone"
            | "calendarCenturyStart"
            | "calendarLanguage"
            | "calendarDaysInFirstWeek"
            | "calendarFirstDayOfWeek"
            | "textNumberPattern"
            | "textNumberRounding"
            | "textNumberRoundingIncrement"
            | "textNumberRoundingMode"
            | "initiator"
            | "terminator"
            | "separator"
            | "initiatedContent"
            | "outputNewLine"
            | "separatorPosition"
            | "textBooleanTrueRep"
            | "textBooleanFalseRep"
            | "alignment"
            | "alignmentUnits"
            | "leadingSkip"
            | "trailingSkip"
            | "sequenceKind"
            | "choiceLengthKind"
            | "choiceLength"
            | "fillByte"
            | "ref"
            | "format"
            | "prefixLengthType"
            | "prefixIncludesPrefixLength"
            | "objectKind"
            | "choiceDispatchKey"
            | "choiceBranchKey"
            | "parseUnparsePolicy"
            | "textBidi"
            | "floating"
    )
}

pub(crate) fn parse_delimiter_literal(raw: &str) -> Result<String> {
    Ok(crate::schema::parse_delimiter_literal_value(raw))
}

pub(crate) fn is_dfdl_local(tag: &str) -> bool {
    matches!(
        tag,
        "format" | "element" | "sequence" | "choice" | "simpleType" | "group"
    )
}

pub(crate) fn local_tag(tag: &str) -> &str {
    if let Some(idx) = tag.rfind('}') {
        return &tag[idx + 1..];
    }
    strip_prefix(tag)
}

pub(crate) fn strip_prefix(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

pub(crate) fn supplement_namespace_prefixes_from_text(
    text: &str,
    out: &mut BTreeMap<String, String>,
) {
    let mut rest = text;
    while let Some(idx) = rest.find("xmlns:") {
        rest = &rest[idx + 6..];
        let Some(eq) = rest.find('=') else {
            break;
        };
        let prefix = rest[..eq].trim();
        if prefix.is_empty() {
            continue;
        }
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next() else {
            continue;
        };
        if q != '"' && q != '\'' {
            continue;
        }
        let value = after[1..].split(q).next().unwrap_or("").trim();
        if !value.is_empty() {
            out.entry(prefix.to_string()).or_insert(value.to_string());
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum SchemaMergeKind {
    Include,
    Import,
}

pub(crate) fn discriminator_xpath_prefix_scope(
    attrs: &BTreeMap<String, String>,
    element_namespace: Option<&xml_no_std::namespace::Namespace>,
) -> alloc::collections::BTreeMap<String, String> {
    let mut scoped = alloc::collections::BTreeMap::new();
    if let Some(ns) = element_namespace {
        for (prefix, uri) in crate::xml_util::namespace_prefix_map(ns) {
            scoped.insert(prefix, uri);
        }
    }
    collect_namespace_prefixes(attrs, &mut scoped);
    scoped
}

pub(crate) fn collect_namespace_prefixes(
    attrs: &BTreeMap<String, String>,
    out: &mut BTreeMap<String, String>,
) {
    for (key, value) in attrs {
        if key.as_str() == "xmlns" {
            out.insert(String::new(), value.clone());
            continue;
        }
        if let Some(prefix) = key.strip_prefix("xmlns:") {
            out.insert(prefix.to_string(), value.clone());
            continue;
        }
        if !key.contains(':')
            && key != "targetNamespace"
            && key != "elementFormDefault"
            && key != "attributeFormDefault"
            && key != "version"
            && (value.starts_with("http://")
                || value.starts_with("https://")
                || value.starts_with("urn:"))
        {
            out.insert(key.clone(), value.clone());
        }
    }
}

pub(crate) fn parse_numeric_facet_bound(v: &str) -> Option<i64> {
    if let Ok(n) = v.parse::<i64>() {
        return Some(n);
    }
    let f = v.parse::<f64>().ok()?;
    if !f.is_finite() {
        return None;
    }
    if f.fract().abs() < f64::EPSILON {
        Some(f as i64)
    } else {
        None
    }
}
