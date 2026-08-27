//! Object identification: `rdf:ID` versus `rdf:about` (IEC 61970-552).
//!
//! A file that *introduces* an object identifies it with `rdf:ID`; a file that adds to an
//! object defined elsewhere uses `rdf:about`. Which of the two applies is a property of
//! the **class within the profile**, not of the profile: the RDFS marks a class a profile
//! only refers to with `cims:stereotype Description`. A Topology file therefore writes
//! `TopologicalNode` with `rdf:ID` — Topology introduces it — and `Terminal` with
//! `rdf:about`, because Equipment did.
//!
//! Deciding this per file instead silently rewrote every `rdf:ID` in the published State
//! Variables and Topology files as `rdf:about`. The round-trip tests could not see it:
//! they compare the assembled model, and the model is the same either way. These tests
//! compare the *serialized form* against the published files.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::cgmes3::{SCHEMA, classes};
use cim_rs::prelude::*;
use cim_rs::writer::{IdStyle, id_style_for};

fn profile(keyword: &str) -> cim_rs::ProfileId {
    SCHEMA
        .profile_by_keyword(keyword)
        .unwrap_or_else(|| panic!("no {keyword} profile"))
}

#[test]
fn identity_style_follows_the_description_stereotype() {
    let sv = profile("SV").mask();
    let tp = profile("TP").mask();
    let ssh = profile("SSH").mask();
    let eq = profile("EQ").mask();
    let sc = profile("SC").mask();

    // State Variables introduces its own results.
    assert_eq!(
        id_style_for(SCHEMA, classes::SvPowerFlow, sv),
        IdStyle::RdfId
    );
    // …but only refers to the converters it annotates.
    assert_eq!(
        id_style_for(SCHEMA, classes::CsConverter, sv),
        IdStyle::RdfAbout
    );

    // Topology introduces topological nodes and refers to terminals.
    assert_eq!(
        id_style_for(SCHEMA, classes::TopologicalNode, tp),
        IdStyle::RdfId
    );
    assert_eq!(
        id_style_for(SCHEMA, classes::Terminal, tp),
        IdStyle::RdfAbout
    );

    // Steady State Hypothesis introduces nothing.
    assert_eq!(
        id_style_for(SCHEMA, classes::Terminal, ssh),
        IdStyle::RdfAbout
    );

    // ShortCircuit only annotates equipment — but the Equipment file that normally
    // carries both defines it, so the combined export uses rdf:ID.
    assert_eq!(
        id_style_for(SCHEMA, classes::ACLineSegment, sc),
        IdStyle::RdfAbout
    );
    assert_eq!(
        id_style_for(SCHEMA, classes::ACLineSegment, eq | sc),
        IdStyle::RdfId
    );

    // A single document holding everything is that model's definition.
    assert_eq!(id_style_for(SCHEMA, classes::Terminal, 0), IdStyle::RdfId);
}

/// Count `rdf:ID=` and `rdf:about=` object attributes in a document.
fn identity_counts(text: &str) -> (usize, usize) {
    (
        text.matches("rdf:ID=").count(),
        // The header's own `rdf:about` is not an object; exclude it.
        text.matches("rdf:about=").count() - text.matches("FullModel rdf:about=").count(),
    )
}

#[test]
fn re_export_identifies_objects_the_way_the_published_files_do() {
    for model in [
        "MiniGrid/MiniGrid-Merged",
        "SmallGrid/SmallGrid-Merged",
        "MicroGrid/MicroGid-BaseCase/MicroGrid-BaseCase-Merged",
    ] {
        let dir = require_corpus!(common::cgmes3_model(model));
        let files = common::xml_files(&dir);
        let mut ds = Dataset::new(SCHEMA);
        ds.load_files(&files, &ReadOptions::lenient()).unwrap();

        let out = std::env::temp_dir().join(format!(
            "cim-identity-{}-{}",
            std::process::id(),
            model.replace('/', "_")
        ));
        std::fs::create_dir_all(&out).unwrap();
        let saved = ds.save_as_loaded(&out).unwrap();

        let mut checked = 0;
        for original in &files {
            let name = original.file_name().unwrap();
            let Some(written) = saved.written.iter().find(|p| p.file_name() == Some(name)) else {
                continue;
            };
            let before = identity_counts(&std::fs::read_to_string(original).unwrap());
            let after = identity_counts(&std::fs::read_to_string(written).unwrap());
            assert_eq!(
                before,
                after,
                "{model}/{}: (rdf:ID, rdf:about) was {before:?}, re-exported as {after:?}",
                name.to_string_lossy()
            );
            checked += 1;
        }
        assert!(checked > 0, "{model}: nothing compared");
        std::fs::remove_dir_all(&out).ok();
    }
}

/// A reference written as an absolute IRI denotes the UUID in its fragment.
///
/// Producers write these when a reference crosses documents. Reading the whole IRI as
/// opaque text is the mistake it looks like it is not: the object is present in the model
/// under its UUID, so the reference dangles against something that is right there — and
/// the same object, referred to both ways, splits in two.
#[test]
fn a_reference_written_as_an_absolute_iri_resolves_to_the_object_it_names() {
    use cim_rs::reader::read_into;

    const LINE: &str = "33333333-3333-4333-8333-333333333333";
    let doc = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:ACLineSegment rdf:ID="_{LINE}">
    <cim:IdentifiedObject.name>Line</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_44444444-4444-4444-8444-444444444444">
    <cim:Terminal.ConductingEquipment rdf:resource="http://example.com/EQ.xml#_{LINE}"/>
  </cim:Terminal>
  <cim:Terminal rdf:ID="_55555555-5555-4555-8555-555555555555">
    <cim:Terminal.ConductingEquipment rdf:resource="#_{LINE}"/>
  </cim:Terminal>
</rdf:RDF>"##
    );

    let mut ds = Dataset::new(SCHEMA);
    read_into(
        &mut ds,
        doc.as_bytes(),
        Some("t.xml"),
        &ReadOptions::lenient(),
    )
    .expect("read");

    assert_eq!(ds.len(), 3, "the absolute IRI is not a fourth object");
    assert!(
        ds.dangling_references().is_empty(),
        "both terminals resolve: {:?}",
        ds.dangling_references()
    );
    // Both spellings name one object, so the two references join on it.
    let line = Mrid::parse(LINE);
    for t in [
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
    ] {
        let o = ds.by_mrid(&Mrid::parse(t)).expect("terminal");
        assert_eq!(
            o.get(cim_rs::cgmes3::attributes::terminal::ConductingEquipment)
                .and_then(|v| v.as_reference()),
            Some(&line),
            "{t}"
        );
    }
}
