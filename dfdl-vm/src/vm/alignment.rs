use crate::ir::{IrProps, ValueKind};
use crate::schema::{BinaryNumberRep, LengthUnits, Representation};
use crate::vm::runtime::Cursor;

/// Skip count in bits for `dfdl:leadingSkip` / `dfdl:trailingSkip` (uses `dfdl:alignmentUnits`).
pub fn skip_units_to_bits(props: &IrProps, skip: u64) -> usize {
    match props.alignment_units {
        LengthUnits::Bits => skip as usize,
        LengthUnits::Bytes => skip as usize * 8,
        LengthUnits::Characters => skip as usize * 8,
    }
}

pub fn implicit_alignment_in_bits(kind: ValueKind, props: &IrProps, encoding: &str) -> usize {
    if props.representation == Representation::Text {
        if kind == ValueKind::String {
            return text_encoding_alignment_bits(encoding);
        }
        // Numeric and other text primitives use 8-bit alignment (DFDL-12-019R), not UTF-16 width.
        return 8;
    }
    if kind == ValueKind::Complex {
        return match props.alignment_units {
            LengthUnits::Bits => 1,
            LengthUnits::Bytes | LengthUnits::Characters => 8,
        };
    }
    let packed = matches!(
        props.binary_number_rep,
        BinaryNumberRep::PackedBcd | BinaryNumberRep::Bcd | BinaryNumberRep::Ibm4690Packed
    );
    match kind {
        ValueKind::Float | ValueKind::Boolean => 32,
        ValueKind::Double => 64,
        ValueKind::HexBinary => 8,
        ValueKind::Long => {
            if packed {
                8
            } else {
                64
            }
        }
        ValueKind::Integer => {
            if packed {
                8
            } else {
                8
            }
        }
        ValueKind::Int | ValueKind::UnsignedInt => {
            if packed {
                8
            } else {
                32
            }
        }
        ValueKind::Short | ValueKind::UnsignedShort => {
            if packed {
                8
            } else {
                16
            }
        }
        ValueKind::Byte | ValueKind::UnsignedByte | ValueKind::Decimal => 8,
        ValueKind::DateTime | ValueKind::Time => 64,
        ValueKind::String => text_encoding_alignment_bits(encoding),
        ValueKind::Complex => 8,
    }
}

fn text_encoding_alignment_bits(encoding: &str) -> usize {
    if let Some(width) = crate::vm::encoding::bits_charset_code_unit_width(encoding) {
        return width as usize;
    }
    let _enc = encoding.to_ascii_uppercase();
    8
}

/// Whether implicit/explicit pre-element alignment runs before a binary bit-length field.
pub fn pre_element_alignment_applies(kind: ValueKind, props: &IrProps, encoding: &str) -> bool {
    use crate::schema::{LengthKind, LengthUnits, Representation};
    if kind == ValueKind::HexBinary
        && props.length_units == LengthUnits::Bits
        && matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed)
    {
        return false;
    }
    if props.representation != Representation::Binary {
        return true;
    }
    if !matches!(props.length_kind, LengthKind::Explicit | LengthKind::Fixed) {
        return true;
    }
    if props.length_units != LengthUnits::Bits {
        return true;
    }
    let Some(len) = props.length else {
        return true;
    };
    if !props.alignment_implicit {
        return true;
    }
    let implicit_bits = implicit_alignment_in_bits(kind, props, encoding);
    if (len as usize) <= implicit_bits && implicit_bits <= 8 {
        return false;
    }
    true
}

/// Resolved `(alignment, alignment_units)` for consume/write alignment helpers.
pub fn resolved_alignment(kind: ValueKind, props: &IrProps, encoding: &str) -> (u64, LengthUnits) {
    if !props.alignment_implicit {
        return (props.alignment, props.alignment_units);
    }
    let bits = implicit_alignment_in_bits(kind, props, encoding);
    match props.alignment_units {
        LengthUnits::Bits => (bits as u64, LengthUnits::Bits),
        LengthUnits::Bytes | LengthUnits::Characters => {
            ((bits / 8).max(1) as u64, LengthUnits::Bytes)
        }
    }
}

