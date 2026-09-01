use std::collections::BTreeMap;
use dfdl_vm::{DfdlSpec, DfdlValue};

#[test]
fn binary_record_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/record.xsd")).expect("spec");
    let input = [0x00, 0x00, 0x00, 0x2A, 0x03];
    let decoded = spec.decode(&input).expect("decode");
    assert_eq!(decoded.field("id"), Some(&DfdlValue::UnsignedInt(42)));
    assert_eq!(decoded.field("flags"), Some(&DfdlValue::UnsignedByte(3)));

    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, input);
}

#[test]
fn binary_record_encode_from_value() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/record.xsd")).expect("spec");
    let mut fields = BTreeMap::new();
    fields.insert("id".into(), DfdlValue::UnsignedInt(42));
    fields.insert("flags".into(), DfdlValue::UnsignedByte(3));
    let encoded = spec
        .encode(&DfdlValue::sequence(fields))
        .expect("encode");
    assert_eq!(encoded, vec![0x00, 0x00, 0x00, 0x2A, 0x03]);
}

#[test]
fn text_message_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/text_message.xsd")).expect("spec");
    let input = b"TAG123;";
    let decoded = spec.decode(input).expect("decode");
    assert_eq!(decoded.field("tag"), Some(&DfdlValue::string("TAG")));
    assert_eq!(decoded.field("value"), Some(&DfdlValue::Int(123)));

    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, input);
}

#[test]
fn choice_initiator_decode() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/choice_initiator.xsd")).expect("spec");
    let decoded = spec.decode(b"1=abc").expect("decode branch A");
    assert_eq!(decoded.field("A"), Some(&DfdlValue::string("abc")));

    let decoded_b = spec.decode(b"2=xyz").expect("decode branch B");
    assert_eq!(decoded_b.field("B"), Some(&DfdlValue::string("xyz")));
}

#[test]
fn initiator_before_delimited_float() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/initiator_delimited.xsd")).expect("spec");
    let decoded = spec.decode(b"30,$17.99").expect("decode");
    assert_eq!(decoded.field("qty"), Some(&DfdlValue::Int(30)));
    let price = decoded.field("price").expect("price");
    match price {
        DfdlValue::Float(v) => assert!((*v - 17.99).abs() < 0.001),
        other => panic!("expected float, got {other:?}"),
    }
}

#[test]
fn initiator_before_delimited_float_ref() {
    let spec = DfdlSpec::from_xsd_root(include_str!("fixtures/initiator_delimited_ref.xsd"), Some("Row")).expect("spec");
    let decoded = spec.decode(b"30,$17.99").expect("decode");
    assert_eq!(decoded.field("qty"), Some(&DfdlValue::Int(30)));
    match decoded.field("price").expect("price") {
        DfdlValue::Float(v) => assert!((v - 17.99).abs() < 0.001),
        other => panic!("expected float, got {other:?}"),
    }
}

#[test]
fn initiator_list_ref_row() {
    let spec =
        DfdlSpec::from_xsd_root(include_str!("fixtures/initiator_item_ref.xsd"), Some("list"))
            .expect("spec");
    spec.decoder()
        .decode(b"Shirts,Sold on Monday,30,$17.99")
        .expect("decode one item");
    spec.decoder()
        .decode(b"Shirts,Sold on Monday,30,$17.99||Shoes,Sold on Tuesday,23,$89.99")
        .expect("decode two items");
}

#[test]
fn initiator_item_ref_row() {
    let spec =
        DfdlSpec::from_xsd_root(include_str!("fixtures/initiator_item_ref.xsd"), Some("Item"))
            .expect("spec");
    spec.decoder()
        .decode(b"Shirts,Sold on Monday,30,$17.99")
        .expect("decode");
}

#[test]
fn ir_is_populated() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/record.xsd")).expect("spec");
    assert_eq!(spec.root_element(), "Record");
    assert!(spec.program().nodes.len() >= 3);
}
