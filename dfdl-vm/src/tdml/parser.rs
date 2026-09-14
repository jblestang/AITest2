use crate::error::{ParseError, Result};
use crate::schema::BitOrder;
use crate::length_validate::DaffodilTunables;
use crate::schema::{expand_entities, expand_entities_str};
use crate::vm::encoding::{bits_charset_spec, encode_document_text};
use crate::xml_util::{attrs_to_map, local_name_str, XmlReader};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xml_no_std::reader::XmlEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundTrip {
    /// Use the suite-level `defaultRoundTrip` attribute.
    Inherit,
    Disabled,
    OnePass,
    TwoPass,
}

/// Parsed TDML test suite.
#[derive(Debug, Clone, PartialEq)]
pub struct TdmlSuite {
    pub name: String,
    pub schemas: BTreeMap<String, TdmlSchema>,
    pub configs: BTreeMap<String, DaffodilTunables>,
    pub tests: Vec<ParserTestCase>,
    pub unparser_tests: Vec<UnparserTestCase>,
    pub default_round_trip: RoundTrip,
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
    pub config: Option<String>,
    pub round_trip: RoundTrip,
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
    /// Significant bits in the last byte when the document ends mid-byte.
    pub last_byte_bit_count: Option<u8>,
    /// How bits are packed into `data` for `type="bits"` documents.
    pub transmission_bit_order: crate::schema::BitOrder,
    /// When set, overrides schema format `bitOrder` for stream bit extraction (TDML `@bitOrder`).
    pub document_transmission_bit_order: Option<crate::schema::BitOrder>,
    /// Bits document with a following encoded `type="text"` part (MIL / 7-bit packed continuation).
    pub mixed_bits_text_document: bool,
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
    let mut reader = XmlReader::new(input);
    let attrs = reader.expect_start("testSuite")?;
    let name = attrs
        .get("suiteName")
        .cloned()
        .unwrap_or_else(|| "unnamed".into());
    let default_round_trip = parse_round_trip(attrs.get("defaultRoundTrip").map(String::as_str));

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

fn parse_define_schema(attrs: BTreeMap<String, String>, reader: &mut XmlReader<'_>) -> Result<TdmlSchema> {
    let name = attrs.get("name").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "defineSchema".into(),
        attribute: "name".into(),
    })?;
    let inner = reader.read_inner_xml()?;
    Ok(TdmlSchema {
        name,
        xsd: wrap_schema(&inner),
        compile_base_dir: None,
    })
}

fn parse_define_config(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<(String, DaffodilTunables)> {
    let name = attrs.get("name").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "defineConfig".into(),
        attribute: "name".into(),
    })?;
    let mut tunables = DaffodilTunables::default();
    reader.for_each_child("defineConfig", |local, _, r| match local {
        "tunables" => {
            r.for_each_child("tunables", |local, _, r| {
                if local == "allowSignedIntegerLength1Bit" {
                    let text = r.read_text_until_end("allowSignedIntegerLength1Bit")?;
                    tunables.allow_signed_integer_length1_bit = text.trim() != "false";
                } else {
                    r.skip_current_subtree()?;
                }
                Ok(())
            })?;
            Ok(())
        }
        _ => r.skip_current_subtree(),
    })?;
    Ok((name, tunables))
}

fn parse_parser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<ParserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root_from_attr = attrs.get("root").cloned();
    let model = attrs.get("model").cloned().ok_or_else(|| ParseError::MissingAttribute {
        element: "parserTestCase".into(),
        attribute: "model".into(),
    })?;
    let round_trip = parse_round_trip(attrs.get("roundTrip").map(String::as_str));
    let config = attrs.get("config").cloned();

    let mut documents = Vec::new();
    let mut expected_infoset = String::new();
    let mut expected_errors = None;

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
            expected_errors = Some(parse_errors(r)?);
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
        config,
        round_trip,
    })
}