/// Post-data alignment after reading a simple element (`dfdl:framingAlignment` only).
pub fn post_read_alignment(props: &IrProps) -> (u64, LengthUnits) {
    if props.framing_alignment != 0 {
        return (props.framing_alignment, props.framing_alignment_units);
    }
    (1, LengthUnits::Bits)
}

pub(crate) fn cursor_uses_bitstream_alignment(cursor: &Cursor<'_>) -> bool {
    cursor.frame_bit_limit.is_some() || cursor.bit_count != 0
}

/// Align the bit stream to the encoding boundary before reading text delimiters or text data.
pub fn align_cursor_to_text_encoding(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
    encoding: &str,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    let enc_bits = text_encoding_alignment_bits(encoding);
    if enc_bits <= 1 {
        return Ok(());
    }
    let pos = cursor.absolute_bit_index();
    let skip = (enc_bits - (pos % enc_bits)) % enc_bits;
    if skip > 0 {
        cursor
            .skip_stream_bits(skip, props.bit_order)
            .map_err(|_| VmError::UnexpectedEof)?;
    }
    Ok(())
}

/// Daffodil default tunable for `dfdl:leadingSkip` / `dfdl:trailingSkip` property values.
pub const LEADING_TRAILING_SKIP_PROPERTY_LIMIT: u64 = 1024;

pub fn consume_leading_skip(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if props.leading_skip == 0 {
        return Ok(());
    }
    if props.leading_skip > LEADING_TRAILING_SKIP_PROPERTY_LIMIT {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Tunable Limit Exceeded Error: Property leadingSkip {} is larger than limit {}",
                props.leading_skip,
                LEADING_TRAILING_SKIP_PROPERTY_LIMIT
            ),
        });
    }
    let skip = skip_units_to_bits(props, props.leading_skip);
    if skip == 0 {
        return Ok(());
    }
    cursor
        .skip_stream_bits(skip, props.bit_order)
        .map_err(|_| VmError::UnexpectedEof)
}

pub fn write_leading_skip(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::vm::runtime::write_stream_bit;
    if props.leading_skip == 0 {
        return Ok(());
    }
    let skip = skip_units_to_bits(props, props.leading_skip);
    for _ in 0..skip {
        write_stream_bit(out, bit_count, 0, props.bit_order);
    }
    Ok(())
}

pub fn write_trailing_skip(
    out: &mut alloc::vec::Vec<u8>,
    bit_count: &mut u8,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    use crate::vm::runtime::{write_byte_aligned, write_stream_bit};
    if props.trailing_skip == 0 {
        return Ok(());
    }
    if props.trailing_skip > LEADING_TRAILING_SKIP_PROPERTY_LIMIT {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Tunable Limit Exceeded Error: Property trailingSkip {} is larger than limit {}",
                props.trailing_skip,
                LEADING_TRAILING_SKIP_PROPERTY_LIMIT
            ),
        });
    }
    let skip_bits = skip_units_to_bits(props, props.trailing_skip);
    if skip_bits == 0 {
        return Ok(());
    }
    if props.alignment_units == LengthUnits::Bytes && *bit_count != 0 {
        if !props.fill_byte_defined {
            return Err(VmError::InvalidValue {
                message: "Schema Definition Error: Property fillByte is not defined".into(),
            });
        }
        while *bit_count != 0 {
            write_stream_bit(out, bit_count, props.fill_byte & 1, props.bit_order);
        }
    }
    if props.alignment_units == LengthUnits::Bytes && *bit_count == 0 && skip_bits.is_multiple_of(8)
    {
        let nbytes = skip_bits / 8;
        write_byte_aligned(out, bit_count, &alloc::vec![props.fill_byte; nbytes])?;
        return Ok(());
    }
    for _ in 0..skip_bits {
        write_stream_bit(out, bit_count, props.fill_byte & 1, props.bit_order);
    }
    Ok(())
}

