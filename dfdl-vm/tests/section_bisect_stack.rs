//! Bisect which Daffodil section / TDML file blows the stack (run with `--ignored`).
use dfdl_vm::tdml::{parse_tdml, run_parser_test, run_unparser_test, TdmlSchema, TdmlSuite};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const TDML_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil"
);

fn enrich(suite: &mut TdmlSuite, tdml_path: &Path) {
    let Some(dir) = tdml_path.parent() else {
        return;
    };
    let dir_str = dir.to_string_lossy().into_owned();
    let mut models = HashSet::new();
    for t in &suite.tests {
        models.insert(t.model.clone());
    }
    for t in &suite.unparser_tests {
        models.insert(t.model.clone());
    }
    suite.resource_context =
        dfdl_vm::tdml::TdmlResourceContext::from_tdml_path(&tdml_path.to_string_lossy());
    for model in models {
        if suite.schemas.contains_key(&model) {
            continue;
        }
        if !(model.ends_with(".xsd") || model.ends_with(".dfdl.xsd")) {
            continue;
        }
        let path = dir.join(&model);
        let Ok(xsd) = fs::read_to_string(&path) else {
            continue;
        };
        suite.schemas.insert(
            model.clone(),
            TdmlSchema {
                name: model,
                xsd,
                compile_base_dir: Some(dir_str.clone()),
            },
        );
    }
}

fn collect_tdml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_tdml(&p, out);
        } else if p.extension().is_some_and(|x| x == "tdml") {
            out.push(p);
        }
    }
}

fn run_file(path: &Path) {
    let tdml = fs::read_to_string(path).expect("read tdml");
    let mut suite = parse_tdml(&tdml).expect("parse tdml");
    enrich(&mut suite, path);
    for test in &suite.tests {
        let _ = run_parser_test(&suite, test);
    }
    for test in &suite.unparser_tests {
        let _ = run_unparser_test(&suite, test);
    }
}

fn run_file_first_parser_test_only(path: &Path) {
    let tdml = fs::read_to_string(path).expect("read tdml");
    let mut suite = parse_tdml(&tdml).expect("parse tdml");
    enrich(&mut suite, path);
    if let Some(test) = suite.tests.first() {
        eprintln!("first test: {}", test.name);
        let _ = run_parser_test(&suite, test);
    }
}

#[test]
#[ignore = "diagnostic: find stack overflow source"]
fn bisect_sections_for_stack_overflow() {
    let root = PathBuf::from(TDML_ROOT);
    for sec in 0..=14 {
        let dir = root.join(format!("section{sec:02}"));
        if !dir.is_dir() {
            continue;
        }
        eprintln!("--- section{sec:02} ---");
        let mut files = Vec::new();
        collect_tdml(&dir, &mut files);
        files.sort();
        let n = files.len();
        for path in &files {
            eprintln!("  file {}", path.display());
            run_file(path);
        }
        eprintln!("  section{sec:02} OK ({n} files)");
    }
}

#[test]
#[ignore]
fn bisect_section06_only() {
    let dir = PathBuf::from(TDML_ROOT).join("section06");
    let mut files = Vec::new();
    collect_tdml(&dir, &mut files);
    files.sort();
    for path in files {
        eprintln!("file {}", path.display());
        run_file(&path);
    }
}

#[test]
#[ignore]
fn compile_prop_scoping_embedded_schema_only() {
    use dfdl_vm::ir::compile_named_with_tunables;
    use dfdl_vm::length_validate::DaffodilTunables;
    use dfdl_vm::schema::parse_schema;
    let xsd = r###"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:tns="http://example.com"
  targetNamespace="http://example.com" elementFormDefault="qualified">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:format ref="tns:GeneralFormat" representation="text"
    occursCountKind="parsed" lengthUnits="bytes" encoding="US-ASCII"
    initiator="" terminator="" separator="" ignoreCase="no" />
  <xs:element name="e1" type="xs:string" dfdl:lengthKind="explicit" dfdl:length="{ 1 }" />
  <xs:element name="e2" dfdl:lengthKind="implicit">
    <xs:complexType>
      <xs:sequence dfdl:separator="," dfdl:separatorPosition="infix">
        <xs:element ref="tns:e1" />
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"###;
    eprintln!("parse...");
    let schema = parse_schema(xsd).expect("parse");
    eprintln!("compile e2...");
    let _ = compile_named_with_tunables(&schema, Some("e2"), DaffodilTunables::default())
        .expect("compile");
    eprintln!("ok");
}

