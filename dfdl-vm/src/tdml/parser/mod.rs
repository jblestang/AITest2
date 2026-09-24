use crate::error::{ParseError, Result};
use crate::length_validate::DaffodilTunables;
use crate::vm::encoding::{bits_charset_spec, encode_document_text};
use crate::xml_util::{local_name_str, normalize_tdml_xml_for_parse, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// TDML `@validation` on parser test cases (Daffodil default suite value is `off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TdmlValidationMode {
    #[default]
    Off,
    Limited,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundTrip {
    /// Use the suite-level `defaultRoundTrip` attribute.
    Inherit,
    Disabled,
    OnePass,
    TwoPass,
}

/// Named TDML `<tdml:defineConfig>` (tunables + optional external variable bindings).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TdmlConfig {
    pub tunables: DaffodilTunables,
    pub external_variables: BTreeMap<String, String>,
}

/// Parsed TDML test suite.
#[derive(Debug, Clone, PartialEq)]
pub struct TdmlSuite {
    pub name: String,
    pub schemas: BTreeMap<String, TdmlSchema>,
    pub configs: BTreeMap<String, TdmlConfig>,
    pub tests: Vec<ParserTestCase>,
    pub unparser_tests: Vec<UnparserTestCase>,
    pub default_round_trip: RoundTrip,
    pub default_validation: TdmlValidationMode,
    /// Set when loading a suite from disk (resolves `documentPart type="file"`).
    pub resource_context: super::resources::TdmlResourceContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TdmlSchema {
    pub name: String,
    pub xsd: String,
    /// Directory for resolving relative `xs:include` when schema was loaded from a file.
    pub compile_base_dir: Option<String>,
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
    /// When set, parse succeeds but XSD facet validation must fail with these messages.
    pub expected_validation_errors: Option<Vec<String>>,
    /// When set, parse must succeed and warning text must contain each message.
    pub expected_warnings: Option<Vec<String>>,
    pub config: Option<String>,
    pub round_trip: RoundTrip,
    pub validation: Option<TdmlValidationMode>,
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
    /// When set, decode loads document bytes from this TDML resource path at run time.
    pub file_resource: Option<String>,
    /// Significant bits in the last byte when the document ends mid-byte.
    pub last_byte_bit_count: Option<u8>,
    /// How bits are packed into `data` for `type="bits"` documents.
    pub transmission_bit_order: crate::schema::BitOrder,
    /// When set, overrides schema format `bitOrder` for stream bit extraction (TDML `@bitOrder`).
    pub document_transmission_bit_order: Option<crate::schema::BitOrder>,
    /// Bits document with a following encoded `type="text"` part (MIL / 7-bit packed continuation).
    pub mixed_bits_text_document: bool,
    /// Per-part `(bitOrder, lengthInBits)` for TDML decode when `@bitOrder` or part orders apply.
    pub part_bit_order_regions: Vec<(crate::schema::BitOrder, usize)>,
    /// TDML document assembly error (e.g. illegal bitOrder transition between parts).
    pub load_error: Option<String>,
}

impl TdmlDocument {
    /// Total significant bits for `type="bits"` documents (None for byte-aligned docs).
    pub fn significant_bit_length(&self) -> Option<usize> {
        if self.kind != DocumentKind::Bits {
            return None;
        }
        if self.data.is_empty() {
            return Some(0);
        }
        let trailing = self.last_byte_bit_count.unwrap_or(0) as usize;
        if trailing == 0 {
            return Some(self.data.len() * 8);
        }
        Some((self.data.len() - 1) * 8 + trailing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Text,
    Hex,
    Bits,
}

/// Parse a TDML test suite document.
pub fn parse_tdml(input: &str) -> Result<TdmlSuite> {
    let normalized = normalize_tdml_xml_for_parse(input);
    let mut reader = XmlReader::new(&normalized);
    let attrs = reader.expect_start("testSuite")?;
    let name = attrs
        .get("suiteName")
        .cloned()
        .unwrap_or_else(|| "unnamed".into());
    let default_round_trip = parse_round_trip(attrs.get("defaultRoundTrip").map(String::as_str));
    let default_validation =
        parse_validation_mode(attrs.get("defaultValidation").map(String::as_str))
            .unwrap_or(TdmlValidationMode::Off);

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
        default_validation,
        resource_context: super::resources::TdmlResourceContext::default(),
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

fn parse_define_schema(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<TdmlSchema> {
    let name = attrs
        .get("name")
        .cloned()
        .ok_or_else(|| ParseError::MissingAttribute {
            element: "defineSchema".into(),
            attribute: "name".into(),
        })?;
    let inner = reader.read_inner_xml()?;
    let element_form_default = attrs.get("elementFormDefault").map(String::as_str);
    Ok(TdmlSchema {
        name,
        xsd: wrap_schema(&inner, element_form_default),
        compile_base_dir: None,
    })
}

fn parse_config_children(
    reader: &mut XmlReader<'_>,
    parent_tag: &str,
    tunables: &mut DaffodilTunables,
    external_variables: &mut BTreeMap<String, String>,
) -> Result<()> {
    reader.for_each_child(parent_tag, |local, _, r| match local {
        "externalVariableBindings" => {
            r.for_each_child("externalVariableBindings", |local, attrs, r| {
                if local == "bind" {
                    let var_name = attrs.get("name").cloned().ok_or_else(|| {
                        ParseError::MissingAttribute {
                            element: "bind".into(),
                            attribute: "name".into(),
                        }
                    })?;
                    let value = r.read_text_until_end("bind")?;
                    external_variables.insert(var_name, value.trim().to_string());
                } else {
                    r.skip_current_subtree()?;
                }
                Ok(())
            })?;
            Ok(())
        }
        "tunables" => {
            r.for_each_child("tunables", |local, _, r| {
                match local {
                    "allowSignedIntegerLength1Bit" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.allow_signed_integer_length1_bit = text.trim() != "false";
                    }
                    "unqualifiedPathStepPolicy" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.unqualified_path_step_policy = match text.trim() {
                            "noNamespace" => {
                                crate::length_validate::UnqualifiedPathStepPolicy::NoNamespace
                            }
                            "preferDefaultNamespace" => {
                                crate::length_validate::UnqualifiedPathStepPolicy::PreferDefaultNamespace
                            }
                            _ => crate::length_validate::UnqualifiedPathStepPolicy::DefaultNamespace,
                        };
                    }
                    "maxOccursBounds" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.max_occurs_bounds = text.trim().parse().ok();
                    }
                    "requireTextBidiProperty" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.require_text_bidi_property = Some(text.trim() == "true");
                    }
                    "requireFloatingProperty" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.require_floating_property = Some(text.trim() == "true");
                    }
                    "requireEncodingErrorPolicy" | "requireEncodingErrorPolicyProperty" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.require_encoding_error_policy = Some(text.trim() == "true");
                    }
                    "maxHexBinaryLengthInBytes" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.max_hex_binary_length_in_bytes = text.trim().parse().ok();
                    }
                    "invalidRestrictionPolicy" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.invalid_restriction_policy = match text.trim() {
                            "ignore" => crate::length_validate::InvalidRestrictionPolicy::Ignore,
                            "validate" => {
                                crate::length_validate::InvalidRestrictionPolicy::Validate
                            }
                            _ => crate::length_validate::InvalidRestrictionPolicy::Error,
                        };
                    }
                    "escalateWarningsToErrors" => {
                        let text = r.read_text_until_end(local)?;
                        tunables.escalate_warnings_to_errors = text.trim() == "true";
                    }
                    _ => {
                        r.skip_current_subtree()?;
                    }
                }
                Ok(())
            })?;
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })
}

