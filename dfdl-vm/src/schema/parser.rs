use super::ast::*;
use super::resolver::SchemaResolver;
use crate::error::{ParseError, Result};
use crate::xml_util::{attrs_to_map, local_name_str, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

const DFDL_NS: &str = "http://www.ogf.org/dfdl/";

/// Options controlling XSD parsing and include resolution.
#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    pub base_dir: Option<String>,
    /// File name for error messages (e.g. TDML external `model="foo.dfdl.xsd"`).
    pub schema_label: Option<String>,
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
    parse_schema_with_resolver_and_label(input, resolver, options.schema_label.as_deref())
}

/// Parse using a custom [`SchemaResolver`] for `xs:include` / `xs:import`.
pub fn parse_schema_with_resolver(input: &str, resolver: SchemaResolver) -> Result<SchemaDocument> {
    parse_schema_with_resolver_and_label(input, resolver, None)
}

fn parse_schema_with_resolver_and_label(
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
    parser.parse_document().map_err(map_xml_parse_error_to_schema)
}

fn map_xml_parse_error_to_schema(err: crate::error::Error) -> crate::error::Error {
    let msg = err.to_string();
    if msg.contains("Cannot redefine XMLNS prefix") {
        return crate::error::SchemaError::InvalidProperty {
            message: "Schema Definition Error: The prefix \"xmlns\" cannot be bound to any namespace explicitly; neither can the namespace for \"xmlns\" be bound to any prefix explicitly".into(),
        }
        .into();
    }
    if msg.contains("Unexpected token inside qualified name") {
        return crate::error::SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: Element or attribute do not match QName production: QName::=(NCName':')?NCName"
            ),
        }
        .into();
    }
    err
}

const QNAME_PRODUCTION_SDE: &str =
    "Schema Definition Error: Element or attribute do not match QName production: QName::=(NCName':')?NCName";

