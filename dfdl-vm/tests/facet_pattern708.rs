use dfdl_vm::schema::{match_length_pattern, parse_schema, SimpleBase, TypeDef};
use dfdl_vm::tdml::{parse_tdml, run_parser_test, TestOutcome};
use std::fs;

const FACETS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section05/facets/Facets.tdml"
);

#[test]
fn dfdl708_orig_stored_pattern_matches_comma() {
    let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/">
  <xs:element name="dfdl708_orig" dfdl:lengthKind="delimited">
    <xs:simpleType>
      <xs:restriction base="xs:string">
        <xs:pattern value="[.A-Za-z0-9!#$%&amp;'*+-/=?^_`\{\|\}~]+" />
      </xs:restriction>
    </xs:simpleType>
  </xs:element>
</xs:schema>"#;
    let doc = parse_schema(xsd).expect("parse");
    let el = dfdl_vm::schema::get_global_element(&doc, "dfdl708_orig").expect("element");
    let TypeDef::Simple { base, .. } = doc.resolve_type(&el.type_name).expect("type") else {
        panic!("simple type");
    };
    let SimpleBase::Restriction { patterns, .. } = base else {
        panic!("restriction");
    };
    let pat = patterns.first().expect("pattern");
    let doc_bytes = b"john,doe";
    assert_eq!(
        match_length_pattern(doc_bytes, pat),
        Some(doc_bytes.len()),
        "pattern `{pat}`"
    );
}

#[test]
fn pattern_regex_dfdl708_04_tdml() {
    let text = fs::read_to_string(FACETS).expect("read");
    let suite = parse_tdml(&text).expect("parse tdml");
    let t = suite
        .tests
        .iter()
        .find(|t| t.name == "patternRegexDFDL708_04")
        .expect("case");
    let r = run_parser_test(&suite, t).expect("run");
    assert_eq!(r.outcome, TestOutcome::Pass, "{:?}", r.outcome);
}