fn parse_define_config(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<(String, TdmlConfig)> {
    let name = attrs
        .get("name")
        .cloned()
        .ok_or_else(|| ParseError::MissingAttribute {
            element: "defineConfig".into(),
            attribute: "name".into(),
        })?;
    let mut tunables = DaffodilTunables::default();
    let mut external_variables = BTreeMap::new();
    parse_config_children(
        reader,
        "defineConfig",
        &mut tunables,
        &mut external_variables,
    )?;
    Ok((
        name,
        TdmlConfig {
            tunables,
            external_variables,
        },
    ))
}

pub fn parse_external_config_xml(input: &str) -> Result<TdmlConfig> {
    let normalized = normalize_tdml_xml_for_parse(input);
    let mut reader = XmlReader::new(&normalized);
    let _attrs = reader.expect_start("dfdlConfig")?;
    let mut tunables = DaffodilTunables::default();
    let mut external_variables = BTreeMap::new();
    parse_config_children(
        &mut reader,
        "dfdlConfig",
        &mut tunables,
        &mut external_variables,
    )?;
    Ok(TdmlConfig {
        tunables,
        external_variables,
    })
}

fn parse_parser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<ParserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root_from_attr = attrs.get("root").cloned();
    let model = attrs
        .get("model")
        .cloned()
        .ok_or_else(|| ParseError::MissingAttribute {
            element: "parserTestCase".into(),
            attribute: "model".into(),
        })?;
    let round_trip = parse_round_trip(attrs.get("roundTrip").map(String::as_str));
    let validation = attrs
        .get("validation")
        .map(|s| parse_validation_mode(Some(s.as_str())))
        .transpose()?;
    let config = attrs.get("config").cloned();

    let mut documents = Vec::new();
    let mut expected_infoset = String::new();
    let mut expected_errors = None;
    let mut expected_validation_errors = None;
    let mut expected_warnings = None;

    reader.for_each_child("parserTestCase", |local, doc_attrs, r| match local {
        "document" => {
            documents.push(parse_document(r, &doc_attrs)?);
            Ok(())
        }
        "infoset" => {
            expected_infoset = r.read_inner_xml()?;
            Ok(())
        }
        "errors" => {
            expected_errors = Some(parse_error_messages(r, "errors")?);
            Ok(())
        }
        "validationErrors" => {
            expected_validation_errors = Some(parse_error_messages(r, "validationErrors")?);
            Ok(())
        }
        "warnings" => {
            expected_warnings = Some(parse_error_messages(r, "warnings")?);
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
        expected_validation_errors,
        expected_warnings,
        config,
        round_trip,
        validation,
    })
}

pub fn effective_validation(test: &ParserTestCase, suite: &TdmlSuite) -> TdmlValidationMode {
    test.validation.unwrap_or(suite.default_validation)
}

fn parse_validation_mode(raw: Option<&str>) -> Result<TdmlValidationMode> {
    match raw.unwrap_or("off").trim() {
        "" | "off" => Ok(TdmlValidationMode::Off),
        "limited" => Ok(TdmlValidationMode::Limited),
        "on" => Ok(TdmlValidationMode::On),
        other => Err(ParseError::InvalidXml {
            message: alloc::format!("unknown validation mode `{other}`"),
        }
        .into()),
    }
}

fn parse_unparser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<UnparserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root_from_attr = attrs.get("root").cloned();
    let model = attrs
        .get("model")
        .cloned()
        .ok_or_else(|| ParseError::MissingAttribute {
            element: "unparserTestCase".into(),
            attribute: "model".into(),
        })?;
    let config = attrs.get("config").cloned();

    let mut infoset = String::new();
    let mut expected_errors = None;
    let mut documents = Vec::new();

    reader.for_each_child("unparserTestCase", |local, doc_attrs, r| match local {
        "infoset" => {
            infoset = r.read_inner_xml()?;
            Ok(())
        }
        "document" => {
            documents.push(parse_document(r, &doc_attrs)?);
            Ok(())
        }
        "errors" => {
            expected_errors = Some(parse_error_messages(r, "errors")?);
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;

    let root = root_from_attr.unwrap_or_else(|| {
        super::infoset::infer_root_element_name(&infoset).unwrap_or_else(|| name.clone())
    });

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

pub(crate) mod document;
pub(crate) use document::*;

fn parse_error_messages(reader: &mut XmlReader<'_>, container: &str) -> Result<Vec<String>> {
    let mut messages = Vec::new();
    reader.for_each_child(container, |local, _, r| {
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

fn wrap_schema(inner: &str, element_form_default: Option<&str>) -> String {
    let efd = element_form_default
        .map(|v| alloc::format!(r#" elementFormDefault="{v}""#))
        .unwrap_or_default();
    alloc::format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:dfdlx="http://www.ogf.org/dfdl/dfdl-1.0/extensions"
           xmlns:ex="http://example.com"
           targetNamespace="http://example.com"{efd}>
{inner}
</xs:schema>"#
    )
}

fn parse_hex_document(text: &str) -> Result<Vec<u8>> {
    // TDML: freeform whitespace allowed; any non-hex character is ignored.
    let mut hex: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.is_empty() {
        return Ok(Vec::new());
    }
    if !hex.len().is_multiple_of(2) {
        hex.insert(0, '0');
    }
    let mut out = Vec::new();
    for chunk in hex.as_bytes().chunks(2) {
        let hi = (chunk[0] as char)
            .to_digit(16)
            .ok_or_else(|| ParseError::InvalidXml {
                message: "invalid hex document".into(),
            })?;
        let lo = (chunk[1] as char)
            .to_digit(16)
            .ok_or_else(|| ParseError::InvalidXml {
                message: "invalid hex document".into(),
            })?;
        out.push((hi << 4 | lo) as u8);
    }
    Ok(out)
}

/// Append raw document bytes to a TDML bit stream (MSB-first within each byte).
fn append_bytes_as_msb_bits(bits: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        for i in 0..8 {
            bits.push((byte >> (7 - i)) & 1);
        }
    }
}

fn collect_bit_digits_from_text(text: &str) -> String {
    text.chars().filter(|c| *c == '0' || *c == '1').collect()
}

fn collect_bits_from_text(text: &str) -> Vec<u8> {
    collect_bit_digits_from_text(text)
        .chars()
        .map(|c| if c == '1' { 1 } else { 0 })
        .collect()
}

fn reverse_bit_string(s: &str) -> String {
    s.chars().rev().collect()
}

fn byte_to_msb_bit_string(b: u8) -> String {
    alloc::format!("{:08b}", b)
}

/// Match Daffodil `TextDocumentPart.dataBits`.
fn text_document_data_bits(text: &str, encoding: Option<&str>) -> Result<Vec<String>> {
    let encoded = if let Some(enc) = encoding {
        encode_document_text(text, enc).map_err(|e| ParseError::InvalidXml {
            message: alloc::format!("documentPart encoding: {e}"),
        })?
    } else {
        text.as_bytes().to_vec()
    };
    let bit_strings: Vec<String> = encoded.iter().map(|b| byte_to_msb_bit_string(*b)).collect();
    if let Some(enc) = encoding {
        if let Some(spec) = bits_charset_spec(enc) {
            let chunks: Vec<String> = text
                .chars()
                .map(|ch| {
                    let u = ch as u32;
                    let mut s = String::new();
                    for i in (0..spec.width).rev() {
                        s.push(if ((u >> i) & 1) == 1 { '1' } else { '0' });
                    }
                    s
                })
                .collect();
            return Ok(chunks);
        }
    }
    Ok(bit_strings)
}

fn chunk_bit_string_ltr(bits: &str) -> Vec<String> {
    bits.as_bytes()
        .chunks(8)
        .map(|chunk| core::str::from_utf8(chunk).unwrap_or("").to_string())
        .collect()
}

fn bit_chunks_for_part(bits: &str, byte_order: DocumentByteOrder) -> Vec<String> {
    match byte_order {
        DocumentByteOrder::Ltr => chunk_bit_string_ltr(bits),
        DocumentByteOrder::Rtl => {
            let rev = reverse_bit_string(bits);
            chunk_bit_string_ltr(&rev)
                .into_iter()
                .map(|chunk| reverse_bit_string(&chunk))
                .collect()
        }
    }
}

/// Regroup bit digits into 8-bit strings (`Seq.mkString` then `sliding(8, 8)`).
fn regroup_bit_digit_strings(chunks: &[String]) -> Vec<String> {
    chunks
        .join("")
        .as_bytes()
        .chunks(8)
        .map(|chunk| core::str::from_utf8(chunk).unwrap_or("").to_string())
        .collect()
}

/// Match Daffodil `Document.documentBits` before last-byte padding.
fn document_bits_byte_strings(
    part_chunks: &[Vec<String>],
    document_bit_order: DocumentBitOrder,
) -> Vec<String> {
    let all_parts_bits: Vec<String> = match document_bit_order {
        DocumentBitOrder::MsbFirst => part_chunks
            .iter()
            .flat_map(|part| part.iter().cloned())
            .collect(),
        DocumentBitOrder::LsbFirst => {
            let reversed_parts: Vec<Vec<String>> = part_chunks
                .iter()
                .map(|part| part.iter().map(|chunk| reverse_bit_string(chunk)).collect())
                .collect();
            let flat: String = reversed_parts
                .iter()
                .flat_map(|part| part.iter())
                .cloned()
                .collect();
            let rtl_bits = reverse_bit_string(&flat);
            rtl_bits
                .chars()
                .rev()
                .collect::<String>()
                .as_bytes()
                .chunks(8)
                .map(|chunk| reverse_bit_string(core::str::from_utf8(chunk).unwrap_or("")))
                .collect()
        }
    };
    regroup_bit_digit_strings(&all_parts_bits)
}

/// Match Daffodil `Document.documentBits` / `bits2Bytes` for TDML test data.
fn assemble_tdml_document_bytes(
    part_chunks: &[Vec<String>],
    document_bit_order: DocumentBitOrder,
) -> (Vec<u8>, u8) {
    let byte_strings = document_bits_byte_strings(part_chunks, document_bit_order);
    if byte_strings.is_empty() {
        return (Vec::new(), 0);
    }
    let total_bits: usize = part_chunks
        .iter()
        .flat_map(|p| p.iter())
        .map(String::len)
        .sum();
    let n_frag = total_bits % 8;
    let n_add = if n_frag == 0 { 0 } else { 8 - n_frag };
    let mut all_bits = byte_strings;
    let last = all_bits.pop().unwrap_or_default();
    let padded_last = match document_bit_order {
        DocumentBitOrder::MsbFirst => {
            let mut s = last;
            s.push_str(&"0".repeat(n_add));
            s
        }
        DocumentBitOrder::LsbFirst => {
            alloc::format!("{}{}", "0".repeat(n_add), last)
        }
    };
    all_bits.push(padded_last);
    let mut out = Vec::with_capacity(all_bits.len());
    for s in all_bits {
        if s.is_empty() {
            continue;
        }
        let byte = u8::from_str_radix(&s, 2).unwrap_or(0);
        out.push(byte);
    }
    let trailing = n_frag as u8;
    (out, trailing)
}

fn pack_bits_msb_first(bits: &[u8]) -> (Vec<u8>, u8) {
    let trailing = (bits.len() % 8) as u8;
    let mut out = Vec::new();
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (i, bit) in chunk.iter().enumerate() {
            byte |= bit << (7 - i);
        }
        out.push(byte);
    }
    (out, trailing)
}

#[allow(dead_code)]
fn local_tag(name: &str) -> &str {
    local_name_str(name)
}

#[cfg(test)]
mod tests;
