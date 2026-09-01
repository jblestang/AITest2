use dfdl_vm::{DfdlSpec, DfdlValue};

/// NMEA 0183 GPGGA example (UTC fix, lat/lon, altitude).
const GPGGA_SAMPLE: &[u8] =
    b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";

#[test]
fn nmea_payload_element_has_terminator_in_ir() {
    use dfdl_vm::ir::IrNode;
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let payload = spec
        .program()
        .nodes
        .iter()
        .find_map(|n| {
            if let IrNode::Element { name, props, .. } = n {
                if spec.program().strings.get(*name).ok() == Some("payload") {
                    return Some(props.terminator);
                }
            }
            None
        })
        .expect("payload node");
    assert!(payload.is_some(), "payload element should have * terminator in IR");
    let term = spec
        .program()
        .strings
        .get(payload.unwrap())
        .expect("terminator string");
    assert_eq!(term, "*");

    let root = spec.program().node(spec.program().root).expect("root node");
    if let IrNode::Element { props, child, .. } = root {
        assert!(props.terminator.is_some(), "NmeaSentence root should have CRLF terminator");
        assert!(child.is_some(), "root should wrap inner content");
    } else {
        panic!("expected root element wrapper, got {root:?}");
    }
}

#[test]
fn nmea_gpgga_typed_decode() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_gpgga.xsd")).expect("spec");
    let decoded = spec.decode(GPGGA_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("utcTime"),
        Some(&DfdlValue::String("123519".into()))
    );
    assert_eq!(
        body.field("latitude"),
        Some(&DfdlValue::String("4807.038".into()))
    );
    assert_eq!(
        body.field("latHemisphere"),
        Some(&DfdlValue::String("N".into()))
    );
    assert_eq!(
        body.field("longitude"),
        Some(&DfdlValue::String("01131.000".into()))
    );
    assert_eq!(
        body.field("lonHemisphere"),
        Some(&DfdlValue::String("E".into()))
    );
    assert_eq!(
        decoded.field("checksum"),
        Some(&DfdlValue::String("47".into()))
    );
}

#[test]
fn nmea_gpgga_typed_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_gpgga.xsd")).expect("spec");
    let decoded = spec.decode(GPGGA_SAMPLE).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    // Trailing optional empty DGPS fields may collapse on decode.
    assert!(encoded.starts_with(b"$GPGGA,123519,"));
    assert!(encoded.ends_with(b"*47\r\n"));
}

#[test]
fn nmea_sentence_generic_gpgga() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(GPGGA_SAMPLE).expect("decode");
    let standard = decoded.field("Standard").expect("Standard branch");
    assert_eq!(
        standard.field("address"),
        Some(&DfdlValue::String("GPGGA".into()))
    );
    assert_eq!(
        standard.field("checksum"),
        Some(&DfdlValue::String("47".into()))
    );
    let payload = standard.field("payload").expect("payload");
    let fields = payload.field("field").expect("fields");
    match fields {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 13);
            assert_eq!(items[0], DfdlValue::String("123519".into()));
            assert_eq!(items[10], DfdlValue::String("46.9".into()));
            assert_eq!(items[11], DfdlValue::String("M".into()));
            assert_eq!(items[12], DfdlValue::String("".into()));
        }
        other => panic!("expected field array, got {other:?}"),
    }
}

#[test]
fn nmea_sentence_generic_round_trip() {
    let input = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n";
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(input).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, input);
}

#[test]
fn nmea_sentence_encapsulated_aivdm() {
    let input = b"!AIVDM,1,1,,A,15M67FC000G?l`nQ@`WplQ@T400,0*7F\r\n";
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(input).expect("decode");
    let encapsulated = decoded.field("Encapsulated").expect("Encapsulated branch");
    assert_eq!(
        encapsulated.field("address"),
        Some(&DfdlValue::String("AIVDM".into()))
    );
    assert_eq!(
        encapsulated.field("checksum"),
        Some(&DfdlValue::String("7F".into()))
    );
    let payload = encapsulated.field("payload").expect("payload");
    match payload.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 5);
            assert_eq!(items[0], DfdlValue::String("1".into()));
            assert_eq!(items[1], DfdlValue::String("1".into()));
            assert_eq!(items[2], DfdlValue::String("A".into()));
        }
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn nmea_sentence_gprmc() {
    let input = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n";
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(input).expect("decode");
    let standard = decoded.field("Standard").expect("Standard");
    assert_eq!(
        standard.field("address"),
        Some(&DfdlValue::String("GPRMC".into()))
    );
    let payload = standard.field("payload").expect("payload");
    match payload.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 11);
            assert_eq!(items[0], DfdlValue::String("123519".into()));
            assert_eq!(items[1], DfdlValue::String("A".into()));
            assert_eq!(items[10], DfdlValue::String("W".into()));
        }
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn nmea_sentence_gprmc_round_trip() {
    let input = b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n";
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(input).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, input);
}
