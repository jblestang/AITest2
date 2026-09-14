use dfdl_vm::ir::{compile_named_with_tunables, IrNode};
use dfdl_vm::length_validate::DaffodilTunables;
use dfdl_vm::schema::{parse_schema, ObjectKind};
use dfdl_vm::tdml::parse_tdml;

#[test]
fn dfdlx_object_kind_attr_parses() {
    let xsd = r##"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/"
           xmlns:dfdlx="http://www.ogf.org/dfdl/dfdl-1.0/extensions">
  <xs:element name="b" type="xs:anyURI" dfdl:lengthKind="explicit" dfdl:length="1" dfdlx:objectKind="bytes"/>
</xs:schema>"##;
    let schema = parse_schema(xsd).expect("parse");
    let el = schema.global_elements.get("b").expect("b");
    assert_eq!(el.props.object_kind, Some(ObjectKind::Bytes));
}

#[test]
fn blob_01_root_has_object_kind_bytes() {
    let tdml = include_str!(
        "../../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/simple_types/Blobs.tdml"
    );
    let suite = parse_tdml(tdml).expect("tdml");
    let xsd = &suite.schemas.get("Blob.dfdl.xsd").expect("schema").xsd;
    let schema = parse_schema(xsd).expect("parse");
    let el = schema.global_elements.get("blob_01").expect("blob_01");
    assert_eq!(el.props.length_kind, Some(dfdl_vm::schema::LengthKind::Explicit));
    assert_eq!(
        el.props.object_kind,
        Some(ObjectKind::Bytes),
        "object_kind missing on blob_01"
    );

    let program =
        compile_named_with_tunables(&schema, Some("blob_01"), DaffodilTunables::default())
            .expect("compile");
    let node = program
        .nodes
        .iter()
        .find_map(|n| {
            if let IrNode::Element { props, .. } = n {
                Some(props)
            } else {
                None
            }
        })
        .expect("root element");
    assert_eq!(node.object_kind, ObjectKind::Bytes);
}