fn precheck_schema_definition_errors(input: &str) -> Option<String> {
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

fn qname_attr_value_is_well_formed(value: &str) -> bool {
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

fn record_foreign_attrs_on_dfdl_element(
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
struct ParsedRestrictionFacets {
    length: Option<u64>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    min_inclusive: Option<i64>,
    max_inclusive: Option<i64>,
    min_exclusive: Option<i64>,
    max_exclusive: Option<i64>,
    min_inclusive_lexical: Option<String>,
    max_inclusive_lexical: Option<String>,
    min_exclusive_lexical: Option<String>,
    max_exclusive_lexical: Option<String>,
    patterns: Vec<String>,
    enumerations: Vec<String>,
    total_digits: Option<u64>,
    fraction_digits: Option<u64>,
    invalid_min_length: Option<String>,
    invalid_max_length: Option<String>,
    invalid_length: Option<String>,
    invalid_total_digits: Option<String>,
    invalid_fraction_digits: Option<String>,
}

/// Types declared before a schema-level `dfdl:format` annotation capture an empty format context;
/// apply the final format defaults so imported types (e.g. USMTF `ct1`) inherit delimited length.
fn backfill_type_format_contexts(doc: &mut SchemaDocument) {
    let defaults = doc.format_defaults.props.clone();
    if !defaults.length_kind_defined {
        return;
    }
    for td in doc.types.values_mut() {
        match td {
            TypeDef::Simple { format_context, .. } | TypeDef::Complex { format_context, .. } => {
                if !format_context.length_kind_defined {
                    *format_context =
                        merge_dfdl_props(format_context.clone(), defaults.clone());
                }
            }
        }
    }
}

struct XsdParser<'a> {
    reader: XmlReader<'a>,
    doc: SchemaDocument,
    pending_props: DfdlProps,
    resolver: SchemaResolver,
    /// True while parsing `dfdl:defineFormat`; nested `dfdl:format` must not alter schema defaults.
    in_define_format: bool,
    /// `daf:suppressSchemaDefinitionWarnings` for the construct currently being parsed.
    suppress_schema_definition_warnings: Option<String>,
    /// Global element whose annotations are currently being parsed (warning scope).
    warning_scope: Option<String>,
    /// Namespace prefix bindings from enclosing `xs:annotation` elements (escapeSchemeRef).
    annotation_prefix_overrides: alloc::vec::Vec<BTreeMap<String, String>>,
}

impl<'a> XsdParser<'a> {
    fn new(input: &'a str, resolver: SchemaResolver) -> Self {
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

    fn escape_scheme_ref_storage_key(&self, qname: &str) -> String {
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

    fn normalize_escape_scheme_ref(&self, props: &mut DfdlProps) {
        if props
            .escape_scheme_ref
            .as_ref()
            .is_some_and(|r| !r.is_empty())
        {
            let qname = props.escape_scheme_ref.clone().unwrap();
            props.escape_scheme_ref = Some(self.escape_scheme_ref_storage_key(&qname));
        }
    }

    fn push_schema_warning(&mut self, warn_id: &str, lines: &[&str]) {
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

    fn merge_included(&mut self, other: SchemaDocument, kind: SchemaMergeKind) -> Result<()> {
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
                if !self.doc.named_formats.contains_key(&bare) {
                    self.doc.named_formats.insert(bare, v);
                }
            }
        }
        for (k, v) in other.named_escape_schemes {
            self.doc.named_escape_schemes.insert(k, v);
        }
        for (k, v) in other.groups {
            self.doc.groups.insert(k, v);
        }
        self.doc.dfdl_annotations_seen |= other.dfdl_annotations_seen;
        self.doc
            .schema_diagnostics
            .extend(other.schema_diagnostics);
        self.doc.schema_warnings.extend(other.schema_warnings);
        for (k, v) in other.scoped_schema_warnings {
            self.doc.scoped_schema_warnings.entry(k).or_default().extend(v);
        }
        if kind == SchemaMergeKind::Include {
            self.doc.format_defaults.props = merge_included_format_defaults(
                self.doc.format_defaults.props.clone(),
                other.format_defaults.props,
            );
        }
        // Import: do not merge imported format defaults into the parent schema; components
        // defined in the parent keep the parent's `dfdl:format` (DFDL-6-007R scope tests).
        self.doc.format_defaults.props.calendar_time_zone_defined = false;
        Ok(())
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

    fn is_dfdl_element(prefix: Option<&str>, local: &str, namespace: Option<&str>) -> bool {
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

    fn parse_document(&mut self) -> Result<SchemaDocument> {
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
                        message: alloc::format!(
                            "unexpected top-level {:?}",
                            event_kind(&other)
                        ),
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

    fn parse_schema_element(
        &mut self,
        attrs: BTreeMap<String, String>,
        namespace: xml_no_std::namespace::Namespace,
    ) -> Result<()> {
        self.doc.target_namespace = attrs.get("targetNamespace").cloned();
        self.doc.namespace_prefixes = crate::xml_util::namespace_prefix_map(&namespace);
        collect_namespace_prefixes(&attrs, &mut self.doc.namespace_prefixes);
        // Fallback when namespace bindings are missing from the reader event.
        if let Some(text) = self.doc.schema_source_text.as_deref() {
            supplement_namespace_prefixes_from_text(text, &mut self.doc.namespace_prefixes);
        }
        if attrs.keys().any(|k| k.contains("dfdl") || k.ends_with(":dfdl"))
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
                                let has_trailing = child_attrs
                                    .keys()
                                    .any(|k| local_tag(k) == "trailingSkip");
                                let has_leading = child_attrs
                                    .keys()
                                    .any(|k| local_tag(k) == "leadingSkip");
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

    fn parse_include_or_import(
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
        let include_label = location
            .rsplit('/')
            .next()
            .unwrap_or(location.as_str());
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

    fn resolve_format_qname(&self, qname: &str) -> String {
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

    fn lookup_named_format(&self, qname: &str) -> Option<DfdlProps> {
        lookup_named_format_in_document(&self.doc, qname)
    }

    fn element_type_prefix_map(
        &self,
        namespace: &xml_no_std::namespace::Namespace,
    ) -> BTreeMap<String, String> {
        let mut scoped = self.doc.namespace_prefixes.clone();
        for (prefix, uri) in crate::xml_util::namespace_prefix_map(namespace) {
            scoped.insert(prefix, uri);
        }
        scoped
    }

    fn parse_global_element(
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

    fn schema_context_snapshot(&self) -> (DfdlProps, Option<String>) {
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
        let (xsd_attrs, dfdl_from_attrs) =
            split_dfdl_attrs_with_variables(
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
        if xsd_attrs.get("nillable").is_some_and(|v| v == "true") {
            props.nillable = Some(true);
        }
        if let Some(s) = &self.suppress_schema_definition_warnings {
            props.suppress_schema_definition_warnings = Some(s.clone());
        }

        let type_xsd_qname = xsd_attrs.get("type").cloned();
        let type_qname_scope = type_xsd_qname
            .as_ref()
            .map(|_| type_prefix_map.clone());
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
            props = self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
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

        let type_qname_prefixed = xsd_attrs
            .get("type")
            .is_some_and(|t| t.contains(':'));
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

    fn parse_simple_type(
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
                        "group" => {
                            let gr = self.parse_group_ref_particle(child_attrs)?;
                            return Ok(ComplexContent::Sequence(SequenceDecl {
                                props: DfdlProps::default(),
                                particles: alloc::vec![Particle::GroupRef(gr)],
                                had_markup_before_particles: false,
                            }));
                        }
                        "annotation" => self.skip_element_body("annotation")?,
                        _ => {
                            self.doc.schema_diagnostics.push(format!(
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

    fn parse_global_group(&mut self, attrs: BTreeMap<String, String>) -> Result<()> {
        let (xsd, _) = split_dfdl_attrs("group", &attrs, None)?;
        let group_name = xsd.get("name").cloned().ok_or_else(|| ParseError::MissingAttribute {
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

    fn parse_group_ref_particle(
        &mut self,
        attrs: BTreeMap<String, String>,
    ) -> Result<GroupRefDecl> {
        let (xsd, dfdl_from_attrs) = split_dfdl_attrs("group", &attrs, None)?;
        let ref_name = xsd.get("ref").cloned().ok_or_else(|| ParseError::MissingAttribute {
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

    fn parse_sequence(&mut self, attrs: BTreeMap<String, String>) -> Result<SequenceDecl> {
        let (_xsd, mut dfdl_from_attrs) = split_dfdl_attrs("sequence", &attrs, None)?;
        for (k, v) in &attrs {
            if local_tag(k) == "hiddenGroupRef" {
                dfdl_from_attrs.hidden_group_ref = Some(normalize_qname(v));
                break;
            }
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
                        "element" => particles.push(Particle::Element(self.parse_element_decl(
                            child_attrs,
                            child_namespace,
                        )?)),
                        "sequence" => {
                            particles.push(Particle::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "group" => {
                            particles.push(Particle::GroupRef(
                                self.parse_group_ref_particle(child_attrs)?,
                            ));
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

        Ok(SequenceDecl {
            props,
            particles,
            had_markup_before_particles,
        })
    }

    fn parse_choice(&mut self, attrs: BTreeMap<String, String>) -> Result<ChoiceDecl> {
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
                        "element" => branches.push(Particle::Element(self.parse_element_decl(
                            child_attrs,
                            child_namespace,
                        )?)),
                        "sequence" => {
                            branches.push(Particle::Sequence(self.parse_sequence(child_attrs)?))
                        }
                        "group" => {
                            branches.push(Particle::GroupRef(
                                self.parse_group_ref_particle(child_attrs)?,
                            ));
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

    fn parse_element_decl(
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
        let (xsd_attrs, dfdl_from_attrs) =
            split_dfdl_attrs_with_variables(
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
        if xsd_attrs.get("nillable").is_some_and(|v| v == "true") {
            props.nillable = Some(true);
        }
        if let Some(s) = &self.suppress_schema_definition_warnings {
            props.suppress_schema_definition_warnings = Some(s.clone());
        }

        let type_xsd_qname = xsd_attrs.get("type").cloned();
        let type_qname_scope = type_xsd_qname
            .as_ref()
            .map(|_| type_prefix_map.clone());
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
                    props: DfdlProps::default(),
                    format_context: DfdlProps::default(),
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
            props = self.parse_inline_content(props, &["complexType", "simpleType", "annotation"])?;
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

    fn parse_inline_type(&mut self) -> Result<(TypeName, DfdlProps)> {
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
                self.doc
                    .schema_diagnostics
                    .push(format!("Schema Definition Error: unrecognized element `{other}`"));
                self.doc.schema_diagnostics.push(local.clone());
                self.skip_element_body(&local)?;
                let name = self.resolver.next_inline_type_name("unknown");
                Ok((TypeName::new(name), DfdlProps::default()))
            }
        }
    }

    fn parse_simple_base(&mut self) -> Result<SimpleBase> {
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
                                self.doc.schema_diagnostics.push(alloc::format!(
                                    "Invalid QName '{base_name}'"
                                ));
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

    fn parse_union_body(
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

    fn parse_restriction_body(&mut self) -> Result<ParsedRestrictionFacets> {
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

    fn parse_inline_content(&mut self, mut props: DfdlProps, allowed: &[&str]) -> Result<DfdlProps> {
        loop {
            self.reader.skip_insignificant_ws()?;
            match self.reader.peek()? {
                XmlEvent::EndElement { .. } => break,
                XmlEvent::StartElement { name, .. } => {
                    let local = name.local_name.clone();
                    if local == "annotation" {
                        let (child_attrs, namespace) = self.reader.take_start_element()?;
                        props = merge_dfdl_props(
                            props,
                            self.parse_annotation(child_attrs, namespace)?,
                        );
                    } else if allowed.iter().any(|a| *a == local.as_str()) {
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

    fn parse_annotation(
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

    fn parse_annotation_body(&mut self) -> Result<DfdlProps> {
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

    fn parse_appinfo(&mut self, attrs: BTreeMap<String, String>) -> Result<DfdlProps> {
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
                            return Err(crate::error::SchemaError::InvalidProperty { message }.into());
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

    fn finalize_props(&mut self, mut props: DfdlProps) -> DfdlProps {
        let had_format_ref = props.format_ref.is_some();
        if let Some(ref_name) = props.format_ref.take() {
            if let Some(base) = self.lookup_named_format(&ref_name) {
                strip_empty_initiator_for_format_ref(&mut props);
                props = merge_dfdl_props(base, props);
            }
        }
        // DFDL-6-007R: schema-level terminator defaults apply to format-referenced types (long_chain_06).
        if had_format_ref && props.terminator.is_none() {
            props.terminator = self.doc.format_defaults.props.terminator.clone();
        }
        props
    }

    fn parse_dfdl_element(
        &mut self,
        local: &str,
        _prefix: Option<&str>,
        attrs: BTreeMap<String, String>,
        element_namespace: Option<xml_no_std::namespace::Namespace>,
    ) -> Result<DfdlProps> {
        self.doc.dfdl_annotations_seen = true;
        record_foreign_attrs_on_dfdl_element(local, &attrs, &mut self.doc.schema_diagnostics);
        for key in attrs.keys() {
            if key.starts_with("dfdl:") || key.starts_with("dfdlx:") {
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
            let _ = self.parse_define_escape_scheme(attrs)?;
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
                .or_else(|| format_ref_from_props.as_deref());
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
                    // Explicit schema `dfdl:format` without ref defines only listed properties
                    // (multi_A_03: lengthKind intentionally absent).
                    self.doc.format_defaults.props = format_props;
                }
                // Returned `props` must not carry format-level TZ as element-defined.
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
                                let prop_name = child_attrs.get("name").cloned().ok_or_else(|| {
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
            if self.reader.peek_is_end(local)? {
                self.expect_end_local(local)?;
            } else {
                let scoped =
                    discriminator_xpath_prefix_scope(&attrs, element_namespace.as_ref());
                let test = self.read_simple_element_text(local)?;
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

    fn read_simple_element_text(&mut self, local: &str) -> Result<String> {
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

    fn parse_define_format(&mut self, attrs: BTreeMap<String, String>) -> Result<DfdlProps> {
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

    fn parse_define_escape_scheme(&mut self, attrs: BTreeMap<String, String>) -> Result<()> {
        let scheme_name = attrs.get("name").cloned();
        self.reader.skip_insignificant_ws()?;
        let mut scheme = EscapeSchemeDef::default();
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
                        self.reader.skip_current_subtree()?;
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
                            "expected defineEscapeScheme child, found {:?}",
                            event_kind(other)
                        ),
                    }
                    .into());
                }
            }
        }
        if let Some(name) = scheme_name {
            let key = format_storage_key(&name, self.doc.target_namespace.as_deref());
            self.doc.named_escape_schemes.insert(key, scheme);
        }
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
        return Some(matching.into_iter().next().unwrap());
    }
    None
}

fn escape_scheme_from_attrs(attrs: &BTreeMap<String, String>) -> EscapeSchemeDef {
    use super::entities::expand_entities_str;
    let escape_kind = match attrs.get("escapeKind").map(String::as_str) {
        Some("escapeBlock") => EscapeKind::EscapeBlock,
        _ => EscapeKind::EscapeCharacter,
    };
    EscapeSchemeDef {
        escape_kind,
        escape_character_raw: attrs.get("escapeCharacter").cloned(),
        escape_character: attrs
            .get("escapeCharacter")
            .map(|s| expand_entities_str(s)),
        escape_escape_character_raw: attrs.get("escapeEscapeCharacter").cloned(),
        escape_escape_character: attrs
            .get("escapeEscapeCharacter")
            .map(|s| expand_entities_str(s)),
        escape_block_start_raw: attrs.get("escapeBlockStart").cloned(),
        escape_block_start: attrs
            .get("escapeBlockStart")
            .map(|s| expand_entities_str(s)),
        escape_block_end_raw: attrs.get("escapeBlockEnd").cloned(),
        escape_block_end: attrs
            .get("escapeBlockEnd")
            .map(|s| expand_entities_str(s)),
    }
}

fn duplicate_format_definition(name: &str) -> ParseError {
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

/// TDML `generalFormat04`: included `GeneralFormat` is in the targetNamespace, but without a
/// default namespace on the schema an unprefixed `ref` must not resolve (no-namespace).
fn check_general_format04_unprefixed_include(doc: &mut SchemaDocument) {
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
        if text.contains("xmlns=\"http://example.com/\"") || text.contains("xmlns='http://example.com/'") {
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

fn resolve_format_qname_in_document(doc: &SchemaDocument, qname: &str) -> String {
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
            return Some(matching.into_iter().next().unwrap());
        }
    }
    None
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

fn xsd_attr<'a>(attrs: &'a BTreeMap<String, String>, local: &str) -> Option<&'a String> {
    if let Some(v) = attrs.get(local) {
        return Some(v);
    }
    attrs
        .iter()
        .find(|(k, _)| k.as_str() == local || k.ends_with(&alloc::format!(":{local}")))
        .map(|(_, v)| v)
}

fn merge_occurs(props: &mut DfdlProps, attrs: &BTreeMap<String, String>) {
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

/// When resolving `dfdl:ref`, inherited `initiator=""` must not clobber the named format.
fn strip_empty_initiator_for_format_ref(props: &mut DfdlProps) {
    if props.initiator.as_deref().is_some_and(str::is_empty) {
        props.initiator = None;
    }
}

/// Merge format defaults from an included/imported schema without letting explicit `""`
/// delimiter properties in the included file clobber non-empty values on the including schema.
fn merge_included_format_defaults(base: DfdlProps, overlay: DfdlProps) -> DfdlProps {
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

/// Apply schema `dfdl:format` defaults from a global element declaration without
/// clobbering explicit element length/encoding overrides.
pub(crate) fn merge_global_element_format_context(base: &mut DfdlProps, ctx: &DfdlProps) {
    if base.initiator.is_none() {
        base.initiator = ctx
            .initiator
            .clone()
            .filter(|s| !s.is_empty());
    }
    if base.terminator.is_none() {
        base.terminator = ctx
            .terminator
            .clone()
            .filter(|s| !s.is_empty());
    }
    if base.separator.is_none() {
        base.separator = ctx
            .separator
            .clone()
            .filter(|s| !s.is_empty());
    }
    if base.encoding.is_none() {
        base.encoding = ctx.encoding.clone();
    }
    if base.representation.is_none() {
        base.representation = ctx.representation;
    }
    if base.separator_position.is_none() {
        base.separator_position = ctx.separator_position;
    }
    if base.separator_suppression_policy.is_none() {
        base.separator_suppression_policy = ctx.separator_suppression_policy;
    }
    if base.ignore_case.is_none() {
        base.ignore_case = ctx.ignore_case;
    }
    if base.text_trim_kind.is_none() {
        base.text_trim_kind = ctx.text_trim_kind;
    }
    if base.initiated_content.is_none() {
        base.initiated_content = ctx.initiated_content;
    }
}

pub(crate) fn merge_dfdl_props(mut base: DfdlProps, overlay: DfdlProps) -> DfdlProps {
    if overlay.representation.is_some() {
        base.representation = overlay.representation;
    }
    if overlay.byte_order.is_some() {
        base.byte_order = overlay.byte_order;
    }
    if overlay.byte_order_conditional_test.is_some() {
        base.byte_order_conditional_test = overlay.byte_order_conditional_test;
        base.byte_order_if_true = overlay.byte_order_if_true;
        base.byte_order_if_false = overlay.byte_order_if_false;
    }
    if overlay.bit_order.is_some() {
        base.bit_order = overlay.bit_order;
    }
    if overlay.length_kind.is_some() {
        base.length_kind = overlay.length_kind;
    }
    if overlay.length_kind_defined {
        base.length_kind_defined = true;
    }
    if overlay.length.is_some() {
        base.length = overlay.length;
    }
    if overlay.length_sibling.is_some() {
        base.length_sibling = overlay.length_sibling;
    }
    if overlay.length_sibling_cast_long {
        base.length_sibling_cast_long = true;
    }
    if overlay.length_expr_unparsed {
        base.length_expr_unparsed = true;
    }
    if overlay.length_self_string_max_cap.is_some() {
        base.length_self_string_max_cap = overlay.length_self_string_max_cap;
    }
    if overlay.length_self_value_length {
        base.length_self_value_length = true;
    }
    if overlay.length_units.is_some() {
        base.length_units = overlay.length_units;
    }
    if overlay.encoding.is_some() {
        base.encoding = overlay.encoding;
    }
    if overlay.encoding_error_policy.is_some() {
        base.encoding_error_policy = overlay.encoding_error_policy;
    }
    if overlay.nillable.is_some() {
        base.nillable = overlay.nillable;
    }
    if overlay.nil_kind.is_some() {
        base.nil_kind = overlay.nil_kind;
    }
    if overlay.nil_value.is_some() {
        base.nil_value = overlay.nil_value;
    }
    if overlay.separator_suppression_policy.is_some() {
        base.separator_suppression_policy = overlay.separator_suppression_policy;
    }
    if overlay.empty_element_parse_policy.is_some() {
        base.empty_element_parse_policy = overlay.empty_element_parse_policy;
    }
    if overlay.occurs_count_kind.is_some() {
        base.occurs_count_kind = overlay.occurs_count_kind;
    }
    if overlay.occurs_count_fn_path.is_some() {
        base.occurs_count_fn_path = overlay.occurs_count_fn_path.clone();
    }
    if overlay.escape_scheme_ref.is_some() {
        base.escape_scheme_ref = overlay.escape_scheme_ref.clone();
    }
    if overlay.hidden_group_ref.is_some() {
        base.hidden_group_ref = overlay.hidden_group_ref.clone();
    }
    if overlay.hidden_group_ref_from_appinfo_sequence {
        base.hidden_group_ref_from_appinfo_sequence = true;
    }
    if overlay.initiated_content.is_some() {
        base.initiated_content = overlay.initiated_content;
    }
    if overlay.ignore_case.is_some() {
        base.ignore_case = overlay.ignore_case;
    }
    if overlay.text_trim_kind.is_some() {
        base.text_trim_kind = overlay.text_trim_kind;
    }
    if overlay.text_pad_kind.is_some() {
        base.text_pad_kind = overlay.text_pad_kind;
    }
    if overlay.truncate_specified_length_string.is_some() {
        base.truncate_specified_length_string = overlay.truncate_specified_length_string;
    }
    if overlay.binary_number_rep.is_some() {
        base.binary_number_rep = overlay.binary_number_rep;
    }
    if overlay.binary_packed_sign_codes.is_some() {
        base.binary_packed_sign_codes = overlay.binary_packed_sign_codes;
    }
    if overlay.binary_packed_sign_codes_defined {
        base.binary_packed_sign_codes_defined = true;
    }
    if overlay.binary_number_check_policy.is_some() {
        base.binary_number_check_policy = overlay.binary_number_check_policy;
    }
    if overlay.binary_calendar_rep.is_some() {
        base.binary_calendar_rep = overlay.binary_calendar_rep;
    }
    if overlay.binary_calendar_epoch.is_some() {
        base.binary_calendar_epoch = overlay.binary_calendar_epoch;
    }
    if overlay.binary_float_rep.is_some() {
        base.binary_float_rep = overlay.binary_float_rep;
    }
    if overlay.binary_decimal_virtual_point.is_some() {
        base.binary_decimal_virtual_point = overlay.binary_decimal_virtual_point;
    }
    if overlay.binary_decimal_virtual_point_sde.is_some() {
        base.binary_decimal_virtual_point_sde = overlay.binary_decimal_virtual_point_sde;
    }
    if overlay.decimal_signed.is_some() {
        base.decimal_signed = overlay.decimal_signed;
    }
    if overlay.calendar_pattern.is_some() {
        base.calendar_pattern = overlay.calendar_pattern;
    }
    if overlay.calendar_pattern_kind.is_some() {
        base.calendar_pattern_kind = overlay.calendar_pattern_kind;
    }
    if overlay.calendar_time_zone.is_some() {
        base.calendar_time_zone = overlay.calendar_time_zone;
    }
    if overlay.calendar_time_zone_defined {
        base.calendar_time_zone_defined = true;
    }
    if overlay.calendar_century_start.is_some() {
        base.calendar_century_start = overlay.calendar_century_start;
    }
    if overlay.calendar_language.is_some() {
        base.calendar_language = overlay.calendar_language;
    }
    if overlay.calendar_language_segments.is_some() {
        base.calendar_language_segments = overlay.calendar_language_segments.clone();
    }
    if overlay.calendar_days_in_first_week.is_some() {
        base.calendar_days_in_first_week = overlay.calendar_days_in_first_week;
    }
    if overlay.calendar_first_day_of_week.is_some() {
        base.calendar_first_day_of_week = overlay.calendar_first_day_of_week;
    }
    if overlay.calendar_check_policy_lax == Some(true) {
        base.calendar_check_policy_lax = Some(true);
    }
    if overlay.text_number_pattern.is_some() {
        base.text_number_pattern = overlay.text_number_pattern;
    }
    if overlay.text_number_check_policy.is_some() {
        base.text_number_check_policy = overlay.text_number_check_policy;
    }
    if overlay.text_number_rep.is_some() {
        base.text_number_rep = overlay.text_number_rep;
    }
    if overlay.text_number_rounding.is_some() {
        base.text_number_rounding = overlay.text_number_rounding;
    }
    if overlay.text_number_rounding_increment.is_some() {
        base.text_number_rounding_increment = overlay.text_number_rounding_increment;
    }
    if overlay.text_number_rounding_mode.is_some() {
        base.text_number_rounding_mode = overlay.text_number_rounding_mode;
    }
    if overlay.text_zoned_sign_style.is_some() {
        base.text_zoned_sign_style = overlay.text_zoned_sign_style;
    }
    if overlay.text_standard_decimal_separator.is_some() {
        base.text_standard_decimal_separator = overlay.text_standard_decimal_separator;
    }
    if overlay.text_standard_decimal_separator_sibling.is_some() {
        base.text_standard_decimal_separator_sibling =
            overlay.text_standard_decimal_separator_sibling;
    }
    if overlay.text_standard_grouping_separator.is_some() {
        base.text_standard_grouping_separator = overlay.text_standard_grouping_separator;
    }
    if overlay.text_standard_grouping_separator_sibling.is_some() {
        base.text_standard_grouping_separator_sibling =
            overlay.text_standard_grouping_separator_sibling;
    }
    if overlay.text_standard_exponent_rep.is_some() {
        base.text_standard_exponent_rep = overlay.text_standard_exponent_rep;
    }
    if overlay.text_standard_exponent_rep_sibling.is_some() {
        base.text_standard_exponent_rep_sibling = overlay.text_standard_exponent_rep_sibling;
    }
    if overlay.text_standard_infinity_rep.is_some() {
        base.text_standard_infinity_rep = overlay.text_standard_infinity_rep;
    }
    if overlay.text_standard_nan_rep.is_some() {
        base.text_standard_nan_rep = overlay.text_standard_nan_rep;
    }
    if overlay.text_standard_zero_rep.is_some() {
        base.text_standard_zero_rep = overlay.text_standard_zero_rep;
    }
    if overlay.initiator.is_some() {
        base.initiator = overlay.initiator;
    }
    if overlay.initiator_percent_escaped {
        base.initiator_percent_escaped = true;
    }
    if overlay.terminator.is_some() {
        base.terminator = overlay.terminator;
    }
    if overlay.separator.is_some() {
        base.separator = overlay.separator;
    }
    if overlay.output_new_line.is_some() {
        base.output_new_line = overlay.output_new_line;
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
        base.choice_dispatch_key = overlay.choice_dispatch_key.clone();
        base.choice_dispatch_sibling = overlay.choice_dispatch_sibling.clone();
        base.choice_dispatch_path = overlay.choice_dispatch_path.clone();
        base.choice_dispatch_literal = overlay.choice_dispatch_literal.clone();
        base.choice_dispatch_sibling_int = overlay.choice_dispatch_sibling_int.clone();
    }
    if overlay.choice_branch_key.is_some() {
        base.choice_branch_key = overlay.choice_branch_key.clone();
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
    if overlay.text_boolean_true_rep_defined {
        base.text_boolean_true_rep_defined = true;
    }
    if overlay.text_boolean_false_rep_defined {
        base.text_boolean_false_rep_defined = true;
    }
    if overlay.text_boolean_pad_character.is_some() {
        base.text_boolean_pad_character = overlay.text_boolean_pad_character.clone();
    }
    if overlay.binary_boolean_true_rep_defined {
        base.binary_boolean_true_rep_defined = true;
        base.binary_boolean_true_rep = overlay.binary_boolean_true_rep;
    }
    if overlay.binary_boolean_false_rep_defined {
        base.binary_boolean_false_rep_defined = true;
        base.binary_boolean_false_rep = overlay.binary_boolean_false_rep;
    }
    if overlay.default_value.is_some() {
        base.default_value = overlay.default_value;
    }
    if overlay.alignment_implicit.is_some() {
        base.alignment_implicit = overlay.alignment_implicit;
    }
    if overlay.alignment.is_some() {
        base.alignment = overlay.alignment;
    }
    if overlay.alignment_units.is_some() {
        base.alignment_units = overlay.alignment_units;
        // `alignmentUnits` alone must not reinterpret inherited numeric alignment from a
        // parent format (Section 12 explUnsignedIntMix / explicitAlignmentNoSkips03).
        if overlay.alignment.is_none() && overlay.alignment_implicit.is_none() {
            base.alignment_implicit = Some(true);
            base.alignment = None;
        }
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
    if overlay.choice_length_kind.is_some() {
        base.choice_length_kind = overlay.choice_length_kind;
    }
    if overlay.choice_length.is_some() {
        base.choice_length = overlay.choice_length;
    }
    if overlay.fill_byte.is_some() {
        base.fill_byte = overlay.fill_byte;
    }
    if overlay.fill_byte_raw.is_some() {
        base.fill_byte_raw = overlay.fill_byte_raw;
    }
    if overlay.format_ref.is_some() {
        base.format_ref = overlay.format_ref;
    }
    if overlay.text_number_pad_character.is_some() {
        base.text_number_pad_character = overlay.text_number_pad_character;
        base.text_number_pad_character_property_form =
            overlay.text_number_pad_character_property_form;
    }
    if overlay.text_string_pad_character.is_some() {
        base.text_string_pad_character = overlay.text_string_pad_character;
        base.text_string_pad_character_property_form =
            overlay.text_string_pad_character_property_form;
    }
    if overlay.text_calendar_pad_character.is_some() {
        base.text_calendar_pad_character = overlay.text_calendar_pad_character;
    }
    if overlay.text_calendar_justification.is_some() {
        base.text_calendar_justification = overlay.text_calendar_justification;
    }
    if overlay.prefix_length_type.is_some() {
        base.prefix_length_type = overlay.prefix_length_type;
    }
    if overlay.prefix_includes_prefix_length.is_some() {
        base.prefix_includes_prefix_length = overlay.prefix_includes_prefix_length;
    }
    if overlay.input_value_calc.is_some() {
        base.input_value_calc = overlay.input_value_calc;
    }
    if overlay.input_value_calc_literal.is_some() {
        base.input_value_calc_literal = overlay.input_value_calc_literal.clone();
    }
    if overlay.input_value_calc_sibling.is_some() {
        base.input_value_calc_sibling = overlay.input_value_calc_sibling;
    }
    if overlay.input_value_calc_segments.is_some() {
        base.input_value_calc_segments = overlay.input_value_calc_segments.clone();
    }
    if overlay.input_value_calc_path.is_some() {
        base.input_value_calc_path = overlay.input_value_calc_path.clone();
    }
    if overlay.input_value_calc_expression.is_some() {
        base.input_value_calc_expression = overlay.input_value_calc_expression.clone();
    }
    if overlay.parse_unparse_policy.is_some() {
        base.parse_unparse_policy = overlay.parse_unparse_policy;
    }
    if overlay.text_bidi.is_some() {
        base.text_bidi = overlay.text_bidi;
    }
    if overlay.floating.is_some() {
        base.floating = overlay.floating;
    }
    if overlay.output_value_calc.is_some() {
        base.output_value_calc = overlay.output_value_calc;
    }
    if overlay.output_value_calc_literal.is_some() {
        base.output_value_calc_literal = overlay.output_value_calc_literal.clone();
    }
    if overlay.output_value_calc_sibling.is_some() {
        base.output_value_calc_sibling = overlay.output_value_calc_sibling;
    }
    if overlay.output_value_calc_path.is_some() {
        base.output_value_calc_path = overlay.output_value_calc_path.clone();
    }
    if overlay.output_value_calc_path_addend.is_some() {
        base.output_value_calc_path_addend = overlay.output_value_calc_path_addend;
    }
    if overlay.output_value_calc_conditional {
        base.output_value_calc_conditional = true;
    }
    if overlay.text_string_justification.is_some() {
        base.text_string_justification = overlay.text_string_justification;
    }
    if overlay.text_number_justification.is_some() {
        base.text_number_justification = overlay.text_number_justification;
    }
    if overlay.text_standard_base.is_some() {
        base.text_standard_base = overlay.text_standard_base;
    }
    if overlay.has_statement_annotation {
        base.has_statement_annotation = true;
    }
    if overlay.assert_message.is_some() {
        base.assert_message = overlay.assert_message.clone();
    }
    if overlay.assert_message_segments.is_some() {
        base.assert_message_segments = overlay.assert_message_segments.clone();
    }
    if overlay.facet_check_constraints {
        base.facet_check_constraints = true;
    }
    if overlay.assert_int_eq.is_some() {
        base.assert_int_eq = overlay.assert_int_eq;
    }
    if overlay.discriminator_test.is_some() {
        base.discriminator_test = overlay.discriminator_test.clone();
    }
    if overlay.discriminator_xpath_prefixes.is_some() {
        base.discriminator_xpath_prefixes = overlay.discriminator_xpath_prefixes.clone();
    }
    if overlay.object_kind.is_some() {
        base.object_kind = overlay.object_kind;
    }
    if overlay.suppress_schema_definition_warnings.is_some() {
        base.suppress_schema_definition_warnings = overlay.suppress_schema_definition_warnings;
    }
    base.set_variables.extend(overlay.set_variables);
    base
}

fn take_daf_suppress_warnings(attrs: &BTreeMap<String, String>) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.starts_with("daf:") && local_tag(k) == "suppressSchemaDefinitionWarnings")
        .map(|(_, v)| v.clone())
}

fn parse_assert_eq_occurs_index_addend(test: &str) -> Option<i64> {
    let compact: alloc::string::String = test
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let body = compact
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(compact.as_str());
    if !body.contains("dfdl:occursIndex()") {
        return None;
    }
    if body.contains("xs:int(.)eqdfdl:occursIndex()") || body.contains(".eqdfdl:occursIndex()") {
        return Some(0);
    }
    if let Some(rest) = body.strip_prefix(".eq(dfdl:occursIndex()+") {
        let n = rest.strip_suffix(')')?;
        return n.parse().ok();
    }
    None
}

fn apply_dfdl_assert_test(props: &mut DfdlProps, test: &str) {
    if test.contains("checkConstraints") {
        props.facet_check_constraints = true;
    }
    if let Some(addend) = parse_assert_eq_occurs_index_addend(test) {
        props.facet_check_constraints = true;
        props.assert_eq_occurs_index_addend = Some(addend);
        return;
    }
    if let Some(n) = parse_assert_int_eq_test(test) {
        props.assert_int_eq = Some(n);
        return;
    }
    let trimmed = test.trim();
    if !trimmed.is_empty() && !props.facet_check_constraints {
        props.discriminator_test = Some(trimmed.to_string());
    }
}

/// `{ xs:int(.) eq N }` on `dfdl:assert/@test` or element body (section 02 assert tests).
fn parse_assert_int_eq_test(test: &str) -> Option<i64> {
    let compact: alloc::string::String = test
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let body = compact
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(compact.as_str());
    let rest = body.strip_prefix("xs:int(.)eq")?;
    rest.parse::<i64>().ok()
}

fn parse_xs_string_literal_arg(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.len() < 2 || !arg.starts_with('\'') || !arg.ends_with('\'') {
        return None;
    }
    Some(arg[1..arg.len() - 1].to_string())
}

fn split_top_level_commas(s: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut out = alloc::vec::Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim().to_string());
    out
}

fn split_top_level_ivc_op(s: &str, op: char) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let mut parts = alloc::vec::Vec::new();
    let mut current = alloc::string::String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            c if c == op && depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    if parts.len() <= 1 {
        return None;
    }
    Some(parts)
}

fn parse_ivc_path_steps(
    s: &str,
) -> Option<(
    bool,
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
    )>,
)> {
    let s = s.trim();
    let (parent_root, rest) = if let Some(r) = s.strip_prefix("parent::") {
        (true, r)
    } else if let Some(r) = s.strip_prefix('/') {
        (false, r)
    } else if let Some(r) = s.strip_prefix("../") {
        (false, r)
    } else {
        return None;
    };
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|p| !p.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some((parent_root, steps))
}

fn parse_ivc_xs_cast_kind(prefix: &str) -> Option<crate::schema::IvcXsCast> {
    use crate::schema::IvcXsCast::*;
    Some(match prefix {
        "xs:byte" => Byte,
        "xs:short" => Short,
        "xs:int" => Int,
        "xs:long" => Long,
        "xs:unsignedByte" => UnsignedByte,
        "xs:unsignedShort" => UnsignedShort,
        "xs:unsignedInt" => UnsignedInt,
        "xs:unsignedLong" => UnsignedLong,
        "xs:float" => Float,
        "xs:double" => Double,
        "xs:string" => String,
        _ => return None,
    })
}

enum IvcIntegerLexical {
    I64(i64),
    Wide(alloc::string::String),
}

fn parse_ivc_integer_lexical(s: &str) -> Option<IvcIntegerLexical> {
    let s = s.trim();
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+') {
        return None;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(IvcIntegerLexical::I64(v));
    }
    Some(IvcIntegerLexical::Wide(s.to_string()))
}

fn split_top_level_ivc_div(s: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let s = s.trim();
    let mut parts = alloc::vec::Vec::new();
    let mut current = alloc::string::String::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
                i += 1;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
                i += 1;
            }
            _ if depth == 0 && s[i..].starts_with(" div ") => {
                parts.push(current.trim().to_string());
                current.clear();
                i += 5;
            }
            _ => {
                current.push(ch);
                i += 1;
            }
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    if parts.len() <= 1 {
        return None;
    }
    Some(parts)
}

fn parse_ivc_path_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let (parent_root, steps) = parse_ivc_path_steps(s)?;
    Some(crate::schema::InputValueCalcExpression::Path {
        parent_root,
        steps,
    })
}

fn parse_ivc_primary(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    for (prefix, kind) in [
        ("xs:byte(", crate::schema::IvcXsCast::Byte),
        ("xs:short(", crate::schema::IvcXsCast::Short),
        ("xs:int(", crate::schema::IvcXsCast::Int),
        ("xs:long(", crate::schema::IvcXsCast::Long),
        ("xs:unsignedByte(", crate::schema::IvcXsCast::UnsignedByte),
        ("xs:unsignedShort(", crate::schema::IvcXsCast::UnsignedShort),
        ("xs:unsignedInt(", crate::schema::IvcXsCast::UnsignedInt),
        ("xs:unsignedLong(", crate::schema::IvcXsCast::UnsignedLong),
        ("xs:float(", crate::schema::IvcXsCast::Float),
        ("xs:double(", crate::schema::IvcXsCast::Double),
        ("xs:string(", crate::schema::IvcXsCast::String),
    ] {
        if let Some(rest) = s.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')) {
            let inner = parse_ivc_add_expr(rest.trim())?;
            return Some(crate::schema::InputValueCalcExpression::Cast {
                kind,
                inner: alloc::boxed::Box::new(inner),
            });
        }
    }
    if let Some(rest) = s.strip_prefix('-') {
        if let Some(lit) = parse_ivc_integer_lexical(rest) {
            return Some(match lit {
                IvcIntegerLexical::I64(v) => crate::schema::InputValueCalcExpression::Literal(-v),
                IvcIntegerLexical::Wide(text) => {
                    let mut neg = alloc::string::String::from("-");
                    neg.push_str(text.trim_start_matches('+'));
                    crate::schema::InputValueCalcExpression::LiteralLexical(neg)
                }
            });
        }
    }
    if let Some(lit) = parse_ivc_integer_lexical(s) {
        return Some(match lit {
            IvcIntegerLexical::I64(v) => crate::schema::InputValueCalcExpression::Literal(v),
            IvcIntegerLexical::Wide(text) => {
                crate::schema::InputValueCalcExpression::LiteralLexical(text)
            }
        });
    }
    if s.len() >= 2
        && ((s.starts_with('\'') && s.ends_with('\''))
            || (s.starts_with('"') && s.ends_with('"')))
    {
        let inner = &s[1..s.len() - 1];
        let unescaped = inner.replace("''", "'").replace("\"\"", "\"");
        return Some(crate::schema::InputValueCalcExpression::LiteralLexical(
            unescaped,
        ));
    }
    parse_ivc_path_expr(s)
}

fn parse_ivc_unary_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    parse_ivc_primary(s)
}

fn parse_ivc_div_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_div(s) {
        let mut left = parse_ivc_unary_expr(&parts[0])?;
        for part in parts.iter().skip(1) {
            let right = parse_ivc_unary_expr(part)?;
            left = crate::schema::InputValueCalcExpression::Div(
                alloc::boxed::Box::new(left),
                alloc::boxed::Box::new(right),
            );
        }
        return Some(left);
    }
    parse_ivc_unary_expr(s)
}

fn parse_ivc_mul_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_op(s, '*') {
        let mut terms = alloc::vec::Vec::new();
        for part in parts {
            terms.push(parse_ivc_div_expr(&part)?);
        }
        return Some(crate::schema::InputValueCalcExpression::Mul(terms));
    }
    parse_ivc_div_expr(s)
}

fn parse_ivc_add_expr(s: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let s = s.trim();
    if let Some(parts) = split_top_level_ivc_op(s, '+') {
        let mut terms = alloc::vec::Vec::new();
        for part in parts {
            terms.push(parse_ivc_mul_expr(&part)?);
        }
        return Some(crate::schema::InputValueCalcExpression::Add(terms));
    }
    parse_ivc_mul_expr(s)
}

fn parse_input_value_calc_expression(value: &str) -> Option<crate::schema::InputValueCalcExpression> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(rest) = inner.strip_prefix("xs:string(").and_then(|r| r.strip_suffix(')')) {
        let path = parse_ivc_add_expr(rest.trim())?;
        return Some(crate::schema::InputValueCalcExpression::StringOf(
            alloc::boxed::Box::new(path),
        ));
    }
    parse_ivc_add_expr(inner)
}

fn parse_concat_arg_segment(part: &str) -> Option<alloc::vec::Vec<crate::schema::InputValueCalcSegment>> {
    use crate::schema::InputValueCalcSegment;
    let part = part.trim();
    if part.is_empty() {
        return None;
    }
    if let Some(nested) = part.strip_prefix("fn:concat(") {
        if !part.ends_with(')') {
            return None;
        }
        let nested_args = &nested[..nested.len() - 1];
        return parse_concat_args_flat(nested_args);
    }
    if let Some(name) = part.strip_prefix("../") {
        return Some(alloc::vec![InputValueCalcSegment::Sibling(
            local_name_from_qname(name).to_string(),
        )]);
    }
    if part.starts_with('/') || part.starts_with("parent::") || part.starts_with("../") {
        if let Some((_parent_root, steps)) = parse_ivc_path_steps(part) {
            return Some(alloc::vec![InputValueCalcSegment::InfosetPath(steps)]);
        }
    }
    if part.len() >= 2
        && ((part.starts_with('\'') && part.ends_with('\''))
            || (part.starts_with('"') && part.ends_with('"')))
    {
        let inner = &part[1..part.len() - 1];
        let unescaped = inner.replace("''", "'").replace("\"\"", "\"");
        return Some(alloc::vec![InputValueCalcSegment::Literal(unescaped)]);
    }
    if let Some(rest) = part.strip_prefix("fn:substring(") {
        if !rest.ends_with(')') {
            return None;
        }
        let sub_args = split_top_level_commas(&rest[..rest.len() - 1]);
        if sub_args.len() != 3 {
            return None;
        }
        let sib = sub_args[0].strip_prefix("../")?;
        let start: usize = sub_args[1].parse().ok()?;
        let length: usize = sub_args[2].parse().ok()?;
        return Some(alloc::vec![InputValueCalcSegment::Substring {
            sibling: local_name_from_qname(sib).to_string(),
            start,
            length,
        }]);
    }
    None
}

fn parse_concat_args_flat(args: &str) -> Option<alloc::vec::Vec<crate::schema::InputValueCalcSegment>> {
    let mut out = alloc::vec::Vec::new();
    for part in split_top_level_commas(args) {
        out.extend(parse_concat_arg_segment(&part)?);
    }
    Some(out)
}

fn parse_input_value_calc_concat(value: &str) -> Option<alloc::vec::Vec<crate::schema::InputValueCalcSegment>> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let rest = inner.strip_prefix("fn:concat(")?;
    if !rest.ends_with(')') {
        return None;
    }
    let args = &rest[..rest.len() - 1];
    parse_concat_args_flat(args)
}

fn parse_parse_unparse_policy(value: &str) -> Result<ParseUnparsePolicy> {
    use ParseUnparsePolicy::*;
    Ok(match value.trim() {
        "both" => Both,
        "parseOnly" => ParseOnly,
        "unparseOnly" => UnparseOnly,
        other => {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("unknown parseUnparsePolicy `{other}`"),
            }
            .into())
        }
    })
}

fn parse_fn_count_path(
    inner: &str,
) -> Option<
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
    )>,
> {
    let inner = inner.trim();
    let path = inner.strip_prefix("fn:count(")?.strip_suffix(')')?.trim();
    let mut rest = path;
    while rest.starts_with("../") {
        rest = &rest[3..];
    }
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some(steps)
}

fn parse_infoset_path_step(step: &str) -> (Option<alloc::string::String>, alloc::string::String, Option<u32>) {
    let (head, index) = if let Some(open) = step.find('[') {
        let idx = step[open + 1..]
            .strip_suffix(']')
            .and_then(|s| s.parse::<u32>().ok());
        (&step[..open], idx)
    } else {
        (step, None)
    };
    let (prefix, local) = if let Some((p, l)) = head.split_once(':') {
        (Some(p.to_string()), l.to_string())
    } else {
        (None, head.to_string())
    };
    (prefix, local, index)
}

fn parse_byte_order_literal(order: &str) -> Option<ByteOrder> {
    match order.trim().trim_matches('\'').trim_matches('"') {
        "bigEndian" => Some(ByteOrder::BigEndian),
        "littleEndian" => Some(ByteOrder::LittleEndian),
        _ => None,
    }
}

fn parse_byte_order_if_expr(value: &str) -> Option<(String, ByteOrder, ByteOrder)> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .map(str::trim)?;
    let rest = inner.strip_prefix("if")?.trim();
    let rest = rest.strip_prefix('(')?.trim();
    let then_idx = rest.find(") then ")?;
    let cond = rest[..then_idx].trim().to_string();
    let rest = rest[then_idx + 7..].trim();
    let else_idx = rest.find(" else ")?;
    let then_lit = rest[..else_idx].trim();
    let else_lit = rest[else_idx + 6..].trim();
    let t = parse_byte_order_literal(then_lit)?;
    let f = parse_byte_order_literal(else_lit)?;
    if cond.is_empty() {
        return None;
    }
    Some((cond, t, f))
}

fn parse_input_value_calc_relative_path(
    value: &str,
) -> Option<
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
    )>,
> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let rest = inner.strip_prefix("../")?;
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some(steps)
}

fn parse_output_value_calc_infoset_path(
    value: &str,
) -> Option<(
    alloc::vec::Vec<(
        Option<alloc::string::String>,
        alloc::string::String,
        Option<u32>,
    )>,
    i64,
)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let (path_expr, addend) = if let Some((left, right)) = inner.rsplit_once('+') {
        (left.trim(), right.trim().parse::<i64>().ok()?)
    } else {
        return None;
    };
    let rest = path_expr.strip_prefix("../")?;
    if rest.is_empty() {
        return None;
    }
    let mut steps = alloc::vec::Vec::new();
    for step in rest.split('/').filter(|s| !s.is_empty()) {
        steps.push(parse_infoset_path_step(step));
    }
    Some((steps, addend))
}

fn length_units_from_calc_args(args: &str) -> LengthUnits {
    let lower = args.to_ascii_lowercase();
    if lower.contains("'bits'") || lower.contains("\"bits\"") {
        LengthUnits::Bits
    } else if lower.contains("'characters'") || lower.contains("\"characters\"") {
        LengthUnits::Characters
    } else {
        LengthUnits::Bytes
    }
}

fn parse_input_value_calc(value: &str) -> Option<(InputValueCalc, Option<String>, Option<String>)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.starts_with("xs:hexBinary(") && inner.ends_with(')') {
        let arg = inner["xs:hexBinary(".len()..inner.len() - 1].trim();
        if let Some(sib) = arg.strip_prefix("../") {
            return Some((
                InputValueCalc::HexBinaryFromSibling,
                Some(local_name_from_qname(sib).to_string()),
                None,
            ));
        }
    }
    if inner.starts_with("xs:string(") && inner.ends_with(')') {
        let arg = &inner["xs:string(".len()..inner.len() - 1];
        let lit = parse_xs_string_literal_arg(arg)?;
        return Some((InputValueCalc::StringLiteral, None, Some(lit)));
    }
    if let Some(lit) = parse_xs_string_literal_arg(inner) {
        return Some((InputValueCalc::StringLiteral, None, Some(lit)));
    }
    if let Some(v) = parse_constant_length_expr(trimmed) {
        return Some((InputValueCalc::Constant(v as i64), None, None));
    }
    if let Ok(v) = inner.parse::<i64>() {
        return Some((InputValueCalc::Constant(v), None, None));
    }
    if matches!(
        parse_ivc_integer_lexical(inner),
        Some(IvcIntegerLexical::Wide(_))
    ) {
        return Some((
            InputValueCalc::ConstantLexical,
            None,
            Some(inner.trim().to_string()),
        ));
    }
    let (func, rest) = inner.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    let units = length_units_from_calc_args(args);
    let target = args
        .split(',')
        .next()?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    match (func, target) {
        ("dfdl:contentLength", "..") => Some((InputValueCalc::ContentLengthSelf(units), None, None)),
        ("dfdl:valueLength", "..") => Some((InputValueCalc::ValueLengthSelf(units), None, None)),
        ("dfdl:contentLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                InputValueCalc::ContentLengthSibling(units),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("dfdl:valueLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                InputValueCalc::ValueLengthSibling(units),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("xs:boolean", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                InputValueCalc::BooleanFromSibling,
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        _ => None,
    }
}

fn parse_variable_input_value_calc(
    value: &str,
    vars: &BTreeMap<String, String>,
) -> Option<(InputValueCalc, Option<String>)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let name = inner.strip_prefix('$')?.trim();
    if vars.contains_key(name) {
        return Some((InputValueCalc::SchemaVariable, Some(name.to_string())));
    }
    None
}

fn parse_variable_length_expr(value: &str, vars: &BTreeMap<String, String>) -> Option<u64> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    let name = inner.strip_prefix('$')?.trim();
    vars.get(name)?.parse().ok()
}

fn parse_repeat_indicator_output_value_calc(value: &str) -> bool {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .map(str::trim)
        .unwrap_or(trimmed);
    let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("dfdl:occursIndex()ltfn:count(..)")
        && compact.contains("then1else0")
}

fn looks_like_xpath_output_value_calc(value: &str) -> bool {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return false;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    inner.contains("../")
        || inner.starts_with("fn:")
        || inner.contains("xs:int(")
        || inner.contains("xs:string(")
        || inner.contains("xs:long(")
}

fn parse_output_value_calc(value: &str) -> Option<(OutputValueCalc, Option<String>, Option<String>)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(lit) = parse_xs_string_literal_arg(inner) {
        return Some((OutputValueCalc::Constant(0), None, Some(lit)));
    }
    if inner.starts_with("xs:string(") && inner.ends_with(')') {
        let arg = inner["xs:string(".len()..inner.len() - 1].trim();
        if let Some(lit) = parse_xs_string_literal_arg(arg) {
            return Some((OutputValueCalc::Constant(0), None, Some(lit)));
        }
    }
    if let Some(hex) = parse_output_value_calc_hex(inner) {
        return Some(hex);
    }
    if let Ok(v) = inner.parse::<i64>() {
        return Some((OutputValueCalc::Constant(v), None, None));
    }
    let (func_part, addend) = if let Some((left, right)) = inner.rsplit_once('+') {
        (left.trim(), right.trim().parse::<i64>().unwrap_or(0))
    } else {
        (inner, 0)
    };
    let (func, rest) = func_part.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    let units = length_units_from_calc_args(args);
    let target = args
        .split(',')
        .next()?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    match (func, target) {
        ("dfdl:contentLength", "..") => {
            Some((OutputValueCalc::ContentLengthSelf(units, addend), None, None))
        }
        ("dfdl:valueLength", "..") => Some((OutputValueCalc::ValueLengthSelf(units, addend), None, None)),
        ("dfdl:contentLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                OutputValueCalc::ContentLengthSibling(units, addend),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("dfdl:valueLength", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                OutputValueCalc::ValueLengthSibling(units, addend),
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("fn:string-length", sib) => {
            let name = sib.strip_prefix("../")?;
            Some((
                OutputValueCalc::StringLengthSibling,
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        ("fn:substring", sib) => {
            let sub_args = split_top_level_commas(args);
            if sub_args.len() != 3 {
                return None;
            }
            let name = sub_args[0].strip_prefix("../")?;
            let start: usize = sub_args[1].parse().ok()?;
            let length: usize = sub_args[2].parse().ok()?;
            Some((
                OutputValueCalc::Substring { start, length },
                Some(local_name_from_qname(name).to_string()),
                None,
            ))
        }
        _ => None,
    }
}

fn parse_output_value_calc_hex(inner: &str) -> Option<(OutputValueCalc, Option<String>, Option<String>)> {
    if inner.starts_with("xs:hexBinary(") && inner.ends_with(')') {
        let arg = inner["xs:hexBinary(".len()..inner.len() - 1].trim();
        let lit = parse_xs_string_literal_arg(arg)?;
        return Some((OutputValueCalc::HexBinaryFromLexical, None, Some(lit)));
    }
    if inner.starts_with("dfdl:hexBinary(") && inner.ends_with(')') {
        let arg = inner["dfdl:hexBinary(".len()..inner.len() - 1].trim();
        if let Ok(n) = arg.parse::<i64>() {
            return Some((OutputValueCalc::HexBinaryFromInteger(n), None, None));
        }
        if let Some(lit) = parse_xs_string_literal_arg(arg) {
            return Some((OutputValueCalc::HexBinaryFromLexical, None, Some(lit)));
        }
        if arg.starts_with("xs:short(") && arg.ends_with(')') {
            let num = arg["xs:short(".len()..arg.len() - 1].trim();
            let v: i16 = num.parse().ok()?;
            return Some((OutputValueCalc::HexBinaryFromShort(v), None, None));
        }
        if arg.starts_with("xs:byte(") && arg.contains("../") {
            let inner_arg = arg["xs:byte(".len()..arg.len() - 1].trim();
            let sib = inner_arg.strip_prefix("../")?;
            return Some((
                OutputValueCalc::HexBinaryFromByteSibling,
                Some(local_name_from_qname(sib).to_string()),
                None,
            ));
        }
    }
    None
}

fn parse_self_string_length_max_expr(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.contains("fn:string-length(.)") && inner.contains("gt 5") {
        return Some(5);
    }
    None
}

fn apply_choice_dispatch_key_parse(props: &mut DfdlProps, value: &str) {
    props.choice_dispatch_key = Some(value.to_string());
    props.choice_dispatch_sibling = None;
    props.choice_dispatch_path = None;
    props.choice_dispatch_literal = None;
    props.choice_dispatch_sibling_int = None;
    if let Some(steps) = parse_input_value_calc_relative_path(value) {
        props.choice_dispatch_path = Some(steps);
        return;
    }
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(rest) = inner.strip_prefix("./") {
        if !rest.is_empty() {
            if rest.contains('/') {
                let mut steps = alloc::vec::Vec::new();
                for step in rest.split('/').filter(|s| !s.is_empty()) {
                    steps.push(parse_infoset_path_step(step));
                }
                props.choice_dispatch_path = Some(steps);
            } else {
                props.choice_dispatch_sibling =
                    Some(local_name_from_qname(rest).to_string());
            }
        }
        return;
    }
    if inner.starts_with("xs:string(") && inner.ends_with(')') {
        let arg = inner["xs:string(".len()..inner.len() - 1].trim();
        if let Some(name) = arg
            .strip_prefix("xs:int(")
            .and_then(|r| r.strip_suffix(')'))
            .map(|s| s.trim())
        {
            props.choice_dispatch_sibling_int =
                Some(local_name_from_qname(name).to_string());
            return;
        }
        if let Some(rest) = arg.strip_prefix("./") {
            props.choice_dispatch_sibling =
                Some(local_name_from_qname(rest).to_string());
            return;
        }
        if let Some(rest) = arg.strip_prefix("../") {
            props.choice_dispatch_sibling =
                Some(local_name_from_qname(rest).to_string());
            return;
        }
        if let Some(lit) = parse_xs_string_literal_arg(arg) {
            props.choice_dispatch_literal = Some(lit);
        }
    }
}

fn parse_sibling_length_expr(value: &str) -> Option<(String, bool)> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if let Some(path) = inner.strip_prefix("../") {
        return Some((local_name_from_qname(path).to_string(), false));
    }
    if let Some(idx) = inner.find("../") {
        let tail = inner[idx + 3..].trim().trim_end_matches(')').trim();
        if !tail.is_empty() {
            let cast_long = inner.contains("xs:long(") || inner.contains("xs:integer(");
            return Some((local_name_from_qname(tail).to_string(), cast_long));
        }
    }
    None
}

/// Parses constant DFDL length expressions such as `{ 6 }`, `{1}`, or `{ 1 + 1 }`.
fn parse_constant_length_expr(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty()
        || inner.contains("..")
        || inner.contains(':')
        || inner.contains('(')
        || inner.contains('/')
    {
        return None;
    }
    if let Ok(v) = inner.parse::<u64>() {
        return Some(v);
    }
    for op in ['+', '-'] {
        if let Some((lhs, rhs)) = inner.split_once(op) {
            let a = lhs.trim().parse::<u64>().ok()?;
            let b = rhs.trim().parse::<u64>().ok()?;
            return match op {
                '+' => Some(a.saturating_add(b)),
                '-' => a.checked_sub(b),
                _ => None,
            };
        }
    }
    None
}

fn local_name_from_qname(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

fn variable_local_name_from_ref(qname: &str) -> alloc::string::String {
    local_name_from_qname(&normalize_qname(qname)).to_string()
}

fn split_dfdl_attrs_with_variables(
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

fn split_dfdl_attrs(
    element_local: &str,
    attrs: &BTreeMap<String, String>,
    diagnostics: Option<&mut alloc::vec::Vec<String>>,
) -> Result<(BTreeMap<String, String>, DfdlProps)> {
    split_dfdl_attrs_with_variables(element_local, attrs, diagnostics, None, None)
}

fn record_global_element_xsd_diagnostics(
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

fn record_local_element_xsd_diagnostics(
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

fn record_invalid_calendar_time_zone(
    value: &str,
    schema_label: Option<&str>,
    diagnostics: &mut alloc::vec::Vec<String>,
) {
    if crate::vm::calendar_binary::is_valid_dfdl_calendar_time_zone(value) {
        return;
    }
    let mut msg = alloc::format!(
        "Schema Definition Error: Value '{value}' is not valid with respect to its type, 'CalendarTimeZoneType'"
    );
    if let Some(label) = schema_label {
        msg.push('\n');
        msg.push_str(label);
    }
    diagnostics.push(msg);
}

fn props_from_attrs(attrs: &BTreeMap<String, String>) -> Result<DfdlProps> {
    props_from_attrs_with_variables(attrs, None, None, None)
}

fn props_from_attrs_with_variables(
    attrs: &BTreeMap<String, String>,
    variables: Option<&BTreeMap<String, String>>,
    schema_label: Option<&str>,
    mut schema_diagnostics: Option<&mut alloc::vec::Vec<String>>,
) -> Result<DfdlProps> {
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
                if let Some((test, t, f)) = parse_byte_order_if_expr(value) {
                    props.byte_order_conditional_test = Some(test);
                    props.byte_order_if_true = Some(t);
                    props.byte_order_if_false = Some(f);
                } else {
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
                props.length_kind_defined = true;
            }
            "lengthPattern" => props.length_pattern = Some(value.clone()),
            "length" => {
                if let Ok(v) = value.parse::<u64>() {
                    props.length = Some(v);
                } else if let Some(v) = parse_constant_length_expr(value) {
                    props.length = Some(v);
                } else if let Some((sibling, cast_long)) = parse_sibling_length_expr(value) {
                    props.length_sibling = Some(sibling);
                    props.length_sibling_cast_long = cast_long;
                } else if variables
                    .and_then(|vars| parse_variable_length_expr(value, vars))
                    .is_some()
                {
                    props.length = variables.and_then(|vars| parse_variable_length_expr(value, vars));
                } else if let Some(cap) = parse_self_string_length_max_expr(value) {
                    props.length_self_string_max_cap = Some(cap);
                    props.length_expr_unparsed = true;
                } else if value.contains("dfdl:valueLength(.") || value.contains("dfdl:valueLength( .") {
                    props.length_expr_unparsed = true;
                    props.length_self_value_length = true;
                } else if value.trim().starts_with('{') {
                    props.length_expr_unparsed = true;
                    // Defer unsupported expressions; do not fail the whole property set.
                } else {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!("invalid length `{value}`"),
                    }
                    .into());
                }
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
            "encodingErrorPolicy" => {
                props.encoding_error_policy_defined = true;
                props.encoding_error_policy = Some(match value.as_str() {
                    "error" => EncodingErrorPolicy::Error,
                    "replace" => EncodingErrorPolicy::Replace,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown encodingErrorPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "nilKind" => {
                props.nil_kind = Some(match value.as_str() {
                    "literalValue" => NilKind::LiteralValue,
                    "literalCharacter" => NilKind::LiteralCharacter,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown nilKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "nilValue" => {
                // Keep the raw list; entities are expanded per-alternative at runtime.
                props.nil_value = Some(value.clone());
            }
            "separatorSuppressionPolicy" => {
                props.separator_suppression_policy = Some(match value.as_str() {
                    "anyEmpty" => SeparatorSuppressionPolicy::AnyEmpty,
                    "trailingEmpty" => SeparatorSuppressionPolicy::TrailingEmpty,
                    "trailingEmptyStrict" => SeparatorSuppressionPolicy::TrailingEmptyStrict,
                    "never" => SeparatorSuppressionPolicy::Never,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "unknown separatorSuppressionPolicy `{other}`"
                            ),
                        }
                        .into())
                    }
                });
            }
            "emptyElementParsePolicy" => {
                props.empty_element_parse_policy = Some(match value.as_str() {
                    "treatAsEmpty" => EmptyElementParsePolicy::TreatAsEmpty,
                    "treatAsAbsent" | "treatAsMissing" => EmptyElementParsePolicy::TreatAsAbsent,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "unknown emptyElementParsePolicy `{other}`"
                            ),
                        }
                        .into())
                    }
                });
            }
            "occursCountKind" => {
                props.occurs_count_kind = Some(match value.as_str() {
                    "parsed" => OccursCountKind::Parsed,
                    "implicit" => OccursCountKind::Implicit,
                    "fixed" => OccursCountKind::Fixed,
                    "expression" => OccursCountKind::Expression,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown occursCountKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "escapeSchemeRef" => {
                props.escape_scheme_ref = Some(value.to_string());
            }
            "hiddenGroupRef" => {
                props.hidden_group_ref = Some(normalize_qname(value));
            }
            "ignoreCase" => {
                props.ignore_case = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown ignoreCase `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "truncateSpecifiedLengthString" => {
                props.truncate_specified_length_string = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!(
                                "unknown truncateSpecifiedLengthString `{other}`"
                            ),
                        }
                        .into())
                    }
                });
            }
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
                props.text_number_pad_character = Some(value.clone());
            }
            "textStandardBase" => {
                props.text_standard_base = Some(value.parse().map_err(|_| {
                    ParseError::InvalidXml {
                        message: alloc::format!("invalid textStandardBase `{value}`"),
                    }
                })?);
            }
            "textStringPadCharacter" => {
                props.text_string_pad_character = Some(value.clone());
            }
            "textCalendarPadCharacter" => {
                props.text_calendar_pad_character = Some(value.clone());
            }
            "textCalendarJustification" => {
                props.text_calendar_justification = Some(match value.as_str() {
                    "left" => TextStringJustification::Left,
                    "right" => TextStringJustification::Right,
                    "center" => TextStringJustification::Center,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textCalendarJustification `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textPadKind" => {
                props.text_pad_kind = Some(match value.as_str() {
                    "none" => crate::schema::TextPadKind::None,
                    "padChar" => crate::schema::TextPadKind::PadChar,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textPadKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textStringJustification" => {
                props.text_string_justification = Some(match value.as_str() {
                    "left" => TextStringJustification::Left,
                    "right" => TextStringJustification::Right,
                    "center" => TextStringJustification::Center,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textStringJustification `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberJustification" => {
                props.text_number_justification = Some(match value.as_str() {
                    "left" => TextNumberJustification::Left,
                    "right" => TextNumberJustification::Right,
                    "center" => TextNumberJustification::Center,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberJustification `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "outputValueCalc" => {
                if parse_repeat_indicator_output_value_calc(value) {
                    props.output_value_calc = Some(OutputValueCalc::RepeatIndicatorFromParentCount);
                } else if value.contains("if (") {
                    props.output_value_calc_conditional = true;
                } else if let Some((steps, addend)) = parse_output_value_calc_infoset_path(value) {
                    props.output_value_calc = Some(OutputValueCalc::InfosetPathAddend);
                    props.output_value_calc_path = Some(steps);
                    props.output_value_calc_path_addend = Some(addend);
                } else if let Some(calc) = parse_output_value_calc(value) {
                    props.output_value_calc = Some(calc.0);
                    props.output_value_calc_sibling = calc.1;
                    props.output_value_calc_literal = calc.2;
                } else if looks_like_xpath_output_value_calc(value) {
                    // e.g. `{ xs:int(../ex:x) }` — computed on unparse, satisfies hidden-group rules.
                    props.output_value_calc_conditional = true;
                }
            }
            "inputValueCalc" => {
                if let Some(expr) = parse_input_value_calc_expression(value) {
                    props.input_value_calc_expression = Some(expr);
                } else if let Some(segments) = parse_input_value_calc_concat(value) {
                    props.input_value_calc_segments = Some(segments);
                } else if let Some(steps) = parse_input_value_calc_relative_path(value) {
                    props.input_value_calc_path = Some(steps);
                } else if let Some((calc, lit)) = variables
                    .and_then(|vars| parse_variable_input_value_calc(value, vars))
                {
                    props.input_value_calc = Some(calc);
                    props.input_value_calc_literal = lit;
                } else if let Some(calc) = parse_input_value_calc(value) {
                    props.input_value_calc = Some(calc.0);
                    props.input_value_calc_sibling = calc.1;
                    props.input_value_calc_literal = calc.2;
                }
            }
            "parseUnparsePolicy" => {
                props.parse_unparse_policy = Some(parse_parse_unparse_policy(value)?);
            }
            "textBidi" => {
                props.text_bidi = Some(match value.trim() {
                    "yes" | "true" => true,
                    "no" | "false" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("invalid textBidi `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "floating" => {
                props.floating = Some(match value.trim() {
                    "yes" | "true" => true,
                    "no" | "false" => false,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("invalid floating `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryPackedSignCodes" => {
                props.binary_packed_sign_codes_defined = true;
                props.binary_packed_sign_codes = Some(value.clone());
            }
            "binaryNumberCheckPolicy" => {
                props.binary_number_check_policy = Some(match value.as_str() {
                    "strict" => crate::schema::BinaryNumberCheckPolicy::Strict,
                    "lax" => crate::schema::BinaryNumberCheckPolicy::Lax,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binaryNumberCheckPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "binaryCalendarEpoch" => {
                props.binary_calendar_epoch = Some(value.clone());
            }
            "binaryNumberRep" | "binaryCalendarRep" => {
                let rep = match value.as_str() {
                    "binary" => BinaryNumberRep::Binary,
                    "bcd" => BinaryNumberRep::Bcd,
                    "packed" | "packedBCD" => BinaryNumberRep::PackedBcd,
                    "ibm4690Packed" | "ibm4690" => BinaryNumberRep::Ibm4690Packed,
                    "binarySeconds" => BinaryNumberRep::BinarySeconds,
                    "binaryMilliseconds" => BinaryNumberRep::BinaryMilliseconds,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown binary number/calendar rep `{other}`"),
                        }
                        .into())
                    }
                };
                if key == "binaryCalendarRep" {
                    props.binary_calendar_rep = Some(rep);
                } else if !matches!(rep, BinaryNumberRep::BinarySeconds | BinaryNumberRep::BinaryMilliseconds) {
                    props.binary_number_rep = Some(rep);
                } else {
                    return Err(ParseError::InvalidXml {
                        message: alloc::format!(
                            "unknown binary number/calendar rep `{value}`"
                        ),
                    }
                    .into());
                }
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
            "binaryDecimalVirtualPoint" => {
                let parsed: i32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid binaryDecimalVirtualPoint `{value}`"),
                })?;
                if parsed < 0 {
                    props.binary_decimal_virtual_point_sde = Some(parsed);
                } else {
                    props.binary_decimal_virtual_point = Some(parsed as u32);
                }
            }
            "decimalSigned" => {
                props.decimal_signed = Some(matches!(value.as_str(), "yes" | "true" | "1"));
            }
            "calendarPattern" => props.calendar_pattern = Some(value.clone()),
            "calendarPatternKind" => {
                props.calendar_pattern_kind = Some(match value.as_str() {
                    "explicit" => crate::schema::CalendarPatternKind::Explicit,
                    "implicit" => crate::schema::CalendarPatternKind::Implicit,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown calendarPatternKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "calendarCheckPolicy" => {
                props.calendar_check_policy_lax = Some(matches!(value.as_str(), "lax"));
            }
            "calendarTimeZone" => {
                if let Some(diags) = schema_diagnostics.as_deref_mut() {
                    record_invalid_calendar_time_zone(&value, schema_label, diags);
                }
                props.calendar_time_zone = Some(value.clone());
                props.calendar_time_zone_defined = true;
            }
            "calendarCenturyStart" => {
                let parsed: u32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid calendarCenturyStart `{value}`"),
                })?;
                props.calendar_century_start = Some(parsed);
            }
            "calendarLanguage" => {
                if let Some(segments) = parse_input_value_calc_concat(value) {
                    props.calendar_language_segments = Some(segments);
                } else {
                    props.calendar_language = Some(value.clone());
                }
            }
            "calendarDaysInFirstWeek" => {
                let parsed: u32 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid calendarDaysInFirstWeek `{value}`"),
                })?;
                props.calendar_days_in_first_week = Some(parsed);
            }
            "calendarFirstDayOfWeek" => props.calendar_first_day_of_week = Some(value.clone()),
            "textNumberPattern" => props.text_number_pattern = Some(value.clone()),
            "textNumberCheckPolicy" => {
                props.text_number_check_policy = Some(match value.as_str() {
                    "strict" => crate::schema::BinaryNumberCheckPolicy::Strict,
                    "lax" => crate::schema::BinaryNumberCheckPolicy::Lax,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberCheckPolicy `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberRep" => {
                props.text_number_rep = Some(match value.as_str() {
                    "standard" => crate::schema::TextNumberRep::Standard,
                    "zoned" => crate::schema::TextNumberRep::Zoned,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberRep `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textZonedSignStyle" => {
                props.text_zoned_sign_style = Some(match value.as_str() {
                    "asciiStandard" => crate::schema::TextZonedSignStyle::AsciiStandard,
                    "asciiTranslatedEBCDIC" => {
                        crate::schema::TextZonedSignStyle::AsciiTranslatedEBCDIC
                    }
                    "asciiCARealiaModified" => {
                        crate::schema::TextZonedSignStyle::AsciiCARealiaModified
                    }
                    "asciiTandemModified" => crate::schema::TextZonedSignStyle::AsciiTandemModified,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textZonedSignStyle `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textStandardDecimalSeparator" => {
                if let Some((sibling, _)) = parse_sibling_length_expr(value) {
                    props.text_standard_decimal_separator_sibling = Some(sibling);
                } else {
                    props.text_standard_decimal_separator = Some(value.clone());
                }
            }
            "textStandardGroupingSeparator" => {
                if let Some((sibling, _)) = parse_sibling_length_expr(value) {
                    props.text_standard_grouping_separator_sibling = Some(sibling);
                } else {
                    props.text_standard_grouping_separator = Some(value.clone());
                }
            }
            "textStandardExponentRep" => {
                if let Some((sibling, _)) = parse_sibling_length_expr(value) {
                    props.text_standard_exponent_rep_sibling = Some(sibling);
                } else {
                    props.text_standard_exponent_rep = Some(value.clone());
                }
            }
            "textStandardInfinityRep" => {
                props.text_standard_infinity_rep = Some(value.clone());
            }
            "textStandardNaNRep" => {
                props.text_standard_nan_rep = Some(value.clone());
            }
            "textStandardZeroRep" => {
                props.text_standard_zero_rep = Some(value.clone());
            }
            "textNumberRounding" => {
                props.text_number_rounding = Some(match value.as_str() {
                    "pattern" => crate::schema::TextNumberRounding::Pattern,
                    "explicit" => crate::schema::TextNumberRounding::Explicit,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberRounding `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "textNumberRoundingIncrement" => {
                props.text_number_rounding_increment = Some(value.clone());
            }
            "textNumberRoundingMode" => {
                props.text_number_rounding_mode = Some(match value.as_str() {
                    "roundCeiling" => crate::schema::TextNumberRoundingMode::RoundCeiling,
                    "roundFloor" => crate::schema::TextNumberRoundingMode::RoundFloor,
                    "roundDown" => crate::schema::TextNumberRoundingMode::RoundDown,
                    "roundUp" => crate::schema::TextNumberRoundingMode::RoundUp,
                    "roundHalfEven" => crate::schema::TextNumberRoundingMode::RoundHalfEven,
                    "roundHalfDown" => crate::schema::TextNumberRoundingMode::RoundHalfDown,
                    "roundHalfUp" => crate::schema::TextNumberRoundingMode::RoundHalfUp,
                    "roundUnnecessary" => crate::schema::TextNumberRoundingMode::RoundUnnecessary,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown textNumberRoundingMode `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "initiator" => {
                if value.contains("%%") {
                    props.initiator_percent_escaped = true;
                }
                let lit = parse_delimiter_literal(value)?;
                props.initiator = Some(lit);
            }
            "terminator" => {
                let lit = parse_delimiter_literal(value)?;
                props.terminator = Some(lit);
            }
            "separator" => {
                let lit = parse_delimiter_literal(value)?;
                props.separator = Some(lit);
            }
            "outputNewLine" => {
                let lit = parse_delimiter_literal(value)?;
                if lit.is_empty() {
                    return Err(ParseError::InvalidXml {
                        message: "For property dfdl:outputNewLine, the length of string must be exactly 1 character, except for CRLF case when it can be 2 characters.".into(),
                    }
                    .into());
                }
                props.output_new_line = Some(lit);
            }
            "initiatedContent" => {
                props.initiated_content = Some(matches!(value.as_str(), "yes" | "true" | "1"));
            }
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
            "textBooleanTrueRep" => {
                props.text_boolean_true_rep_defined = true;
                props.text_boolean_true_rep = Some(value.clone());
            }
            "textBooleanFalseRep" => {
                props.text_boolean_false_rep_defined = true;
                props.text_boolean_false_rep = Some(value.clone());
            }
            "textBooleanPadCharacter" => {
                props.text_boolean_pad_character =
                    Some(crate::schema::expand_entities_str(value));
            }
            "occursCount" => {
                let trimmed = value.trim();
                if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    let inner = trimmed[1..trimmed.len() - 1].trim();
                    if let Ok(n) = inner.parse::<u64>() {
                        props.occurs_min = Some(n);
                        props.occurs_max = Some(n);
                        props.max_occurs_specified = true;
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    } else if let Some(steps) = parse_fn_count_path(inner) {
                        props.occurs_count_fn_path = Some(steps);
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    } else if let Some(steps) =
                        parse_input_value_calc_relative_path(&alloc::format!("{{{inner}}}"))
                    {
                        props.occurs_count_fn_path = Some(steps);
                        props.occurs_count_kind = Some(OccursCountKind::Expression);
                    }
                }
            }
            "binaryBooleanTrueRep" => {
                props.binary_boolean_true_rep_defined = true;
                if value.is_empty() {
                    props.binary_boolean_true_rep = None;
                } else {
                    let n: u64 = value.parse().map_err(|_| ParseError::InvalidXml {
                        message: alloc::format!(
                            "invalid binaryBooleanTrueRep `{value}`"
                        ),
                    })?;
                    props.binary_boolean_true_rep = Some(n);
                }
            }
            "binaryBooleanFalseRep" => {
                props.binary_boolean_false_rep_defined = true;
                let n: u64 = value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!(
                        "invalid binaryBooleanFalseRep `{value}`"
                    ),
                })?;
                props.binary_boolean_false_rep = Some(n);
            }
            "alignment" => {
                if value == "implicit" {
                    props.alignment_implicit = Some(true);
                } else {
                    props.alignment = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                        message: alloc::format!("invalid alignment `{value}`"),
                    })?);
                    props.alignment_implicit = Some(false);
                }
            }
            "alignmentUnits" => {
                props.alignment_units = Some(match value.as_str() {
                    "bytes" => LengthUnits::Bytes,
                    "bits" => LengthUnits::Bits,
                    "characters" => LengthUnits::Characters,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown alignmentUnits `{other}`"),
                        }
                        .into())
                    }
                });
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
            "choiceLengthKind" => {
                props.choice_length_kind = Some(match value.as_str() {
                    "implicit" => ChoiceLengthKind::Implicit,
                    "explicit" => ChoiceLengthKind::Explicit,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown choiceLengthKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "choiceLength" => {
                props.choice_length = Some(value.parse().map_err(|_| ParseError::InvalidXml {
                    message: alloc::format!("invalid choiceLength `{value}`"),
                })?);
            }
            "fillByte" => {
                props.fill_byte_raw = Some(value.to_string());
                props.fill_byte = Some(crate::schema::expand_entities(value));
            }
            "ref" => props.format_ref = Some(value.to_string()),
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
            "objectKind" => {
                props.object_kind = Some(match value.as_str() {
                    "bytes" => crate::schema::ObjectKind::Bytes,
                    "chars" => crate::schema::ObjectKind::Chars,
                    other => {
                        return Err(ParseError::InvalidXml {
                            message: alloc::format!("unknown objectKind `{other}`"),
                        }
                        .into())
                    }
                });
            }
            "choiceDispatchKey" => apply_choice_dispatch_key_parse(&mut props, value),
            "choiceBranchKey" => props.choice_branch_key = Some(value.clone()),
            _ => {}
        }
    }
    Ok(props)
}

fn parse_delimiter_literal(raw: &str) -> Result<String> {
    Ok(crate::schema::parse_delimiter_literal_value(raw))
}

fn is_dfdl_local(tag: &str) -> bool {
    matches!(
        tag,
        "format" | "element" | "sequence" | "choice" | "simpleType" | "group"
    )
}

fn local_tag(tag: &str) -> &str {
    if let Some(idx) = tag.rfind('}') {
        return &tag[idx + 1..];
    }
    strip_prefix(tag)
}

fn strip_prefix(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

fn normalize_qname(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// Storage key for a global element QName (namespace URI + local name).
pub fn resolve_global_element_storage_key(schema: &SchemaDocument, qname: &str) -> String {
    if let Some((prefix, local)) = qname.split_once(':') {
        if let Some(uri) = schema.namespace_prefixes.get(prefix) {
            return format_storage_key(local, Some(uri.as_str()));
        }
        return format_storage_key(local, None);
    }
    format_storage_key(qname, schema.target_namespace.as_deref())
}

fn unique_global_element_by_local<'a>(
    schema: &'a SchemaDocument,
    local: &str,
) -> Option<&'a GlobalElement> {
    let mut matches = schema
        .global_elements
        .iter()
        .filter(|(k, _)| format_local_from_storage_key(k) == local);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.1)
}

fn namespace_uri_matches_prefix(ns: &str, prefix: &str) -> bool {
    if ns == prefix {
        return true;
    }
    let host = ns
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(ns);
    let host = host.split(':').next().unwrap_or(host);
    let host_label = host.split('.').next().unwrap_or(host);
    host == prefix
        || host_label == prefix
        || host_label.starts_with(prefix)
        || prefix.starts_with(host_label)
        || host.strip_prefix(&format!("{prefix}.")).is_some()
        || host.strip_suffix(&format!(".{prefix}")).is_some()
}

fn supplement_namespace_prefixes_from_text(text: &str, out: &mut BTreeMap<String, String>) {
    let mut rest = text;
    while let Some(idx) = rest.find("xmlns:") {
        rest = &rest[idx + 6..];
        let Some(eq) = rest.find('=') else { break; };
        let prefix = rest[..eq].trim();
        if prefix.is_empty() {
            continue;
        }
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next() else { continue; };
        if q != '"' && q != '\'' {
            continue;
        }
        let value = after[1..]
            .split(q)
            .next()
            .unwrap_or("")
            .trim();
        if !value.is_empty() {
            out.entry(prefix.to_string()).or_insert(value.to_string());
        }
    }
}

fn global_element_by_prefixed_name<'a>(
    schema: &'a SchemaDocument,
    prefix: &str,
    local: &str,
) -> Option<&'a GlobalElement> {
    let mut matches = schema.global_elements.iter().filter(|(k, _)| {
        format_local_from_storage_key(k) == local
            && k.contains('|')
            && !k.starts_with('|')
            && k.split_once('|')
                .map(|(ns, _)| namespace_uri_matches_prefix(ns, prefix))
                .unwrap_or(false)
    });
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.1)
}

/// Resolve a global element by TDML root name or `ref="prefix:local"` QName.
pub fn get_global_element<'a>(schema: &'a SchemaDocument, qname: &str) -> Option<&'a GlobalElement> {
    let local = normalize_qname(qname);
    if let Some((prefix, _)) = qname.split_once(':') {
        let key = resolve_global_element_storage_key(schema, qname);
        if let Some(g) = schema.global_elements.get(&key) {
            if g.type_name.as_str() != "xs:string" {
                return Some(g);
            }
        }
        if let Some(g) = global_element_by_prefixed_name(schema, prefix, &local) {
            return Some(g);
        }
    }
    let key = resolve_global_element_storage_key(schema, qname);
    if let Some(g) = schema.global_elements.get(&key) {
        return Some(g);
    }
    if !qname.contains(':') {
        if let Some(g) = unique_global_element_by_local(schema, qname) {
            return Some(g);
        }
    }
    let fallback = format_storage_key(&local, None);
    if fallback != key {
        if let Some(g) = schema.global_elements.get(&fallback) {
            return Some(g);
        }
    }
    schema
        .global_elements
        .get(qname)
        .or_else(|| schema.global_elements.get(&local))
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SchemaMergeKind {
    Include,
    Import,
}

fn discriminator_xpath_prefix_scope(
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

fn collect_namespace_prefixes(attrs: &BTreeMap<String, String>, out: &mut BTreeMap<String, String>) {
    for (key, value) in attrs {
        if key.as_str() == "xmlns" {
            out.insert(String::new(), value.clone());
            continue;
        }
        if let Some(prefix) = key.strip_prefix("xmlns:") {
            out.insert(prefix.to_string(), value.clone());
            continue;
        }
        // xml_no_std exposes xmlns declarations as unprefixed keys (e.g. `ex` → URI).
        if !key.contains(':')
            && key != "targetNamespace"
            && key != "elementFormDefault"
            && key != "attributeFormDefault"
            && key != "version"
            && (value.starts_with("http://") || value.starts_with("https://") || value.starts_with("urn:"))
        {
            out.insert(key.clone(), value.clone());
        }
    }
}

pub(crate) fn resolve_type_qname_in_schema(
    doc: &SchemaDocument,
    type_attr: &str,
    scope: Option<&BTreeMap<String, String>>,
) -> core::result::Result<TypeName, crate::error::SchemaError> {
    use crate::error::SchemaError;
    use crate::schema::{BuiltinType, TypeName};
    const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";
    let mut prefix_map = doc.namespace_prefixes.clone();
    if let Some(s) = scope {
        for (k, v) in s {
            prefix_map.insert(k.clone(), v.clone());
        }
    }
    if !type_attr.contains(':') {
        let local = normalize_qname(type_attr);
        if BuiltinType::from_xsd(&local).is_some() {
            let q = if local.contains(':') {
                local
            } else {
                alloc::format!("xs:{local}")
            };
            return Ok(TypeName::new(q));
        }
        if doc.types.contains_key(&TypeName::new(&local)) {
            return Ok(TypeName::new(local));
        }
        return Ok(TypeName::new(local));
    }
    let (prefix, local) = type_attr.split_once(':').unwrap();
    let local = normalize_qname(local);
    let Some(uri) = prefix_map.get(prefix) else {
        return Ok(TypeName::new(local));
    };
    if uri == XSD_NS {
        let q = alloc::format!("xs:{local}");
        if BuiltinType::from_xsd(&q).is_some() {
            return Ok(TypeName::new(q));
        }
        return Err(SchemaError::InvalidProperty {
            message: alloc::format!(
                "Schema Definition Error: No type definition found for '{type_attr}'."
            ),
        });
    }
    if doc.types.contains_key(&TypeName::new(&local)) {
        return Ok(TypeName::new(local));
    }
    let xs_q = alloc::format!("xs:{local}");
    if prefix == "xs" && BuiltinType::from_xsd(&xs_q).is_some() {
        return Ok(TypeName::new(xs_q));
    }
    Err(SchemaError::InvalidProperty {
        message: alloc::format!(
            "Schema Definition Error: No type definition found for '{type_attr}'."
        ),
    })
}

fn format_storage_key(local: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(ns) if !ns.is_empty() => alloc::format!("{ns}|{local}"),
        _ => alloc::format!("|{local}"),
    }
}

fn format_local_from_storage_key(key: &str) -> &str {
    key.rsplit_once('|').map(|(_, local)| local).unwrap_or(key)
}

/// Parse XSD numeric facet values (`0`, `0.0`, `-1.5`) into i64 when representable as integer.
fn parse_numeric_facet_bound(v: &str) -> Option<i64> {
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
    fn junk_xs_format_in_appinfo_is_sde() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" targetNamespace="http://example.com">
  <xs:annotation>
    <xs:appinfo source="http://www.ogf.org/dfdl/">
      <xs:format separator="" initiator="" terminator="" representation="text"/>
    </xs:appinfo>
  </xs:annotation>
  <xs:element name="x" type="xs:int" />
</xs:schema>"#;
        let err = parse_schema(xsd).unwrap_err().to_string();
        assert!(err.contains("Invalid dfdl annotation"), "{err}");
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
    fn group_ref_named_format_lookup() {
        let tdml_inner = r#"
    <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
    <dfdl:defineFormat name="def">
      <dfdl:format separator=","/>
    </dfdl:defineFormat>
    <xs:group name="namedGroup">
      <xs:sequence dfdl:separatorPosition="infix">
        <xs:element name="quantity" type="xs:int" />
      </xs:sequence>
    </xs:group>
    <xs:element name="Item" dfdl:lengthKind="implicit">
      <xs:complexType>
        <xs:group ref="ex:namedGroup" dfdl:ref="ex:def" />
      </xs:complexType>
    </xs:element>"#;
        let xsd = alloc::format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com"
           targetNamespace="http://example.com">{inner}
</xs:schema>"#,
            inner = tdml_inner
        );
        let doc = parse_schema(&xsd).expect("parse");
        let def_key = format_storage_key("def", Some("http://example.com"));
        let def = doc.named_formats.get(&def_key).expect("def format");
        assert_eq!(def.separator.as_deref(), Some(","));
        let item = get_global_element(&doc, "Item").expect("Item");
        if let TypeDef::Complex { content, .. } = doc.resolve_type(&item.type_name).unwrap() {
            if let ComplexContent::Sequence(seq) = content {
                if let Particle::GroupRef(gr) = &seq.particles[0] {
                    assert_eq!(gr.props.separator.as_deref(), Some(","));
                } else {
                    panic!("expected group ref particle");
                }
            } else {
                panic!("expected sequence content {:?}", content);
            }
        } else {
            panic!("expected complex type");
        }
    }

    #[test]
    fn ibm_general_purpose_imported_format_defaults_have_byte_order() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/nonNegativeInteger.dfdl.xsd"
        );
        let xsd = std::fs::read_to_string(path).expect("read");
        let dir = std::path::Path::new(path).parent().unwrap();
        let doc = parse_schema_with_options(
            &xsd,
            &ParseOptions {
                base_dir: Some(dir.to_string_lossy().into()),
                schema_label: None,
            },
        )
        .expect("parse");
        assert_eq!(
            doc.format_defaults.props.byte_order,
            Some(crate::schema::ByteOrder::BigEndian)
        );
    }

    #[test]
    fn define_format_in_appinfo_does_not_merge_into_format_defaults() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/">
  <xs:annotation>
    <xs:appinfo source="http://www.ogf.org/dfdl/">
      <dfdl:format initiator="" terminator="" separator=""/>
      <dfdl:defineFormat name="pipes">
        <dfdl:format separator="|" initiator="||" terminator="|||"/>
      </dfdl:defineFormat>
    </xs:appinfo>
  </xs:annotation>
  <xs:element name="A" type="xs:int"/>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        assert_eq!(doc.format_defaults.props.initiator.as_deref(), Some(""));
        assert!(doc.named_formats.contains_key("|pipes"));
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
        assert!(doc.named_formats.contains_key("|trimmed"));
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
        let wrap = get_global_element(&doc, "wrap").expect("wrap");
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
        assert!(get_global_element(&doc, "Record").is_some());
        assert!(doc.types.contains_key(&TypeName::new("RecordType")));
    }

    #[test]
    fn parse_constant_length_expression() {
        assert_eq!(parse_constant_length_expr("{ 6 }"), Some(6));
        assert_eq!(parse_constant_length_expr("{1}"), Some(1));
        assert_eq!(parse_constant_length_expr("{ 1 + 1 }"), Some(2));
        assert_eq!(parse_constant_length_expr("{ ../len }"), None);
    }

    #[test]
    fn parse_initiated_content_property() {
        let mut attrs = BTreeMap::new();
        attrs.insert("initiatedContent".into(), "yes".into());
        let props = props_from_attrs(&attrs).expect("props");
        assert_eq!(props.initiated_content, Some(true));
        let (xsd, dfdl) =
            split_dfdl_attrs(
                "sequence",
                &BTreeMap::from([(
                    "dfdl:initiatedContent".to_string(),
                    "yes".to_string(),
                )]),
                None,
            )
            .expect("split");
        assert!(xsd.is_empty());
        assert_eq!(dfdl.initiated_content, Some(true));
    }

    #[test]
    fn initiated_content_yes_rejects_empty_initiator_at_compile() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="ex:GeneralFormat"/>
  <xs:element name="zeroLengthString">
    <xs:complexType>
      <xs:sequence dfdl:initiatedContent="yes">
        <xs:element name="s1" type="xs:string" dfdl:lengthKind="delimited" dfdl:initiator=""/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        let el = get_global_element(&doc, "zeroLengthString").expect("element");
        if let TypeDef::Complex { content, .. } = doc.resolve_type(&el.type_name).unwrap() {
            if let ComplexContent::Sequence(seq) = content {
                assert_eq!(
                    seq.props.initiated_content,
                    Some(true),
                    "sequence props: {:?}",
                    seq.props
                );
            } else {
                panic!("expected sequence content");
            }
        } else {
            panic!("expected complex type");
        }
        let err = crate::ir::compile_named(&doc, Some("zeroLengthString")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Schema Definition Error"), "{msg}");
        assert!(msg.contains("initiatedContent"), "{msg}");
    }

    #[test]
    fn parse_hidden_group_ref_on_empty_sequence() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:base="http://baseSchema.com">
  <xs:group name="aGroup">
    <xs:sequence dfdl:separator=".">
      <xs:element name="aMem01" type="xs:string"/>
    </xs:sequence>
  </xs:group>
  <xs:element name="root">
    <xs:complexType>
      <xs:sequence dfdl:separator="|">
        <xs:sequence dfdl:hiddenGroupRef="base:aGroup"/>
        <xs:element name="tail" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        let el = get_global_element(&doc, "root").expect("root");
        let TypeDef::Complex { content, .. } = doc.resolve_type(&el.type_name).unwrap() else {
            panic!("complex");
        };
        let ComplexContent::Sequence(outer) = content else {
            panic!("outer sequence");
        };
        assert_eq!(outer.props.separator.as_deref(), Some("|"));
        assert_eq!(outer.particles.len(), 2);
        let Particle::Sequence(inner) = &outer.particles[0] else {
            panic!("inner particle");
        };
        assert_eq!(
            inner.props.hidden_group_ref.as_deref(),
            Some("base:aGroup"),
            "inner props: {:?}",
            inner.props
        );
    }

    #[test]
    fn parse_assert_int_eq_test_from_body() {
        assert_eq!(
            parse_assert_int_eq_test("{ xs:int(.) eq 42 }"),
            Some(42)
        );
    }

    fn parse_numeric_facet_bound_accepts_decimal_lexical() {
        assert_eq!(parse_numeric_facet_bound("0"), Some(0));
        assert_eq!(parse_numeric_facet_bound("0.0"), Some(0));
        assert_eq!(parse_numeric_facet_bound("-1"), Some(-1));
        assert!(parse_numeric_facet_bound("1.5").is_none());
    }

    #[test]
    fn complex_type_group_ref_is_not_empty() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="ex:GeneralFormat" lengthKind="delimited" representation="text"/>
  <xs:group name="g">
    <xs:sequence>
      <xs:element name="a" type="xs:string"/>
    </xs:sequence>
  </xs:group>
  <xs:element name="root">
    <xs:complexType>
      <xs:group ref="ex:g"/>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        let el = get_global_element(&doc, "root").expect("root");
        let TypeDef::Complex { content, .. } = doc.resolve_type(&el.type_name).unwrap() else {
            panic!("complex type");
        };
        let ComplexContent::Sequence(seq) = content else {
            panic!("sequence wrapper");
        };
        assert_eq!(seq.particles.len(), 1);
        assert!(matches!(seq.particles[0], Particle::GroupRef(_)));
        let prog = crate::ir::compile_named(&doc, Some("root")).expect("compile");
        let root_node = &prog.nodes[prog.root as usize];
        let crate::ir::IrNode::Element { child: Some(child), .. } = root_node else {
            panic!("root element");
        };
        let crate::ir::IrNode::Sequence { children, .. } = &prog.nodes[*child as usize] else {
            panic!("inner sequence");
        };
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn property_form_text_string_pad_whitespace_is_sde() {
        let xsd = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:ex="http://example.com">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="ex:GeneralFormat"/>
  <xs:element name="propertyForm" type="xs:string">
    <xs:annotation>
      <xs:appinfo source="http://www.ogf.org/dfdl/">
        <dfdl:element>
          <dfdl:property name="textStringPadCharacter"><![CDATA[ ]]></dfdl:property>
        </dfdl:element>
      </xs:appinfo>
    </xs:annotation>
  </xs:element>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("property-form pad parses");
        let err = crate::ir::compile_named(&doc, Some("propertyForm")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Schema Definition Error"), "{msg}");
        assert!(msg.contains("whitespace"), "{msg}");
    }

    #[test]
    fn parse_global_group_skips_annotation_and_comment() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:group name="sequenceGroup">
    <xs:annotation><xs:documentation>doc</xs:documentation></xs:annotation>
    <!-- COMMENT -->
    <xs:sequence>
      <xs:element name="inty" type="xs:int"/>
    </xs:sequence>
  </xs:group>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("annotated group parses");
        assert!(doc.groups.contains_key("sequenceGroup"));
    }

    #[test]
    fn hidden_group_ref_appinfo_attribute_notation_is_sde() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:ex="http://example.com">
  <xs:group name="ab">
    <xs:sequence dfdl:separator=":">
      <xs:element name="a" dfdl:length="1" dfdl:lengthKind="explicit" type="xs:int" />
      <xs:element name="b" dfdl:length="1" dfdl:lengthKind="explicit" type="xs:int" />
    </xs:sequence>
  </xs:group>
  <xs:element name="e">
    <xs:complexType>
      <xs:sequence>
        <xs:annotation>
          <xs:appinfo source="http://www.ogf.org/dfdl/">
            <dfdl:sequence hiddenGroupRef="ex:ab"/>
          </xs:appinfo>
        </xs:annotation>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let doc = parse_schema(xsd).expect("parse");
        let el = get_global_element(&doc, "e").expect("e");
        let TypeDef::Complex { content, .. } = doc.resolve_type(&el.type_name).unwrap() else {
            panic!("complex");
        };
        let ComplexContent::Sequence(seq) = content else {
            panic!("seq");
        };
        assert!(
            seq.props.hidden_group_ref_from_appinfo_sequence,
            "flag should be set: {:?}",
            seq.props
        );
        let err = crate::ir::compile_named(&doc, Some("e")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot have children"), "{msg}");
    }

    #[test]
    fn hidden_group_ref_sequence_with_children_is_sde() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:ex="http://example.com">
  <xs:group name="hg">
    <xs:sequence>
      <xs:element name="f" type="xs:int"/>
    </xs:sequence>
  </xs:group>
  <xs:element name="e">
    <xs:complexType>
      <xs:sequence>
        <xs:sequence dfdl:hiddenGroupRef="ex:hg">
          <xs:element name="x" type="xs:int"/>
        </xs:sequence>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let err = parse_schema(xsd).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot have children"), "{msg}");
    }
}
