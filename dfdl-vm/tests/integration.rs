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
        .encode(&DfdlValue::Sequence(fields))
        .expect("encode");
    assert_eq!(encoded, vec![0x00, 0x00, 0x00, 0x2A, 0x03]);
}

#[test]
fn text_message_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/text_message.xsd")).expect("spec");
    let input = b"TAG123;";
    let decoded = spec.decode(input).expect("decode");
    assert_eq!(decoded.field("tag"), Some(&DfdlValue::String("TAG".into())));
    assert_eq!(decoded.field("value"), Some(&DfdlValue::Int(123)));

    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, input);
}

#[test]
fn choice_initiator_decode() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/choice_initiator.xsd")).expect("spec");
    let decoded = spec.decode(b"1=abc").expect("decode branch A");
    assert_eq!(decoded.field("A"), Some(&DfdlValue::String("abc".into())));

    let decoded_b = spec.decode(b"2=xyz").expect("decode branch B");
    assert_eq!(decoded_b.field("B"), Some(&DfdlValue::String("xyz".into())));
}

#[test]
fn ir_is_populated() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/record.xsd")).expect("spec");
    assert_eq!(spec.root_element(), "Record");
    assert!(spec.program().nodes.len() >= 3);
}
