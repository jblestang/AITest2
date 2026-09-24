#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;
    use crate::schema::{ComplexContent, Particle, TypeDef};
    use alloc::collections::BTreeMap;

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
    fn parse_sibling_length_with_adjustment() {
        assert_eq!(
            parse_sibling_length_expr("{ ../messageLength - 8 }"),
            Some(("messageLength".into(), false, -8))
        );
    }

    #[test]
    fn parse_initiated_content_property() {
        let mut attrs = BTreeMap::new();
        attrs.insert("initiatedContent".into(), "yes".into());
        let props = props_from_attrs(&attrs).expect("props");
        assert_eq!(props.initiated_content, Some(true));
        let (xsd, dfdl) = split_dfdl_attrs(
            "sequence",
            &BTreeMap::from([("dfdl:initiatedContent".to_string(), "yes".to_string())]),
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
        assert_eq!(parse_assert_int_eq_test("{ xs:int(.) eq 42 }"), Some(42));
    }

    #[test]
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
        let crate::ir::IrNode::Element {
            child: Some(child), ..
        } = root_node
        else {
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
  <xs:element name="propertyForm" type="xs:string" dfdl:lengthKind="explicit" dfdl:length="1">
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
        let doc = parse_schema(xsd).expect("parse");
        let err = crate::ir::compile_named(&doc, Some("e")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot have children"), "{msg}");
    }

    #[test]
    fn input_value_calc_float_div_expression() {
        assert!(parse_input_value_calc_expression(
            "{ xs:float(xs:int(../ex:e2) div xs:int(../ex:e3)) }"
        )
        .is_some());
    }
}