fn parse_unparser_test_case(
    attrs: BTreeMap<String, String>,
    reader: &mut XmlReader<'_>,
) -> Result<UnparserTestCase> {
    let name = attrs.get("name").cloned().unwrap_or_default();
    let root_from_attr = attrs.get("root").cloned();
    let model = attrs.get("model").cloned().ok_or_else(|| ParseError::MissingAttribute {
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
            expected_errors = Some(parse_errors(r)?);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentBitOrder {
    MsbFirst,
    LsbFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentByteOrder {
    Ltr,
    Rtl,
}

fn parse_document_bit_order(value: &str) -> Result<DocumentBitOrder> {
    match value {
        "MSBFirst" => Ok(DocumentBitOrder::MsbFirst),
        "LSBFirst" => Ok(DocumentBitOrder::LsbFirst),
        other => Err(ParseError::InvalidXml {
            message: alloc::format!("unknown document bitOrder `{other}`"),
        }
        .into()),
    }
}

fn effective_document_bit_order(
    document_default: DocumentBitOrder,
    from_document_attr: bool,
    parts: &[(DocumentBitOrder, usize, bool)],
) -> DocumentBitOrder {
    if from_document_attr || parts.is_empty() {
        return document_default;
    }
    let first = parts[0].0;
    if parts.iter().all(|(order, _, _)| *order == first) {
        first
    } else {
        document_default
    }
}

fn document_bit_order_to_transmission(order: DocumentBitOrder) -> BitOrder {
    match order {
        DocumentBitOrder::LsbFirst => BitOrder::LeastSignificantBitFirst,
        DocumentBitOrder::MsbFirst => BitOrder::MostSignificantBitFirst,
    }
}

fn packed_document_transmission_bit_order(use_lsb_assembly: bool) -> BitOrder {
    if use_lsb_assembly {
        BitOrder::LeastSignificantBitFirst
    } else {
        BitOrder::MostSignificantBitFirst
    }
}

fn check_explicit_part_bit_order_mixture(
    from_document_attr: bool,
    parts: &[(DocumentBitOrder, usize, bool)],
) -> Result<()> {
    if from_document_attr {
        return Ok(());
    }
    let mut saw_lsb = false;
    let mut saw_msb = false;
    for (order, _, explicit) in parts {
        if !explicit {
            continue;
        }
        match order {
            DocumentBitOrder::LsbFirst => saw_lsb = true,
            DocumentBitOrder::MsbFirst => saw_msb = true,
        }
    }
    if saw_lsb && saw_msb {
        return Err(ParseError::InvalidXml {
            message: "Must specify bitOrder on document element when parts have a mixture of bit orders.".into(),
        }
        .into());
    }
    Ok(())
}

fn check_document_part_bit_order_transitions(
    parts: &[(DocumentBitOrder, usize, bool)],
) -> Result<()> {
    if parts.len() <= 1 {
        return Ok(());
    }
    let mut cumulative = 0usize;
    for (i, (order, len, _)) in parts.iter().enumerate() {
        if i > 0 {
            let prior = parts[i - 1].0;
            if prior != *order && cumulative % 8 != 0 {
                return Err(ParseError::InvalidXml {
                    message: "bitOrder can only change on a byte boundary.".into(),
                }
                .into());
            }
        }
        cumulative += len;
    }
    Ok(())
}

fn parse_document(
    reader: &mut XmlReader<'_>,
    doc_attrs: &BTreeMap<String, String>,
) -> Result<TdmlDocument> {
    reader.skip_insignificant_ws()?;
    let document_bit_order_from_attr = doc_attrs.get("bitOrder").is_some();
    let default_bit_order = doc_attrs
        .get("bitOrder")
        .map(|s| parse_document_bit_order(s))
        .transpose()?
        .unwrap_or(DocumentBitOrder::MsbFirst);
    let document_transmission_bit_order = doc_attrs
        .get("bitOrder")
        .map(|s| parse_document_bit_order(s))
        .transpose()?
        .map(document_bit_order_to_transmission);

    if reader.peek_is_end("document")? {
        reader.expect_end("document")?;
        return Ok(TdmlDocument {
            kind: DocumentKind::Text,
            data: Vec::new(),
            last_byte_bit_count: None,
            transmission_bit_order: BitOrder::MostSignificantBitFirst,
            document_transmission_bit_order,
            mixed_bits_text_document: false,
            load_error: None,
        });
    }

    if reader.peek_start_local()? == Some("documentPart".to_string()) {
        let mut kind = DocumentKind::Text;
        let mut data = Vec::new();
        let mut last_byte_bit_count = None;
        let mut pending_bits: Vec<u8> = Vec::new();
        let mut bit_part_chunks: Vec<Vec<String>> = Vec::new();
        let mut saw_bits_part = false;
        let mut part_transitions: Vec<(DocumentBitOrder, usize, bool)> = Vec::new();
        let mut saw_rtl_byte_order = false;
        let mut mixed_bits_text_document = false;

        let flush_pending_bits =
            |pending: &mut Vec<u8>, data: &mut Vec<u8>, last: &mut Option<u8>| {
                if pending.is_empty() {
                    return;
                }
                let (packed, trailing) = pack_bits_msb_first(pending);
                data.extend(packed);
                *last = Some(trailing);
                pending.clear();
            };

        while reader.peek_start_local()? == Some("documentPart".to_string()) {
            let part = parse_document_part(reader, default_bit_order, document_bit_order_from_attr)?;
            part_transitions.push((part.bit_order, part.length_in_bits, part.explicit_bit_order));
            if let Some(chunks) = part.bit_chunks {
                saw_bits_part = true;
                kind = DocumentKind::Bits;
                if part.byte_order == DocumentByteOrder::Rtl {
                    saw_rtl_byte_order = true;
                }
                for chunk in &chunks {
                    for c in chunk.chars() {
                        pending_bits.push(if c == '1' { 1 } else { 0 });
                    }
                }
                bit_part_chunks.push(chunks);
            } else if part.kind == DocumentKind::Hex && part.bit_chunks.is_some() {
                let chunks = part.bit_chunks.unwrap();
                saw_bits_part = true;
                kind = DocumentKind::Bits;
                bit_part_chunks.push(chunks);
            } else if saw_bits_part && part.kind == DocumentKind::Text {
                mixed_bits_text_document = true;
                if saw_rtl_byte_order {
                    // Daffodil `Document.documentBits`: encoded text parts join the bit stream (MIL / 7-bit packed).
                    let chunks = part
                        .data_bit_chunks
                        .clone()
                        .expect("text data_bit_chunks in RTL bits document");
                    bit_part_chunks.push(chunks);
                    kind = DocumentKind::Bits;
                } else if pending_bits.len() % 8 != 0 {
                    append_bytes_as_msb_bits(&mut pending_bits, &part.data);
                } else {
                    flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
                    data.extend(part.data);
                    last_byte_bit_count = part.last_byte_bit_count;
                }
            } else {
                flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
                if data.is_empty() && !saw_bits_part {
                    kind = part.kind;
                }
                data.extend(part.data);
                last_byte_bit_count = part.last_byte_bit_count;
            }
            reader.skip_insignificant_ws()?;
        }
        reader.expect_end("document")?;
        let assembly_order = effective_document_bit_order(
            default_bit_order,
            document_bit_order_from_attr,
            &part_transitions,
        );
        let use_lsb_assembly = !bit_part_chunks.is_empty()
            && (assembly_order == DocumentBitOrder::LsbFirst || saw_rtl_byte_order);
        if use_lsb_assembly {
            let (packed, trailing) =
                assemble_tdml_document_bytes(&bit_part_chunks, DocumentBitOrder::LsbFirst);
            pending_bits.clear();
            if data.is_empty() {
                data = packed;
                last_byte_bit_count = Some(trailing);
            } else {
                data.extend(packed);
                last_byte_bit_count = Some(trailing);
            }
            kind = DocumentKind::Bits;
        } else {
            flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
            if saw_bits_part {
                kind = DocumentKind::Bits;
            }
        }
        if let Err(e) = check_explicit_part_bit_order_mixture(document_bit_order_from_attr, &part_transitions) {
            return Ok(TdmlDocument {
                kind: DocumentKind::Text,
                data: Vec::new(),
                last_byte_bit_count: None,
                transmission_bit_order: BitOrder::MostSignificantBitFirst,
                document_transmission_bit_order,
                mixed_bits_text_document,
                load_error: Some(e.to_string()),
            });
        }
        if let Err(e) = check_document_part_bit_order_transitions(&part_transitions) {
            return Ok(TdmlDocument {
                kind: DocumentKind::Text,
                data: Vec::new(),
                last_byte_bit_count: None,
                transmission_bit_order: BitOrder::MostSignificantBitFirst,
                document_transmission_bit_order,
                mixed_bits_text_document,
                load_error: Some(e.to_string()),
            });
        }
        return Ok(TdmlDocument {
            kind,
            data,
            last_byte_bit_count,
            transmission_bit_order: packed_document_transmission_bit_order(use_lsb_assembly),
            document_transmission_bit_order,
            mixed_bits_text_document,
            load_error: None,
        });
    }

    let text = reader.read_text_until_end("document")?;
    Ok(TdmlDocument {
        kind: DocumentKind::Text,
        data: text.into_bytes(),
        last_byte_bit_count: None,
        transmission_bit_order: BitOrder::MostSignificantBitFirst,
        document_transmission_bit_order,
        mixed_bits_text_document: false,
        load_error: None,
    })
}

struct ParsedDocumentPart {
    kind: DocumentKind,
    data: Vec<u8>,
    last_byte_bit_count: Option<u8>,
    /// Bit chunks (up to 8 digits each) when `kind == Bits`.
    bit_chunks: Option<Vec<String>>,
    /// Daffodil `TextDocumentPart.dataBits` when `kind == Text`.
    data_bit_chunks: Option<Vec<String>>,
    bit_order: DocumentBitOrder,
    explicit_bit_order: bool,
    byte_order: DocumentByteOrder,
    length_in_bits: usize,
}

fn parse_document_part(
    reader: &mut XmlReader<'_>,
    default_bit_order: DocumentBitOrder,
    document_bit_order_from_attr: bool,
) -> Result<ParsedDocumentPart> {
    let XmlEvent::StartElement { attributes, .. } = reader.next_event()? else {
        return Err(ParseError::InvalidXml {
            message: "expected documentPart".into(),
        }
        .into());
    };
    let attrs = attrs_to_map(&attributes);
    let kind = match attrs.get("type").map(String::as_str) {
        Some("hex") | Some("byte") => DocumentKind::Hex,
        Some("bits") => DocumentKind::Bits,
        _ => DocumentKind::Text,
    };
    let replace_entities = attrs
        .get("replaceDFDLEntities")
        .map(|v| v == "true")
        .unwrap_or(false);
    let encoding = attrs.get("encoding").map(String::as_str);
    let explicit_bit_order = attrs.contains_key("bitOrder");
    let bit_order = attrs
        .get("bitOrder")
        .map(|s| parse_document_bit_order(s))
        .transpose()?
        .unwrap_or(default_bit_order);
    let byte_order = match attrs.get("byteOrder").map(String::as_str) {
        Some("RTL") => {
            if bit_order != DocumentBitOrder::LsbFirst {
                return Err(ParseError::InvalidXml {
                    message: "byteOrder RTL requires bitOrder LSBFirst".into(),
                }
                .into());
            }
            DocumentByteOrder::Rtl
        }
        Some("LTR") | None => DocumentByteOrder::Ltr,
        Some(other) => {
            return Err(ParseError::InvalidXml {
                message: alloc::format!("unknown document byteOrder `{other}`"),
            }
            .into());
        }
    };
    let text = reader.read_text_until_end("documentPart")?;
    let (data, last_byte_bit_count, bit_chunks, data_bit_chunks) = match kind {
        DocumentKind::Text => {
            let bits_text = if replace_entities {
                expand_entities_str(&text)
            } else {
                text.clone()
            };
            let data = if replace_entities {
                expand_entities(&text)
            } else if let Some(enc) = encoding {
                encode_document_text(&text, enc).map_err(|e| ParseError::InvalidXml {
                    message: alloc::format!("documentPart encoding: {e}"),
                })?
            } else {
                text.into_bytes()
            };
            let data_bit_chunks = text_document_data_bits(&bits_text, encoding).ok();
            (data, None, None, data_bit_chunks)
        }
        DocumentKind::Hex => {
            let data_bytes = parse_hex_document(&text)?;
            if document_bit_order_from_attr && default_bit_order == DocumentBitOrder::LsbFirst {
                let chunks = data_bytes
                    .iter()
                    .map(|b| alloc::format!("{:08b}", b))
                    .collect();
                (Vec::new(), None, Some(chunks), None)
            } else {
                (data_bytes, None, None, None)
            }
        }
        DocumentKind::Bits => {
            let digits = collect_bit_digits_from_text(&text);
            let chunks = bit_chunks_for_part(&digits, byte_order);
            (Vec::new(), None, Some(chunks), None)
        }
    };
    let length_in_bits = if let Some(chunks) = &bit_chunks {
        chunks.iter().map(String::len).sum()
    } else {
        data.len() * 8
    };
    Ok(ParsedDocumentPart {
        kind,
        data,
        last_byte_bit_count,
        bit_chunks,
        data_bit_chunks,
        bit_order,
        explicit_bit_order,
        byte_order,
        length_in_bits,
    })
}

fn parse_errors(reader: &mut XmlReader<'_>) -> Result<Vec<String>> {
    let mut messages = Vec::new();
    reader.for_each_child("errors", |local, _, r| {
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
    // TDML: freeform whitespace allowed; any non-hex character is ignored.
    let mut hex: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.is_empty() {
        return Ok(Vec::new());
    }
    if hex.len() % 2 != 0 {
        hex.insert(0, '0');
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

/// Append raw document bytes to a TDML bit stream (MSB-first within each byte).
fn append_bytes_as_msb_bits(bits: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        for i in 0..8 {
            bits.push((byte >> (7 - i)) & 1);
        }
    }
}

fn collect_bit_digits_from_text(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '0' || *c == '1')
        .collect()
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
            let code_units = text.chars().count();
            let n_bits = code_units * spec.width as usize;
            let concatenated: String = bit_strings.iter().rev().map(String::as_str).collect();
            let all_bits = if concatenated.len() > n_bits {
                concatenated[concatenated.len() - n_bits..].to_string()
            } else {
                concatenated
            };
            let width = spec.width as usize;
            let rev: String = all_bits.chars().rev().collect();
            let chunks = rev
                .as_bytes()
                .chunks(width)
                .map(|chunk| {
                    reverse_bit_string(core::str::from_utf8(chunk).unwrap_or(""))
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
                .map(|part| {
                    part.iter()
                        .map(|chunk| reverse_bit_string(chunk))
                        .collect()
                })
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
                .map(|chunk| {
                    reverse_bit_string(core::str::from_utf8(chunk).unwrap_or(""))
                })
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
mod tests {
    use super::*;
    use alloc::vec;

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
            assert!(false, "schema compile failed: {e}\n---\n{}", schema.xsd);
        }
    }

    #[test]
    fn parse_byte_document_part_as_hex() {
        let tdml = r##"<tdml:testSuite suiteName="t" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:int"/></xs:schema></tdml:defineSchema>
  <tdml:parserTestCase name="bin" root="A" model="s">
    <tdml:document><tdml:documentPart type="byte">00 00 00 05</tdml:documentPart></tdml:document>
    <tdml:infoset><tdml:dfdlInfoset><A>5</A></tdml:dfdlInfoset></tdml:infoset>
  </tdml:parserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        let test = suite.tests.iter().find(|t| t.name == "bin").unwrap();
        assert_eq!(test.documents[0].data, alloc::vec![0, 0, 0, 5]);
    }

    #[test]
    fn parse_utf16be_document_part() {
        let tdml = r##"<tdml:testSuite suiteName="t" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:string"/></xs:schema></tdml:defineSchema>
  <tdml:parserTestCase name="utf16" root="A" model="s">
    <tdml:document><tdml:documentPart type="text" encoding="utf-16be">AB</tdml:documentPart></tdml:document>
    <tdml:infoset><tdml:dfdlInfoset><A>AB</A></tdml:dfdlInfoset></tdml:infoset>
  </tdml:parserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        let test = suite.tests.iter().find(|t| t.name == "utf16").unwrap();
        assert_eq!(test.documents[0].data, alloc::vec![0x00, b'A', 0x00, b'B']);
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

    #[test]
    fn parse_round_trip_attributes() {
        let tdml = r##"<tdml:testSuite suiteName="t" defaultRoundTrip="onePass" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:string"/></xs:schema></tdml:defineSchema>
  <tdml:parserTestCase name="two" root="A" model="s" roundTrip="twoPass">
    <tdml:document><tdml:documentPart type="text">x</tdml:documentPart></tdml:document>
    <tdml:infoset><tdml:dfdlInfoset><A>x</A></tdml:dfdlInfoset></tdml:infoset>
  </tdml:parserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        assert_eq!(suite.default_round_trip, RoundTrip::OnePass);
        assert_eq!(suite.tests[0].round_trip, RoundTrip::TwoPass);
    }

    #[test]
    fn parse_unparser_test_case() {
        let tdml = r##"<tdml:testSuite suiteName="t" xmlns:tdml="http://www.ibm.com/xmlns/dfdl/testData" xmlns:ex="http://example.com">
  <tdml:defineSchema name="s"><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="A" type="xs:string"/></xs:schema></tdml:defineSchema>
  <tdml:unparserTestCase name="enc" root="A" model="s">
    <tdml:infoset><tdml:dfdlInfoset><A>hi</A></tdml:dfdlInfoset></tdml:infoset>
    <tdml:errors><tdml:error>bad</tdml:error></tdml:errors>
  </tdml:unparserTestCase>
</tdml:testSuite>"##;
        let suite = parse_tdml(tdml).expect("parse");
        assert_eq!(suite.unparser_tests.len(), 1);
        assert_eq!(
            suite.unparser_tests[0].expected_errors,
            Some(alloc::vec![alloc::string::String::from("bad")])
        );
    }

    #[test]
    fn parse_negative_test_errors() {
        let tdml = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/lengthKind/PatternTests.tdml"
        );
        let suite = parse_tdml(tdml).expect("parse pattern tdml");
        let fail = suite
            .tests
            .iter()
            .find(|t| t.name == "lengthKindPatternFail")
            .expect("negative test");
        assert_eq!(
            fail.expected_errors,
            Some(alloc::vec![String::new(), String::new()])
        );
        let no_match = suite
            .tests
            .iter()
            .find(|t| t.name == "lengthKindPattern_02")
            .expect("no-match test");
        assert_eq!(no_match.expected_errors, Some(alloc::vec![String::new()]));
    }

    #[test]
    fn lsb_document_bits_matches_daffodil_bit_order_change() {
        let part1_chunks = bit_chunks_for_part(
            &collect_bit_digits_from_text("01001|011"),
            DocumentByteOrder::Ltr,
        );
        let part2_chunks = bit_chunks_for_part(
            &collect_bit_digits_from_text("010101|00"),
            DocumentByteOrder::Ltr,
        );
        let (bytes, _) = assemble_tdml_document_bytes(
            &[part1_chunks, part2_chunks],
            DocumentBitOrder::LsbFirst,
        );
        assert_eq!(bytes, vec![0x4B, 0x54]);
    }

    #[test]
    fn parse_hex_document_ignores_separators_and_odd_nibbles() {
        use alloc::vec;
        let data = parse_hex_document("A5-E9-FF-00").unwrap();
        assert_eq!(data, vec![0xA5, 0xE9, 0xFF, 0x00]);
        let odd = parse_hex_document("12345").unwrap();
        assert_eq!(odd, vec![0x01, 0x23, 0x45]);
    }
}
