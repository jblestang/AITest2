use dfdl_vm::schema::{parse_schema, ComplexContent, Particle, TypeDef, get_global_element};

#[test]
fn nested_sequence_hidden_group_ref_attribute() {
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
    assert_eq!(outer.props.separator.as_deref(), Some("|"), "outer: {:?}", outer.props);
    assert_eq!(outer.particles.len(), 2);
    let Particle::Sequence(inner) = &outer.particles[0] else {
        panic!("inner");
    };
    assert_eq!(
        inner.props.hidden_group_ref.as_deref(),
        Some("aGroup"),
        "inner props: {:?}",
        inner.props
    );
}
