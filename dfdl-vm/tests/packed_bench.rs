//! Investigate packed.tdml slowness (ignored by default).
use dfdl_vm::schema::parse_schema;
use dfdl_vm::tdml::{effective_round_trip, parse_tdml, run_parser_test, RoundTrip};
use dfdl_vm::DfdlSpec;
use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, Instant};

const PACKED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/daffodil/daffodil-test/src/test/resources/org/apache/daffodil/section13/packed/packed.tdml"
);

#[test]
#[ignore = "diagnostic timing for packed.tdml"]
fn packed_tdml_timing_breakdown() {
    let tdml = fs::read_to_string(PACKED).expect("read packed");
    let suite = parse_tdml(&tdml).expect("parse tdml");
    let n = suite.tests.len();
    let unparse_n = suite.unparser_tests.len();

    let mut keys = BTreeSet::new();
    for t in &suite.tests {
        keys.insert((t.model.clone(), t.root.clone()));
    }

    let sample = suite
        .tests
        .iter()
        .find(|t| t.name == "hexCharset01")
        .unwrap();
    let xsd = suite.schemas.get(&sample.model).unwrap().xsd.clone();

    let t = Instant::now();
    let schema = parse_schema(&xsd).expect("parse_schema");
    let parse_schema_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let spec =
        DfdlSpec::from_schema_root_with_tunables(schema, Some(&sample.root), Default::default())
            .expect("compile");
    let compile_once_ms = t.elapsed().as_millis();

    let t = Instant::now();
    for _ in 0..50 {
        let schema = parse_schema(&xsd).expect("parse_schema");
        let _ = DfdlSpec::from_schema_root_with_tunables(
            schema,
            Some(&sample.root),
            Default::default(),
        )
        .expect("compile");
    }
    let compile_50x_ms = t.elapsed().as_millis();

    let doc = &sample.documents[0].data;
    let t = Instant::now();
    for _ in 0..5000 {
        let _ = spec.decoder().decode(doc).expect("decode");
    }
    let decode_5000x_ms = t.elapsed().as_millis();

    let mut slowest: Vec<(Duration, String)> = Vec::new();
    let t_all = Instant::now();
    for test in &suite.tests {
        let t = Instant::now();
        eprintln!(">> start {}", test.name);
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let _ = run_parser_test(&suite, test).expect("run");
        let elapsed = t.elapsed();
        eprintln!(">> done {} in {elapsed:?}", test.name);
        let _ = std::io::Write::flush(&mut std::io::stderr());
        slowest.push((elapsed, test.name.clone()));
        if elapsed > Duration::from_secs(2) {
            eprintln!("!! SLOW (>2s): {} {:?}", test.name, elapsed);
        }
    }
    let all_ms = t_all.elapsed().as_millis();

    slowest.sort_by_key(|a| std::cmp::Reverse(a.0));

    let mut two_pass = 0usize;
    for t in &suite.tests {
        if effective_round_trip(t.round_trip, suite.default_round_trip) == RoundTrip::TwoPass {
            two_pass += 1;
        }
    }

    eprintln!("=== packed.tdml slowness breakdown ===");
    eprintln!("parser tests: {n}, unparser tests: {unparse_n}");
    eprintln!("unique (model, root): {}", keys.len());
    eprintln!("default_round_trip: {:?}", suite.default_round_trip);
    eprintln!("tests with twoPass (incl default): ~{two_pass}");
    eprintln!("parse_schema once: {parse_schema_ms} ms");
    eprintln!("compile once: {compile_once_ms} ms");
    eprintln!(
        "50x parse+compile: {compile_50x_ms} ms (~{} ms each)",
        compile_50x_ms / 50
    );
    eprintln!(
        "5000x decode (cached spec): {decode_5000x_ms} ms (~{:.3} ms each)",
        decode_5000x_ms as f64 / 5000.0
    );
    eprintln!(
        "all {n} run_parser_test: {all_ms} ms (~{:.1} ms each)",
        all_ms as f64 / n as f64
    );
    eprintln!("top 10 slowest cases:");
    for (d, name) in slowest.into_iter().take(10) {
        eprintln!("  {d:>7?}  {name}");
    }
}

#[test]
#[ignore = "diagnostic: where DelimitedBCDIntSeq hangs"]
fn packed_delimited_bcd_int_seq_decode_only() {
    let tdml = fs::read_to_string(PACKED).expect("read");
    let suite = parse_tdml(&tdml).expect("parse");
    let test = suite
        .tests
        .iter()
        .find(|t| t.name == "DelimitedBCDIntSeq")
        .expect("case");
    let xsd = suite.schemas.get(&test.model).unwrap().xsd.clone();
    let schema = parse_schema(&xsd).expect("schema");
    let spec =
        DfdlSpec::from_schema_root_with_tunables(schema, Some(&test.root), Default::default())
            .expect("compile");
    let doc = &test.documents[0].data;
    let t = Instant::now();
    let decoded = spec.decoder().decode(doc).expect("decode");
    eprintln!("decode ok in {:?}, value: {:?}", t.elapsed(), decoded);
}
