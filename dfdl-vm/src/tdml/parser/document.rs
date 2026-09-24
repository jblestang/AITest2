use crate::error::{ParseError, Result};
use crate::schema::{expand_entities, BitOrder};
use crate::vm::encoding::{bits_charset_spec, encode_document_text};
use crate::xml_util::XmlReader;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::{DocumentKind, TdmlDocument};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentBitOrder {
    MsbFirst,
    LsbFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentByteOrder {
    Ltr,
    Rtl,
}

pub(crate) struct ParsedDocumentPart {
    pub kind: DocumentKind,
    pub data: Vec<u8>,
    pub file_resource: Option<String>,
    pub last_byte_bit_count: Option<u8>,
    pub bit_chunks: Option<Vec<String>>,
    pub data_bit_chunks: Option<Vec<String>>,
    pub bit_order: DocumentBitOrder,
    pub explicit_bit_order: bool,
    pub byte_order: DocumentByteOrder,
    pub length_in_bits: usize,
}

pub(crate) fn parse_document_bit_order(value: &str) -> Result<DocumentBitOrder> {
    match value {
        "MSBFirst" => Ok(DocumentBitOrder::MsbFirst),
        "LSBFirst" => Ok(DocumentBitOrder::LsbFirst),
        other => Err(ParseError::InvalidXml {
            message: alloc::format!("unknown document bitOrder `{other}`"),
        }
        .into()),
    }
}

pub(crate) fn effective_document_bit_order(
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

pub(crate) fn document_bit_order_to_transmission(order: DocumentBitOrder) -> BitOrder {
    match order {
        DocumentBitOrder::LsbFirst => BitOrder::LeastSignificantBitFirst,
        DocumentBitOrder::MsbFirst => BitOrder::MostSignificantBitFirst,
    }
}

pub(crate) fn packed_document_transmission_bit_order(use_lsb_assembly: bool) -> BitOrder {
    if use_lsb_assembly {
        BitOrder::LeastSignificantBitFirst
    } else {
        BitOrder::MostSignificantBitFirst
    }
}

pub(crate) fn check_explicit_part_bit_order_mixture(
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
            message:
                "Must specify bitOrder on document element when parts have a mixture of bit orders."
                    .into(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn check_document_part_bit_order_transitions(
    parts: &[(DocumentBitOrder, usize, bool)],
) -> Result<()> {
    if parts.len() <= 1 {
        return Ok(());
    }
    let mut cumulative = 0usize;
    for (i, (order, len, _)) in parts.iter().enumerate() {
        if i > 0 {
            let prior = parts[i - 1].0;
            if prior != *order && !cumulative.is_multiple_of(8) {
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

pub(crate) fn parse_document(
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
            file_resource: None,
            last_byte_bit_count: None,
            transmission_bit_order: BitOrder::MostSignificantBitFirst,
            document_transmission_bit_order,
            mixed_bits_text_document: false,
            part_bit_order_regions: Vec::new(),
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
        let mut part_bit_order_regions: Vec<(BitOrder, usize)> = Vec::new();
        let mut file_resource: Option<String> = None;

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

        let mut load_error = None;
        while reader.peek_start_local()? == Some("documentPart".to_string()) {
            let part = match parse_document_part(reader, default_bit_order, document_bit_order_from_attr) {
                Ok(p) => p,
                Err(e) => {
                    load_error = Some(e.to_string());
                    while reader.peek_start_local()?.is_some() {
                        let _ = reader.read_inner_xml();
                    }
                    break;
                }
            };
            if let Some(path) = part.file_resource {
                file_resource = Some(path);
                reader.skip_insignificant_ws()?;
                continue;
            }
            part_transitions.push((part.bit_order, part.length_in_bits, part.explicit_bit_order));
            part_bit_order_regions.push((
                document_bit_order_to_transmission(part.bit_order),
                part.length_in_bits,
            ));
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
            } else if part.kind == DocumentKind::Hex {
                if let Some(chunks) = part.bit_chunks.clone() {
                    saw_bits_part = true;
                    kind = DocumentKind::Bits;
                    bit_part_chunks.push(chunks);
                } else if saw_bits_part {
                    kind = DocumentKind::Bits;
                    let chunks: Vec<String> =
                        part.data.iter().map(|b| alloc::format!("{:08b}", b)).collect();
                    for byte in &part.data {
                        for i in (0..8).rev() {
                            pending_bits.push((byte >> i) & 1);
                        }
                    }
                    bit_part_chunks.push(chunks);
                } else {
                    flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
                    if data.is_empty() && !saw_bits_part {
                        kind = part.kind;
                    }
                    data.extend(part.data);
                    last_byte_bit_count = part.last_byte_bit_count;
                }
            } else if saw_bits_part && part.kind == DocumentKind::Text {
                mixed_bits_text_document = true;
                if saw_rtl_byte_order {
                    if let Some(chunks) = part.data_bit_chunks.clone() {
                        bit_part_chunks.push(chunks);
                    } else {
                        flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
                        data.extend(part.data);
                    }
                } else {
                    flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
                    data.extend(part.data);
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
        if load_error.is_none() {
            if let Err(e) = check_explicit_part_bit_order_mixture(document_bit_order_from_attr, &part_transitions) {
                load_error = Some(e.to_string());
            } else if let Err(e) = check_document_part_bit_order_transitions(&part_transitions) {
                load_error = Some(e.to_string());
            }
        }

        if let Some(err_msg) = load_error {
            return Ok(TdmlDocument {
                kind,
                data: Vec::new(),
                file_resource,
                last_byte_bit_count: None,
                transmission_bit_order: BitOrder::MostSignificantBitFirst,
                document_transmission_bit_order,
                mixed_bits_text_document: false,
                part_bit_order_regions: Vec::new(),
                load_error: Some(err_msg),
            });
        }

        let effective_doc_bit_order = effective_document_bit_order(
            default_bit_order,
            document_bit_order_from_attr,
            &part_transitions,
        );

        if !bit_part_chunks.is_empty() {
            let (bits_data, trailing) =
                assemble_tdml_document_bytes(&bit_part_chunks, effective_doc_bit_order);
            data.extend(bits_data);
            if trailing > 0 {
                last_byte_bit_count = Some(trailing);
            }
        } else {
            flush_pending_bits(&mut pending_bits, &mut data, &mut last_byte_bit_count);
        }

        let is_lsb_assembly = effective_doc_bit_order == DocumentBitOrder::LsbFirst;
        let trans_bit_order = document_transmission_bit_order
            .unwrap_or_else(|| packed_document_transmission_bit_order(is_lsb_assembly));
        let has_explicit_bit_order = document_bit_order_from_attr
            || part_transitions.iter().any(|(_, _, explicit)| *explicit);
        let final_part_bit_order_regions = if has_explicit_bit_order {
            part_bit_order_regions
        } else {
            Vec::new()
        };

        return Ok(TdmlDocument {
            kind,
            data,
            file_resource,
            last_byte_bit_count,
            transmission_bit_order: trans_bit_order,
            document_transmission_bit_order,
            mixed_bits_text_document,
            part_bit_order_regions: final_part_bit_order_regions,
            load_error: None,
        });
    }

    let text = reader.read_text_until_end("document")?;
    let replace_entities = doc_attrs
        .get("replaceDFDLEntities")
        .map(|v| v.as_str() == "true")
        .unwrap_or(true);

    let data = if replace_entities {
        expand_entities(&text)
    } else {
        text.into_bytes()
    };

    Ok(TdmlDocument {
        kind: DocumentKind::Text,
        data,
        file_resource: None,
        last_byte_bit_count: None,
        transmission_bit_order: BitOrder::MostSignificantBitFirst,
        document_transmission_bit_order,
        mixed_bits_text_document: false,
        part_bit_order_regions: Vec::new(),
        load_error: None,
    })
}

pub(crate) fn parse_document_part(
    reader: &mut XmlReader<'_>,
    default_bit_order: DocumentBitOrder,
    document_bit_order_from_attr: bool,
) -> Result<ParsedDocumentPart> {
    use crate::xml_util::attrs_to_map;
    use xml_no_std::reader::XmlEvent;

    let XmlEvent::StartElement { attributes, .. } = reader.next_event()? else {
        return Err(ParseError::InvalidXml {
            message: "expected documentPart".into(),
        }
        .into());
    };
    let attrs = attrs_to_map(&attributes);
    let part_type = attrs.get("type").map(String::as_str);
    if part_type == Some("file") {
        if document_bit_order_from_attr {
            return Err(ParseError::InvalidXml {
                message: "bitOrder may not be specified on document parts of type 'file'".into(),
            }
            .into());
        }
        let text = reader.read_text_until_end("documentPart")?;
        let path = text.trim().to_string();
        return Ok(ParsedDocumentPart {
            kind: DocumentKind::Text,
            data: Vec::new(),
            file_resource: Some(path),
            last_byte_bit_count: None,
            bit_chunks: None,
            data_bit_chunks: None,
            bit_order: default_bit_order,
            explicit_bit_order: false,
            byte_order: DocumentByteOrder::Ltr,
            length_in_bits: 0,
        });
    }
    let kind = match part_type {
        Some("hex") | Some("byte") => DocumentKind::Hex,
        Some("bits") => DocumentKind::Bits,
        _ => DocumentKind::Text,
    };

    let explicit_bit_order = attrs.get("bitOrder").is_some();
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

    let replace_entities = attrs
        .get("replaceDFDLEntities")
        .map(|v| v.as_str() == "true")
        .unwrap_or(true);

    let encoding = attrs.get("encoding").map(String::as_str);

    let (data, last_byte_bit_count, bit_chunks, data_bit_chunks) = match kind {
        DocumentKind::Text => {
            let bits_text = if replace_entities {
                crate::schema::expand_entities_str(&text)
            } else {
                text.clone()
            };
            let data = if let Some(enc) = encoding {
                encode_document_text(&bits_text, enc).map_err(|e| ParseError::InvalidXml {
                    message: alloc::format!("documentPart encoding: {e}"),
                })?
            } else if replace_entities {
                expand_entities(&text)
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
        file_resource: None,
        last_byte_bit_count,
        bit_chunks,
        data_bit_chunks,
        bit_order,
        explicit_bit_order,
        byte_order,
        length_in_bits,
    })
}

pub(crate) fn parse_hex_document(text: &str) -> Result<Vec<u8>> {
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

pub(crate) fn collect_bit_digits_from_text(text: &str) -> String {
    text.chars().filter(|c| *c == '0' || *c == '1').collect()
}

pub(crate) fn reverse_bit_string(s: &str) -> String {
    s.chars().rev().collect()
}

pub(crate) fn byte_to_msb_bit_string(b: u8) -> String {
    alloc::format!("{:08b}", b)
}

pub(crate) fn text_document_data_bits(text: &str, encoding: Option<&str>) -> Result<Vec<String>> {
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

pub(crate) fn chunk_bit_string_ltr(bits: &str) -> Vec<String> {
    bits.as_bytes()
        .chunks(8)
        .map(|chunk| core::str::from_utf8(chunk).unwrap_or("").to_string())
        .collect()
}

pub(crate) fn bit_chunks_for_part(bits: &str, byte_order: DocumentByteOrder) -> Vec<String> {
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

pub(crate) fn regroup_bit_digit_strings(chunks: &[String]) -> Vec<String> {
    chunks
        .join("")
        .as_bytes()
        .chunks(8)
        .map(|chunk| core::str::from_utf8(chunk).unwrap_or("").to_string())
        .collect()
}

pub(crate) fn document_bits_byte_strings(
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

pub(crate) fn assemble_tdml_document_bytes(
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

pub(crate) fn pack_bits_msb_first(bits: &[u8]) -> (Vec<u8>, u8) {
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
