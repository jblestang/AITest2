use dfdl_vm::{DfdlSpec, DfdlValue};

/// NMEA 0183 GPGGA example (UTC fix, lat/lon, altitude).
const GPGGA_SAMPLE: &[u8] =
    b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";

const GPRMC_SAMPLE: &[u8] =
    b"$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A\r\n";

const AIVDM_SAMPLE: &[u8] = b"!AIVDM,1,1,,A,15M67FC000G?l`nQ@`WplQ@T400,0*7F\r\n";

const GPGLL_SAMPLE: &[u8] = b"$GPGLL,4807.038,N,01131.000,E,123519,A,A*58\r\n";

const GPVTG_SAMPLE: &[u8] = b"$GPVTG,054.7,T,034.4,M,005.5,N,010.2,K,A*48\r\n";

#[test]
fn nmea_consecutive_empty_fields_before_checksum() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(GPGGA_SAMPLE).expect("decode");
    let standard = decoded.field("Standard").expect("Standard");
    let payload = standard.field("payload").expect("payload");
    match payload.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 14);
            assert_eq!(items[12], DfdlValue::String("".into()));
            assert_eq!(items[13], DfdlValue::String("".into()));
        }
        other => panic!("expected field array, got {other:?}"),
    }
}

#[test]
fn nmea_gpgga_full_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(GPGGA_SAMPLE).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, GPGGA_SAMPLE);
}

#[test]
fn nmea_gpgga_typed_decode() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_gpgga.xsd")).expect("spec");
    let decoded = spec.decode(GPGGA_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("utcTime"),
        Some(&DfdlValue::UnsignedInt(123519))
    );
    assert_eq!(body.field("fixQuality"), Some(&DfdlValue::UnsignedByte(1)));
    assert_eq!(body.field("numSatellites"), Some(&DfdlValue::UnsignedByte(8)));
    assert_eq!(body.field("hdop"), Some(&DfdlValue::Float(0.9)));
    assert_eq!(body.field("altitude"), Some(&DfdlValue::Float(545.4)));
    assert_eq!(
        body.field("geoidSeparation"),
        Some(&DfdlValue::Float(46.9))
    );
    assert_eq!(
        body.field("dgpsAge"),
        Some(&DfdlValue::String("".into()))
    );
    assert_eq!(
        body.field("dgpsStationId"),
        Some(&DfdlValue::String("".into()))
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
    assert_eq!(encoded, GPGGA_SAMPLE);
}

#[test]
fn nmea_gprmc_typed_decode_and_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_gprmc.xsd")).expect("spec");
    let decoded = spec.decode(GPRMC_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("utcTime"),
        Some(&DfdlValue::UnsignedInt(123519))
    );
    assert_eq!(body.field("status"), Some(&DfdlValue::String("A".into())));
    assert_eq!(body.field("speedKnots"), Some(&DfdlValue::Float(22.4)));
    assert_eq!(body.field("trackTrue"), Some(&DfdlValue::Float(84.4)));
    assert_eq!(body.field("date"), Some(&DfdlValue::UnsignedInt(230394)));
    assert_eq!(
        body.field("magneticVariation"),
        Some(&DfdlValue::Float(3.1))
    );
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, GPRMC_SAMPLE);
}

#[test]
fn nmea_aivdm_typed_decode_and_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_aivdm.xsd")).expect("spec");
    let decoded = spec.decode(AIVDM_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("totalSentences"),
        Some(&DfdlValue::UnsignedByte(1))
    );
    assert_eq!(
        body.field("sentenceNumber"),
        Some(&DfdlValue::UnsignedByte(1))
    );
    assert_eq!(
        body.field("sequentialId"),
        Some(&DfdlValue::String("".into()))
    );
    assert_eq!(body.field("channel"), Some(&DfdlValue::String("A".into())));
    assert_eq!(
        body.field("payload"),
        Some(&DfdlValue::String("15M67FC000G?l`nQ@`WplQ@T400".into()))
    );
    assert_eq!(body.field("fillBits"), Some(&DfdlValue::UnsignedByte(0)));
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, AIVDM_SAMPLE);
}

#[test]
fn nmea_gll_typed_decode_and_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_gll.xsd")).expect("spec");
    let decoded = spec.decode(GPGLL_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(
        body.field("latitude"),
        Some(&DfdlValue::String("4807.038".into()))
    );
    assert_eq!(
        body.field("utcTime"),
        Some(&DfdlValue::UnsignedInt(123519))
    );
    assert_eq!(body.field("mode"), Some(&DfdlValue::String("A".into())));
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, GPGLL_SAMPLE);
}

#[test]
fn nmea_vtg_typed_decode_and_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_vtg.xsd")).expect("spec");
    let decoded = spec.decode(GPVTG_SAMPLE).expect("decode");
    let body = decoded.field("body").expect("body");
    assert_eq!(body.field("trackTrue"), Some(&DfdlValue::Float(54.7)));
    assert_eq!(body.field("trackMagnetic"), Some(&DfdlValue::Float(34.4)));
    assert_eq!(body.field("speedKnots"), Some(&DfdlValue::Float(5.5)));
    assert_eq!(body.field("speedKmh"), Some(&DfdlValue::Float(10.2)));
    assert_eq!(body.field("mode"), Some(&DfdlValue::String("A".into())));
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, GPVTG_SAMPLE);
}

#[test]
fn nmea_sentence_generic_aivdm() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(AIVDM_SAMPLE).expect("decode");
    let encapsulated = decoded.field("Encapsulated").expect("Encapsulated");
    let payload = encapsulated.field("payload").expect("payload");
    match payload.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 6);
            assert_eq!(items[0], DfdlValue::String("1".into()));
            assert_eq!(items[2], DfdlValue::String("".into()));
            assert_eq!(items[3], DfdlValue::String("A".into()));
        }
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn nmea_empty_field_in_middle_of_payload() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let sentence = b"$GPAAA,one,,three*70\r\n";
    let decoded = spec.decode(sentence).expect("decode");
    let standard = decoded.field("Standard").expect("Standard");
    let payload = standard.field("payload").expect("payload");
    match payload.field("field").expect("fields") {
        DfdlValue::Array(items) => {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], DfdlValue::String("one".into()));
            assert_eq!(items[1], DfdlValue::String("".into()));
            assert_eq!(items[2], DfdlValue::String("three".into()));
        }
        other => panic!("expected array, got {other:?}"),
    }
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, sentence);
}

#[test]
fn nmea_sentence_gprmc_round_trip() {
    let spec = DfdlSpec::from_xsd(include_str!("fixtures/nmea_sentence.xsd")).expect("spec");
    let decoded = spec.decode(GPRMC_SAMPLE).expect("decode");
    let encoded = spec.encode(&decoded).expect("encode");
    assert_eq!(encoded, GPRMC_SAMPLE);
}