#[test]
#[ignore]
fn compile_prop_scoping_circular_formats() {
    use dfdl_vm::ir::compile_named_with_tunables;
    use dfdl_vm::length_validate::DaffodilTunables;
    use dfdl_vm::schema::parse_schema;
    let xsd = r###"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
  xmlns:dfdl="http://www.ogf.org/dfdl/dfdl-1.0/" xmlns:tns="http://example.com"
  targetNamespace="http://example.com" elementFormDefault="qualified">
  <xs:include schemaLocation="/org/apache/daffodil/xsd/DFDLGeneralFormat.dfdl.xsd"/>
  <dfdl:defineFormat name="def">
    <dfdl:format ref="tns:def1" encoding="utf-8"
      lengthKind="explicit" lengthUnits="characters" length="5"
      textNumberRep="zoned" />
  </dfdl:defineFormat>
  <dfdl:defineFormat name="def2">
    <dfdl:format ref="tns:def3" lengthKind="explicit"
      lengthUnits="characters" length="4" representation="text"
      textNumberRep="standard" />
  </dfdl:defineFormat>
  <dfdl:defineFormat name="def3">
    <dfdl:format ref="tns:GeneralFormat" representation="binary" />
  </dfdl:defineFormat>
  <dfdl:defineFormat name="def1">
    <dfdl:format ref="tns:def2" />
  </dfdl:defineFormat>
  <dfdl:format ref="tns:GeneralFormat" lengthKind="explicit"
    lengthUnits="characters" length="3" />
  <xs:element name="easy" type="xs:int" dfdl:textNumberRep="standard" dfdl:textNumberPattern="00000">
    <xs:annotation>
      <xs:appinfo source="http://www.ogf.org/dfdl/">
        <dfdl:element ref="tns:def"/>
      </xs:appinfo>
    </xs:annotation>
  </xs:element>
</xs:schema>"###;
    eprintln!("parse...");
    let schema = parse_schema(xsd).expect("parse");
    eprintln!("compile easy...");
    let _ = compile_named_with_tunables(&schema, Some("easy"), DaffodilTunables::default())
        .expect("compile");
    eprintln!("ok");
}

#[test]
#[ignore]
fn bisect_single_tdml_property_scoping_01() {
    let path = PathBuf::from(TDML_ROOT).join("section08/property_scoping/PropertyScoping_01.tdml");
    eprintln!("{}", path.display());
    run_file(&path);
}

#[test]
#[ignore]
fn bisect_property_scoping_01_each_test() {
    let path = PathBuf::from(TDML_ROOT).join("section08/property_scoping/PropertyScoping_01.tdml");
    let tdml = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&tdml).expect("parse");
    enrich(&mut suite, &path);
    for test in &suite.tests {
        eprintln!("parser test {}", test.name);
        let _ = run_parser_test(&suite, test);
    }
    for test in &suite.unparser_tests {
        eprintln!("unparser test {}", test.name);
        let _ = run_unparser_test(&suite, test);
    }
}

#[test]
#[ignore]
fn bisect_property_scoping_01_first_test_only() {
    let path = PathBuf::from(TDML_ROOT).join("section08/property_scoping/PropertyScoping_01.tdml");
    run_file_first_parser_test_only(&path);
}

#[test]
#[ignore]
fn bisect_property_scoping_01_parse_tdml_only() {
    let path = PathBuf::from(TDML_ROOT).join("section08/property_scoping/PropertyScoping_01.tdml");
    let tdml = fs::read_to_string(&path).expect("read");
    let _ = parse_tdml(&tdml).expect("parse tdml");
}

#[test]
#[ignore]
fn parse_cycle_base_schema_only() {
    use dfdl_vm::schema::{parse_schema_with_options, ParseOptions};
    let dir = PathBuf::from(TDML_ROOT).join("section06/namespaces");
    let path = dir.join("cycle_base.dfdl.xsd");
    let xsd = fs::read_to_string(&path).expect("read");
    let opts = ParseOptions {
        base_dir: Some(dir.to_string_lossy().into_owned()),
        schema_label: None,
    };
    eprintln!("parse cycle_base...");
    let doc = parse_schema_with_options(&xsd, &opts).expect("parse");
    eprintln!("ok global_elements={}", doc.global_elements.len());
    for (k, v) in &doc.global_elements {
        eprintln!("  ge {k} -> type {}", v.type_name.as_str());
    }
    for (k, _) in &doc.types {
        eprintln!("  type {}", k.as_str());
    }
}

#[test]
#[ignore]
fn compile_and_decode_multifile_cyclical() {
    use dfdl_vm::length_validate::DaffodilTunables;
    use dfdl_vm::schema::{parse_schema_with_options, ParseOptions};
    use dfdl_vm::DfdlSpec;
    let dir = PathBuf::from(TDML_ROOT).join("section06/namespaces");
    let path = dir.join("cycle_base.dfdl.xsd");
    let xsd = fs::read_to_string(&path).expect("read");
    let opts = ParseOptions {
        base_dir: Some(dir.to_string_lossy().into_owned()),
        schema_label: None,
    };
    eprintln!("parse...");
    let doc = parse_schema_with_options(&xsd, &opts).expect("parse");
    eprintln!("compile elem...");
    let spec =
        DfdlSpec::from_schema_root_with_tunables(doc, Some("elem"), DaffodilTunables::default())
            .expect("compile");
    eprintln!("decode...");
    let _ = spec.decode(b"12*34").expect("decode");
    eprintln!("ok");
}

#[test]
#[ignore]
fn bisect_namespaces_each_test() {
    let path = PathBuf::from(TDML_ROOT).join("section06/namespaces/namespaces.tdml");
    let tdml = fs::read_to_string(&path).expect("read");
    let mut suite = parse_tdml(&tdml).expect("parse");
    enrich(&mut suite, &path);
    for test in &suite.tests {
        eprintln!("parser test {}", test.name);
        let _ = run_parser_test(&suite, test);
    }
}

#[test]
#[ignore]
fn bisect_single_tdml_namespaces() {
    let path = PathBuf::from(TDML_ROOT).join("section06/namespaces/namespaces.tdml");
    eprintln!("{}", path.display());
    run_file(&path);
}

#[test]
#[ignore]
fn bisect_section08_only() {
    let dir = PathBuf::from(TDML_ROOT).join("section08");
    let mut files = Vec::new();
    collect_tdml(&dir, &mut files);
    files.sort();
    for path in files {
        eprintln!("file {}", path.display());
        run_file(&path);
    }
}
