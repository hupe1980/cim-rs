//! Round-trip fidelity: read → write → read must preserve the model exactly.
//!
//! The guarantee tested here is *semantic*, not byte-level: a CGMES model is a set of
//! profile files, and re-exporting it legitimately redistributes objects across those
//! files. What must not change is the assembled model — the objects, their classes and
//! every attribute value.

mod common;

use std::collections::BTreeMap;

use cim::cgmes3::SCHEMA;
use cim::prelude::*;
use cim::validate;

/// A comparable, order-independent projection of a dataset.
///
/// Objects are keyed by mRID and attributes by name, with values rendered to text, so
/// that differences report as readable strings rather than opaque ids.
type Projection = BTreeMap<String, (String, BTreeMap<String, Vec<String>>)>;

fn project(ds: &Dataset) -> Projection {
    let schema = ds.schema();
    let mut out = Projection::new();
    for (_, obj) in ds.iter() {
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (attr, value) in obj.values() {
            attrs
                .entry(schema.attr(attr).name.to_owned())
                .or_default()
                .push(cim::value::display_value(schema, value));
        }
        for v in attrs.values_mut() {
            v.sort();
        }
        out.insert(
            obj.mrid().canonical(),
            (schema.class(obj.class()).name.to_owned(), attrs),
        );
    }
    out
}

/// Report the first few differences between two projections.
fn diff(a: &Projection, b: &Projection) -> Vec<String> {
    let mut out = Vec::new();
    for (mrid, (class, attrs)) in a {
        match b.get(mrid) {
            None => out.push(format!("missing after round-trip: {class} {mrid}")),
            Some((class_b, attrs_b)) => {
                if class != class_b {
                    out.push(format!("{mrid}: class {class} -> {class_b}"));
                }
                for (name, values) in attrs {
                    match attrs_b.get(name) {
                        None => out.push(format!("{mrid}: lost attribute {name} = {values:?}")),
                        Some(vb) if vb != values => {
                            out.push(format!("{mrid}: {name} {values:?} -> {vb:?}"))
                        }
                        _ => {}
                    }
                }
                for name in attrs_b.keys() {
                    if !attrs.contains_key(name) {
                        out.push(format!("{mrid}: gained attribute {name}"));
                    }
                }
            }
        }
    }
    for mrid in b.keys() {
        if !a.contains_key(mrid) {
            out.push(format!("appeared after round-trip: {mrid}"));
        }
    }
    out
}

/// Export every populated profile of `ds` and read the result back.
fn round_trip(ds: &Dataset) -> Dataset {
    let mut files: Vec<(cim::ProfileId, Vec<u8>)> = Vec::new();
    for cov in validate::profile_coverage(ds) {
        if cov.objects == 0 {
            continue;
        }
        let mut buf = Vec::new();
        let options = WriteOptions {
            id_style: cim::writer::conventional_id_style(ds.schema(), cov.profile),
            ..Default::default()
        };
        cim::writer::write_profile(ds, cov.profile, &mut buf, None, &options).unwrap();
        files.push((cov.profile, buf));
    }
    assert!(!files.is_empty(), "nothing was exported");

    let mut back = Dataset::new(ds.schema());
    for (profile, buf) in &files {
        let name = format!("{}.xml", ds.schema().profile(*profile).keyword);
        let outcome = cim::reader::read_into(
            &mut back,
            buf.as_slice(),
            Some(&name),
            &ReadOptions::strict(),
        )
        .unwrap();
        assert!(
            !outcome.report.has_errors(),
            "re-reading {name} produced errors: {}",
            outcome.report
        );
    }
    back
}

fn check_model(rel: &str) {
    let dir = match common::cgmes3_model(rel) {
        Some(d) => d,
        None => {
            eprintln!("skipping {rel}: corpus not present");
            return;
        }
    };
    let files = common::xml_files(&dir);
    let mut original = Dataset::new(SCHEMA);
    original
        .load_files(&files, &ReadOptions::lenient())
        .unwrap();
    assert!(!original.is_empty(), "{rel} loaded nothing");

    let back = round_trip(&original);

    let a = project(&original);
    let b = project(&back);
    let differences = diff(&a, &b);
    if !differences.is_empty() {
        for d in differences.iter().take(20) {
            eprintln!("  {d}");
        }
        panic!(
            "{rel}: {} differences after round-trip ({} objects before, {} after)",
            differences.len(),
            original.len(),
            back.len()
        );
    }
    println!("{rel}: {} objects round-tripped exactly", original.len());
}

#[test]
fn microgrid_base_case_round_trips() {
    check_model("MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS");
}

#[test]
fn microgrid_assembled_round_trips() {
    check_model("MicroGrid/MicroGid-BaseCase/MicroGrid-BaseCase-Merged");
}

#[test]
fn minigrid_round_trips() {
    check_model("MiniGrid/MiniGrid-Merged");
}

#[test]
fn smallgrid_round_trips() {
    check_model("SmallGrid/SmallGrid-Merged");
}

#[test]
fn fullgrid_round_trips() {
    check_model("FullGrid/FullGrid-Merged");
}

#[test]
fn writing_is_deterministic() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let files = common::xml_files(&dir);
    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(&files, &ReadOptions::lenient()).unwrap();

    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let render = || {
        let mut buf = Vec::new();
        cim::writer::write_profile(&ds, eq, &mut buf, None, &WriteOptions::default()).unwrap();
        buf
    };
    assert_eq!(
        render(),
        render(),
        "two writes of one model must be identical"
    );
}

#[test]
fn output_is_valid_xml_and_reparses_strictly() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let files = common::xml_files(&dir);
    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(&files, &ReadOptions::lenient()).unwrap();

    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let header = cim::ModelHeader {
        kind: cim::ModelKind::Full,
        id: Some(Mrid::parse("87da6373-3b6c-47a2-9493-1918a8d9df61")),
        created: Some("2021-03-25T23:16:27Z".into()),
        scenario_time: Some("2021-03-25T15:30:00Z".into()),
        version: Some("001".into()),
        modeling_authority_set: Some("http://tennet.nl/CGMES".into()),
        profiles: vec![SCHEMA.profile(eq).version_iri.to_owned()],
        ..Default::default()
    };
    let mut buf = Vec::new();
    cim::writer::write_profile(&ds, eq, &mut buf, Some(header), &WriteOptions::default()).unwrap();

    let text = String::from_utf8(buf.clone()).expect("output must be UTF-8");
    assert!(text.starts_with("<?xml"), "missing XML declaration");
    assert!(text.contains("md:FullModel"), "missing header element");
    assert!(
        text.contains("Model.scenarioTime"),
        "header fields not written"
    );

    // Strict re-read proves every element we wrote is one the schema recognises.
    let mut back = Dataset::new(SCHEMA);
    let outcome = cim::reader::read_into(
        &mut back,
        buf.as_slice(),
        Some("EQ.xml"),
        &ReadOptions::strict(),
    )
    .unwrap();
    assert!(!outcome.report.has_errors(), "{}", outcome.report);
    let h = outcome.header.expect("header must round-trip");
    assert_eq!(h.scenario_time.as_deref(), Some("2021-03-25T15:30:00Z"));
    assert_eq!(
        h.modeling_authority_set.as_deref(),
        Some("http://tennet.nl/CGMES")
    );
    assert_eq!(h.profiles.len(), 1);
}
