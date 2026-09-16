use dfdl_vm::schema::{parse_schema, ComplexContent, Particle, TypeName};

#[test]
fn header_creator_has_ovc() {
    let xsd = r##"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:ex="http://example.com">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="ex:GeneralFormat" lengthKind="delimited"/>
  <xs:group name="hdrGroup"><xs:sequence><xs:element name="hdrblock" type="ex:header"/></xs:sequence></xs:group>
  <xs:complexType name="header"><xs:sequence>
    <xs:element name="Creator" type="xs:string" dfdl:outputValueCalc="{ ' NCSA' }"/>
    <xs:element name="Date" type="xs:string" dfdl:outputValueCalc="{ ' Mon' }"/>
  </xs:sequence></xs:complexType>
  <xs:element name="table"><xs:complexType><xs:sequence>
    <xs:sequence dfdl:hiddenGroupRef="ex:hdrGroup"/>
    <xs:element name="g" type="xs:int"/>
  </xs:sequence></xs:complexType></xs:element>
</xs:schema>"##;
    let doc = parse_schema(xsd).expect("parse");
    let t = doc.resolve_type(&TypeName::new("header")).expect("header type");
    let dfdl_vm::schema::TypeDef::Complex { content, .. } = t else { panic!("complex") };
    let ComplexContent::Sequence(seq) = content else { panic!("seq") };
    let Particle::Element(c) = &seq.particles[0] else { panic!() };
    assert!(c.props.output_value_calc.is_some() || c.props.output_value_calc_literal.is_some() || c.props.output_value_calc_conditional,
        "Creator OVC not parsed: {:?}", c.props);
}
