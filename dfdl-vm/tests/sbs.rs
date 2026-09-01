use dfdl_vm::{DfdlSpec, DfdlValue, SchemaResolver};

/// MSG,3 airborne position (dump1090 / BaseStation format).
const MSG3_SAMPLE: &[u8] = b"MSG,3,496,211,4CA2D6,10057,2008/11/28,14:53:50.594,2008/11/28,14:58:51.153,,37000,,,51.45735,-1.02826,,,0,0,0,0\r\n";

/// MSG,4 velocity message.
const MSG4_SAMPLE: &[u8] =
    b"MSG,4,1,1,3C6545,1,2026/03/13,14:30:00.234,2026/03/13,14:30:00.567,,,450,125.3,,,,0,,,,\r\n";

/// AIR new-aircraft announcement (empty transmission type and trailing fields).
const AIR_SAMPLE: &[u8] =
    b"AIR,,1,1,3C6545,1,2026/03/13,14:29:55.000,2026/03/13,14:29:55.100,,,,,,,,,,,,\r\n";

fn sbs_spec(xsd: &str) -> DfdlSpec {
    let resolver = SchemaResolver::new()
        .with_bundled("sbs_types.xsd", include_str!("fixtures/sbs_types.xsd"));
    DfdlSpec::from_xsd_with_resolver(xsd, resolver).expect("spec")
}

#[test]
fn sbs_line_generic_decode_22_fields() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/sbs_line.xsd")).expect("spec");
    let decoded = spec.decode(MSG3_SAMPLE).expect("decode");
    match decoded.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 22);
            assert_eq!(items[0], DfdlValue::String("MSG".into()));
            assert_eq!(items[4], DfdlValue::String("4CA2D6".into()));
            assert_eq!(items[11], DfdlValue::String("37000".into()));
            assert_eq!(items[14], DfdlValue::String("51.45735".into()));
        }
        other => panic!("expected field array, got {other:?}"),
    }
}

#[test]
fn sbs_line_generic_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/sbs_line.xsd")).expect("spec");
    let decoded = spec.decode(MSG3_SAMPLE).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, MSG3_SAMPLE);
}

#[test]
fn sbs_line_generic_empty_fields_in_middle() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/sbs_line.xsd")).expect("spec");
    let decoded = spec.decode(AIR_SAMPLE).expect("decode");
    match decoded.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 22);
            assert_eq!(items[0], DfdlValue::String("AIR".into()));
            assert_eq!(items[1], DfdlValue::String("".into()));
            assert_eq!(items[10], DfdlValue::String("".into()));
        }
        other => panic!("expected field array, got {other:?}"),
    }
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, AIR_SAMPLE);
}

#[test]
fn sbs_message_typed_msg3_decode_and_round_trip() {
    let spec = sbs_spec(include_str!("fixtures/sbs_message.xsd"));
    let decoded = spec.decode(MSG3_SAMPLE).expect("decode");
    let msg = decoded.field("Msg").expect("Msg");
    assert_eq!(
        msg.field("hexIdent"),
        Some(&DfdlValue::String("4CA2D6".into()))
    );
    assert_eq!(
        msg.field("transmissionType"),
        Some(&DfdlValue::String("3".into()))
    );
    assert_eq!(msg.field("altitude"), Some(&DfdlValue::String("37000".into())));
    assert_eq!(
        msg.field("latitude"),
        Some(&DfdlValue::String("51.45735".into()))
    );
    assert_eq!(
        msg.field("longitude"),
        Some(&DfdlValue::String("-1.02826".into()))
    );
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, MSG3_SAMPLE);
}

#[test]
fn sbs_message_typed_air_decode_and_round_trip() {
    let spec = sbs_spec(include_str!("fixtures/sbs_message.xsd"));
    let decoded = spec.decode(AIR_SAMPLE).expect("decode");
    assert!(decoded.field("Air").is_some());
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, AIR_SAMPLE);
}

#[test]
fn sbs_msg_typed_decode_and_round_trip() {
    let spec = sbs_spec(include_str!("fixtures/sbs_msg.xsd"));
    let decoded = spec.decode(MSG3_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("transmissionType"),
        Some(&DfdlValue::UnsignedByte(3))
    );
    assert_eq!(
        body.field("sessionId"),
        Some(&DfdlValue::UnsignedInt(496))
    );
    assert_eq!(
        body.field("timeGenerated"),
        Some(&DfdlValue::String("14:53:50.594".into()))
    );
    assert_eq!(body.field("callsign"), Some(&DfdlValue::String("".into())));
    assert_eq!(body.field("isOnGround"), Some(&DfdlValue::String("0".into())));
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, MSG3_SAMPLE);
}

#[test]
fn sbs_msg_typed_msg4_round_trip() {
    let spec = sbs_spec(include_str!("fixtures/sbs_msg.xsd"));
    let decoded = spec.decode(MSG4_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("transmissionType"),
        Some(&DfdlValue::UnsignedByte(4))
    );
    assert_eq!(
        body.field("groundSpeed"),
        Some(&DfdlValue::String("450".into()))
    );
    assert_eq!(body.field("track"), Some(&DfdlValue::String("125.3".into())));
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, MSG4_SAMPLE);
}

#[test]
fn sbs_typed_rejects_invalid_hex_ident() {
    let spec = sbs_spec(include_str!("fixtures/sbs_msg.xsd"));
    let bad = b"MSG,3,1,1,ZZZZZZ,1,2026/03/13,14:30:00.123,2026/03/13,14:30:00.456,,36000,,,,47.45,19.26,,,,0,,0\r\n";
    assert!(spec.decode(bad).is_err());
}

#[test]
fn sbs_typed_rejects_invalid_transmission_type() {
    let spec = sbs_spec(include_str!("fixtures/sbs_msg.xsd"));
    let bad = b"MSG,9,1,1,3C6545,1,2026/03/13,14:30:00.123,2026/03/13,14:30:00.456,,36000,,,,47.45,19.26,,,,0,,0\r\n";
    assert!(spec.decode(bad).is_err());
}

#[test]
fn sbs_typed_rejects_invalid_date_format() {
    let spec = sbs_spec(include_str!("fixtures/sbs_msg.xsd"));
    let bad = b"MSG,3,1,1,3C6545,1,2026-03-13,14:30:00.123,2026/03/13,14:30:00.456,,36000,,,,47.45,19.26,,,,0,,0\r\n";
    assert!(spec.decode(bad).is_err());
}
