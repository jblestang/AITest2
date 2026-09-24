#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::compile_named;
    use crate::ir::IrNode;
    use crate::schema::parse_schema;
    use crate::value::DfdlValue;

    #[test]
    fn test05_choice_dispatch_ir() {
        use crate::schema::parse_schema_with_options;
        use std::fs;
        use std::path::Path;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section06/namespaces");
        let xsd = fs::read_to_string(dir.join("test05sch1.dfdl.xsd")).unwrap();
        let schema = parse_schema_with_options(
            &xsd,
            &crate::schema::ParseOptions {
                base_dir: Some(dir.to_string_lossy().into()),
                schema_label: Some("test05sch1.dfdl.xsd".into()),
            },
        )
        .expect("parse");
        let ty = schema
            .resolve_type(&crate::schema::TypeName::new("rootType"))
            .unwrap();
        if let crate::schema::TypeDef::Complex { content, .. } = ty {
            if let crate::schema::ComplexContent::Sequence(seq) = content {
                if let crate::schema::Particle::Choice(ch) = &seq.particles[1] {
                    assert_eq!(ch.props.choice_dispatch_sibling.as_deref(), Some("elem"));
                    if let crate::schema::Particle::Element(el) = &ch.branches[0] {
                        assert_eq!(el.props.choice_branch_key.as_deref(), Some("1"));
                    }
                }
            }
        }
        let program = compile_named(&schema, Some("root")).expect("compile");
        let root = program.node(program.root).unwrap();
        if let IrNode::Element {
            child: Some(id), ..
        } = root
        {
            if let IrNode::Sequence { children, .. } = program.node(*id).unwrap() {
                if let IrNode::Choice { branches, props } = program.node(children[1]).unwrap() {
                    assert!(props.choice_dispatch_sibling.is_some());
                    assert!(branches[0].branch_key.is_some());
                }
            }
        }
        let spec = crate::api::DfdlSpec::from_schema_root_with_tunables(
            schema,
            Some("root"),
            Default::default(),
        )
        .expect("spec");
        let val = spec.decode(b"1ab").expect("decode");
        let DfdlValue::Sequence(root) = val else {
            panic!("expected root sequence");
        };
        let inner = root.fields.get("root").expect("root element");
        let DfdlValue::Sequence(fields) = inner else {
            panic!("expected root complex");
        };
        assert!(fields.fields.contains_key("elem1"));
        assert!(!fields.fields.contains_key("elem1_fields"));
    }

    #[test]
    fn compile_record_schema() {
        let xsd = include_str!("../../../tests/fixtures/record.xsd");
        let schema = parse_schema(xsd).expect("parse");
        let program = super::super::compile(&schema).expect("compile");
        assert_eq!(program.root_element, "Record");
        assert!(!program.nodes.is_empty());
    }

    #[test]
    fn text_message_tag_has_fixed_length() {
        use crate::schema::{LengthKind, Representation};
        let xsd = include_str!("../../../tests/fixtures/text_message.xsd");
        let schema = parse_schema(xsd).expect("parse");
        let ty = schema
            .resolve_type(&crate::schema::TypeName::new("MessageType"))
            .unwrap();
        if let crate::schema::TypeDef::Complex { content, .. } = ty {
            if let crate::schema::ComplexContent::Sequence(seq) = content {
                let tag = &seq.particles[0];
                if let crate::schema::Particle::Element(el) = tag {
                    assert_eq!(el.props.length_kind, Some(LengthKind::Fixed));
                    assert_eq!(el.props.length, Some(3));
                    assert_eq!(el.props.representation, Some(Representation::Text));
                }
            }
        }
        let program = super::super::compile(&schema).expect("compile");
        let tag_node = program
            .nodes
            .iter()
            .find_map(|n| {
                if let IrNode::Element { name, props, .. } = n {
                    if program.strings.get(*name).ok() == Some("tag") {
                        return Some(props.clone());
                    }
                }
                None
            })
            .expect("tag node");
        assert_eq!(tag_node.length_kind, LengthKind::Fixed);
        assert_eq!(tag_node.length, Some(3));
    }

    #[test]
    fn simple_type_leading_skip_on_referenced_element() {
        use crate::schema::LengthUnits;
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
        let schema = crate::schema::parse_schema(xsd).expect("parse");
        let program = compile_named(&schema, Some("root")).expect("compile");
        let one = program
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element { name, props, .. }
                    if program.strings.get(*name).ok() == Some("one") =>
                {
                    Some(props.clone())
                }
                _ => None,
            })
            .expect("one");
        assert_eq!(one.leading_skip, 4, "leadingSkip from simpleType");
        assert_eq!(one.length_units, LengthUnits::Bits);
        assert_eq!(one.length, Some(2));
    }

    #[test]
    fn simple_type_leading_skip_decodes() {
        use crate::DfdlSpec;
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
        let spec = DfdlSpec::from_schema_root_with_tunables(
            schema,
            Some("root"),
            crate::length_validate::DaffodilTunables::default(),
        )
        .expect("spec");
        let one_props = spec
            .program()
            .nodes
            .iter()
            .find_map(|n| match n {
                IrNode::Element { name, props, .. }
                    if spec.program().strings.get(*name).ok() == Some("one") =>
                {
                    Some(props.leading_skip)
                }
                _ => None,
            })
            .expect("one props");
        assert_eq!(one_props, 4);
        let data = [0x0E_u8];
        let value = spec.decode(&data).expect("decode");
        let crate::value::DfdlValue::Sequence(fields) = value else {
            panic!("expected sequence");
        };
        let inner = match fields.fields.get("root") {
            Some(crate::value::DfdlValue::Sequence(seq)) => seq,
            other => panic!("expected root sequence, got {other:?}"),
        };
        assert_eq!(
            inner.fields.get("one"),
            Some(&crate::value::DfdlValue::UnsignedByte(3))
        );
    }

    #[test]
    fn fixed_occurs_min_max_mismatch_is_sde() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
            xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:ex="http://example.com">
          <dfdl:format representation="text" encoding="utf-8" occursCountKind="fixed"
            lengthKind="delimited" separatorSuppressionPolicy="never"/>
          <xs:complexType name="basicType">
            <xs:sequence dfdl:separator="|">
              <xs:element name="data" type="xs:int"
                dfdl:occursCountKind="fixed" minOccurs="2" maxOccurs="3"
                dfdl:textNumberRep="standard" dfdl:lengthKind="delimited" />
            </xs:sequence>
          </xs:complexType>
          <xs:element name="root">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="basic" type="ex:basicType" dfdl:lengthKind="implicit"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>"#;
        let schema = parse_schema(xsd).expect("parse");
        let err = compile_named(&schema, Some("root")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("occursCountKind='fixed'"), "{msg}");
        assert!(msg.contains("equal"), "{msg}");
    }
}