pub fn consume_trailing_skip(
    cursor: &mut Cursor<'_>,
    props: &IrProps,
) -> Result<(), crate::error::VmError> {
    use crate::error::VmError;
    if props.trailing_skip == 0 {
        return Ok(());
    }
    if props.trailing_skip > LEADING_TRAILING_SKIP_PROPERTY_LIMIT {
        return Err(VmError::InvalidValue {
            message: alloc::format!(
                "Tunable Limit Exceeded Error: Property trailingSkip {} is larger than limit {}",
                props.trailing_skip,
                LEADING_TRAILING_SKIP_PROPERTY_LIMIT
            ),
        });
    }
    let skip = skip_units_to_bits(props, props.trailing_skip);
    if skip == 0 {
        return Ok(());
    }
    cursor
        .skip_stream_bits(skip, props.bit_order)
        .map_err(|_| VmError::UnexpectedEof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{compile_named, IrNode};
    use crate::schema::parse_schema;
    use crate::vm::runtime::Cursor;

    #[test]
    fn consume_leading_skip_advances_bit_cursor() {
        let mut props = crate::ir::IrProps::default();
        props.leading_skip = 4;
        props.alignment_units = LengthUnits::Bits;
        let mut cursor = Cursor::new(&[0x0E_u8]);
        consume_leading_skip(&mut cursor, &props).expect("skip");
        assert_eq!(cursor.absolute_bit_index(), 4);
    }

    #[test]
    fn compiled_simple_type_element_has_leading_skip_in_ir() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
            xmlns:ex="http://example.com">
          <dfdl:format representation="binary" encoding="utf-8" alignmentUnits="bits"/>
          <xs:simpleType name="uByte2Bits" dfdl:lengthKind="explicit" dfdl:lengthUnits="bits"
            dfdl:length="2" dfdl:leadingSkip="4">
            <xs:restriction base="xs:unsignedByte"/>
          </xs:simpleType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="one" type="ex:uByte2Bits"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let program = compile_named(&schema, Some("root")).expect("compile");
        let skip = program
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element { name, props, .. }
                    if program.strings.get(*name).ok() == Some("one") =>
                {
                    Some(props.leading_skip)
                }
                _ => None,
            })
            .expect("one");
        assert_eq!(skip, 4);
    }

    #[test]
    fn consume_element_framing_uses_type_leading_skip() {
        use crate::vm::runtime::consume_element_framing;
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
            xmlns:ex="http://example.com">
          <dfdl:format representation="binary" encoding="utf-8" alignmentUnits="bits"/>
          <xs:simpleType name="uByte2Bits" dfdl:lengthKind="explicit" dfdl:lengthUnits="bits"
            dfdl:length="2" dfdl:leadingSkip="4">
            <xs:restriction base="xs:unsignedByte"/>
          </xs:simpleType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="one" type="ex:uByte2Bits"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let program = compile_named(&schema, Some("root")).expect("compile");
        let (props, kind) = program
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element {
                    name, props, kind, ..
                } if program.strings.get(*name).ok() == Some("one") => Some((props.clone(), *kind)),
                _ => None,
            })
            .expect("one");
        assert_eq!(props.leading_skip, 4);
        let mut cursor = Cursor::new(&[0x0E_u8]);
        let enc = program.strings.get(props.encoding).expect("enc");
        consume_element_framing(&mut cursor, &props, kind, enc).expect("framing");
        assert_eq!(cursor.absolute_bit_index(), 4);
    }

    #[test]
    fn resolve_length_props_preserves_leading_skip() {
        use crate::vm::decoder::resolve_length_props_for_test;
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
            xmlns:ex="http://example.com">
          <dfdl:format representation="binary" encoding="utf-8" alignmentUnits="bits"/>
          <xs:simpleType name="uByte2Bits" dfdl:lengthKind="explicit" dfdl:lengthUnits="bits"
            dfdl:length="2" dfdl:leadingSkip="4">
            <xs:restriction base="xs:unsignedByte"/>
          </xs:simpleType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="one" type="ex:uByte2Bits"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let program = compile_named(&schema, Some("root")).expect("compile");
        let (props, kind) = program
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element {
                    name, props, kind, ..
                } if program.strings.get(*name).ok() == Some("one") => Some((props.clone(), *kind)),
                _ => None,
            })
            .expect("one");
        let resolved = resolve_length_props_for_test(
            &props,
            kind,
            &program.strings,
            &crate::length_validate::DaffodilTunables::default(),
        )
        .expect("resolve");
        assert_eq!(resolved.leading_skip, 4);
    }

    #[test]
    fn tdml_hb_root_has_implicit_alignment() {
        use crate::tdml::parse_tdml;
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let def = suite
            .schemas
            .get("implicitAlignmentSchema")
            .expect("schema");
        let schema = crate::schema::parse_schema_with_options(
            &def.xsd,
            &crate::schema::ParseOptions {
                base_dir: def.compile_base_dir.clone(),
                schema_label: None,
            },
        )
        .expect("parse");
        let program = compile_named(&schema, Some("hB")).expect("compile");
        let root = program.node(program.root).expect("root");
        let IrNode::Element { props, kind, .. } = root else {
            panic!("root not element");
        };
        assert_eq!(*kind, crate::ir::ValueKind::HexBinary);
        assert!(props.alignment_implicit, "hB alignment=implicit");
        assert_eq!(props.leading_skip, 4);
    }

    #[test]
    fn sequence_references_merged_one_element() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
            xmlns:ex="http://example.com">
          <dfdl:format representation="binary" encoding="utf-8" alignmentUnits="bits"/>
          <xs:simpleType name="uByte2Bits" dfdl:lengthKind="explicit" dfdl:lengthUnits="bits"
            dfdl:length="2" dfdl:leadingSkip="4">
            <xs:restriction base="xs:unsignedByte"/>
          </xs:simpleType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="one" type="ex:uByte2Bits"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let program = compile_named(&schema, Some("root")).expect("compile");
        let root = program.node(program.root).expect("root");
        let IrNode::Element {
            child: Some(seq_id),
            ..
        } = root
        else {
            panic!("root not complex");
        };
        let seq = program.node(*seq_id).expect("seq");
        let IrNode::Sequence { children, .. } = seq else {
            panic!("not sequence");
        };
        assert_eq!(children.len(), 1);
        let child = program.node(children[0]).expect("child");
        let IrNode::Element { name, props, .. } = child else {
            panic!("child not element");
        };
        assert_eq!(program.strings.get(*name).ok(), Some("one"));
        assert_eq!(props.leading_skip, 4);
    }

    #[test]
    fn tdml_alignment02_first_field_manual() {
        use crate::length_validate::DaffodilTunables;
        use crate::tdml::parse_tdml;
        use crate::vm::runtime::{consume_element_framing, read_binary_scalar, Cursor};
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == "alignment02")
            .expect("t");
        let def = suite.schemas.get("alignmentSchema").expect("schema");
        let schema = crate::schema::parse_schema_with_options(
            &def.xsd,
            &crate::schema::ParseOptions {
                base_dir: def.compile_base_dir.clone(),
                schema_label: None,
            },
        )
        .expect("parse");
        let program = compile_named(&schema, Some("e3")).expect("ir");
        let root = program.node(program.root).expect("root");
        let IrNode::Element {
            child: Some(seq_id),
            ..
        } = root
        else {
            panic!("root");
        };
        let seq = program.node(*seq_id).expect("seq");
        let IrNode::Sequence { children, .. } = seq else {
            panic!("seq");
        };
        let child = program.node(children[0]).expect("child");
        let IrNode::Element {
            name, kind, props, ..
        } = child
        else {
            panic!("child");
        };
        assert_eq!(program.strings.get(*name).ok(), Some("one"));
        assert_eq!(props.leading_skip, 4);
        let doc = &test.documents[0];
        let mut cursor =
            Cursor::with_frame_bits(&doc.data, doc.significant_bit_length().expect("bits"));
        let enc = program.strings.get(props.encoding).expect("enc");
        consume_element_framing(&mut cursor, props, *kind, enc).expect("framing");
        assert_eq!(cursor.absolute_bit_index(), 4);
        let v = read_binary_scalar(
            &mut cursor,
            *kind,
            props,
            &program.strings,
            false,
            &[],
            Some("one"),
            &DaffodilTunables::default(),
            None,
        )
        .expect("read");
        assert!(matches!(v, crate::value::DfdlValue::UnsignedByte(3)));
    }

    #[test]
    fn tdml_hb_delimited_after_framing() {
        use crate::tdml::parse_tdml;
        use crate::vm::runtime::{consume_element_framing, read_delimited_bytes, Cursor};
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == "impAlignmentHexBinary")
            .expect("t");
        let def = suite
            .schemas
            .get("implicitAlignmentSchema")
            .expect("schema");
        let schema = crate::schema::parse_schema_with_options(
            &def.xsd,
            &crate::schema::ParseOptions {
                base_dir: def.compile_base_dir.clone(),
                schema_label: None,
            },
        )
        .expect("parse");
        let program = compile_named(&schema, Some("hB")).expect("ir");
        let root = program.node(program.root).expect("root");
        let IrNode::Element { props, kind, .. } = root else {
            panic!("root");
        };
        let doc = &test.documents[0];
        let mut cursor =
            Cursor::with_frame_bits(&doc.data, doc.significant_bit_length().expect("bits"));
        let enc = program.strings.get(props.encoding).expect("enc");
        consume_element_framing(&mut cursor, props, *kind, enc).expect("framing");
        assert_eq!(cursor.absolute_bit_index(), 8, "skip+implicit align");
        let bytes =
            read_delimited_bytes(&mut cursor, props, &program.strings, false, &[]).expect("delim");
        assert_eq!(bytes, [0xF4], "got {bytes:02x?}");
    }

    #[test]
    fn tdml_alignment03_e4_one_props_and_manual() {
        use crate::length_validate::DaffodilTunables;
        use crate::tdml::parse_tdml;
        use crate::value::DfdlValue;
        use crate::vm::runtime::{consume_element_framing, read_simple, Cursor};
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == "alignment03")
            .expect("t");
        let def = suite.schemas.get("alignmentSchema").expect("schema");
        let schema = crate::schema::parse_schema_with_options(
            &def.xsd,
            &crate::schema::ParseOptions {
                base_dir: def.compile_base_dir.clone(),
                schema_label: None,
            },
        )
        .expect("parse");
        let program = compile_named(&schema, Some("e4")).expect("ir");
        let root = program.node(program.root).expect("root");
        let IrNode::Element {
            child: Some(seq_id),
            ..
        } = root
        else {
            panic!("root");
        };
        let seq = program.node(*seq_id).expect("seq");
        let IrNode::Sequence { children, .. } = seq else {
            panic!("seq");
        };
        let child = program.node(children[0]).expect("one");
        let IrNode::Element { props, kind, .. } = child else {
            panic!("one");
        };
        assert_eq!(props.alignment, 8, "element pre-align");
        assert_eq!(props.framing_alignment, 4, "type post-align");
        assert_eq!(props.leading_skip, 4);
        let doc = &test.documents[0];
        let mut cursor =
            Cursor::with_frame_bits(&doc.data, doc.significant_bit_length().expect("bits"));
        let enc = program.strings.get(props.encoding).unwrap();
        let tunables = DaffodilTunables::default();
        consume_element_framing(&mut cursor, props, *kind, enc).expect("framing");
        assert_eq!(cursor.absolute_bit_index(), 8);
        let v = read_simple(
            &mut cursor,
            *kind,
            props,
            &program.strings,
            false,
            &[],
            None,
            &tunables,
            true,
            None,
            None,
            true,
            false,
            None,
        )
        .expect("read");
        assert!(matches!(v, DfdlValue::UnsignedByte(1)));
        assert_eq!(
            cursor.absolute_bit_index(),
            12,
            "post framing align to 4-bit boundary"
        );
    }

    #[test]
    fn tdml_implicit_unsigned_long_framing_cursor() {
        use crate::length_validate::DaffodilTunables;
        use crate::tdml::parse_tdml;
        use crate::vm::runtime::{consume_element_framing, read_binary_scalar, Cursor};
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == "implicitAlignmentUnsignedLong")
            .expect("t");
        let def = suite
            .schemas
            .get("implicitAlignmentSchema")
            .expect("schema");
        let schema = crate::schema::parse_schema_with_options(
            &def.xsd,
            &crate::schema::ParseOptions {
                base_dir: def.compile_base_dir.clone(),
                schema_label: None,
            },
        )
        .expect("parse");
        let program = compile_named(&schema, Some("uLong")).expect("ir");
        let root = program.node(program.root).expect("root");
        let IrNode::Element { props, kind, .. } = root else {
            panic!("root");
        };
        assert!(
            props.alignment_implicit,
            "uLong implicit align, got {}",
            props.alignment
        );
        assert_eq!(props.alignment_units, LengthUnits::Bits);
        let doc = &test.documents[0];
        let mut cursor =
            Cursor::with_frame_bits(&doc.data, doc.significant_bit_length().expect("bits"));
        let enc = program.strings.get(props.encoding).unwrap();
        consume_element_framing(&mut cursor, props, *kind, enc).expect("framing");
        assert_eq!(cursor.absolute_bit_index(), 64, "after skip+align");
        let v = read_binary_scalar(
            &mut cursor,
            *kind,
            props,
            &program.strings,
            false,
            &[],
            None,
            &DaffodilTunables::default(),
            None,
        )
        .expect("read");
        assert!(matches!(
            v,
            crate::value::DfdlValue::UnsignedLong(12_345_678)
        ));
    }

    fn tdml_implicit_unsigned_long_document_layout() {
        use crate::tdml::parse_tdml;
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let test = suite
            .tests
            .iter()
            .find(|t| t.name == "implicitAlignmentUnsignedLong")
            .expect("t");
        let doc = &test.documents[0];
        assert_eq!(doc.data.len(), 16, "packed bits + byte long");
        assert_eq!(doc.significant_bit_length(), Some(128));
    }

    fn tdml_explicit_no_skips03_e7_props() {
        use crate::tdml::parse_tdml;
        const TDML: &str = include_str!(
            "../../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section12/aligned_data/Aligned_Data.tdml"
        );
        let suite = parse_tdml(TDML).expect("tdml");
        let def = suite.schemas.get("alignmentSchema").expect("schema");
        let schema = crate::schema::parse_schema_with_options(
            &def.xsd,
            &crate::schema::ParseOptions {
                base_dir: def.compile_base_dir.clone(),
                schema_label: None,
            },
        )
        .expect("parse");
        let program = compile_named(&schema, Some("e7")).expect("ir");
        let root = program.node(program.root).expect("root");
        let IrNode::Element {
            child: Some(seq_id),
            ..
        } = root
        else {
            panic!("root");
        };
        let seq = program.node(*seq_id).expect("seq");
        let IrNode::Sequence { children, .. } = seq else {
            panic!("seq");
        };
        let names = ["one", "two", "three"];
        let expected = [(6u64, 1u64), (3, 2), (12, 1)];
        for (idx, &id) in children.iter().enumerate() {
            let IrNode::Element { name, props, .. } = program.node(id).expect("el") else {
                panic!("el");
            };
            assert_eq!(program.strings.get(*name).ok(), Some(names[idx]));
            assert_eq!(props.length, Some(expected[idx].0));
            assert_eq!(props.alignment, expected[idx].1);
            assert_eq!(props.framing_alignment, 4);
            assert_eq!(props.framing_alignment_units, LengthUnits::Bits);
            assert_eq!(props.length_units, LengthUnits::Bits);
            assert_eq!(props.alignment_units, LengthUnits::Bytes);
        }
    }
}
