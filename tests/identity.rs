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
//!
//! The same reasoning reaches one level further down, to what an object *says*. Counting
//! identity styles cannot see a value being rewritten, and neither can the model-level
//! round-trip, which compares parsed numbers — `2.62637E-05` and `0.0000262637` are the
//! same `f64` on both sides of that comparison, so the assertion holds while the document
//! changes. [`value_census`](common::value_census) closes that, and it is what caught the
//! published corpus being re-exported with 8,623 of its numbers reformatted.

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

/// Re-export reproduces the *text* of every value, not merely its number.
///
/// Deliberately whole-model rather than per-file for the numbers themselves: a value
/// legitimately moves between the files of a set. What must not change is what the model
/// set says.
///
/// Model sets that contradict themselves are excluded by name — not by count, and not by
/// tolerating a threshold. `MicroGrid-Type3` holds 24 hourly snapshots per directory, so
/// loading one merges 24 different values for the same single-valued attributes and 23 of
/// them are discarded by design ([`Dataset::merge_conflicts`]); a document that was never
/// assembled cannot be reproduced. Asking the dataset whether it contradicts itself is the
/// exact predicate, where "skip the big ones" would be a guess that quietly grows.
#[test]
fn re_export_reproduces_the_text_of_every_value() {
    let root = require_corpus!(common::cgmes3_models());
    let mut checked_models = 0usize;
    let mut checked_values = 0usize;
    let mut skipped = Vec::new();

    for model in [
        "MiniGrid/MiniGrid-Merged",
        "SmallGrid/SmallGrid-Merged",
        "MicroGrid/MicroGrid-Type1/MicroGrid-Type1-Merged",
        "FullGrid/FullGrid-Merged",
        "MicroGrid/MicroGrid-Type3/CGMs",
        "MicroGrid/MicroGrid-Type3/IGMs",
    ] {
        let dir = root.join(model);
        if !dir.is_dir() {
            continue;
        }
        let ds = Dataset::load_dir(SCHEMA, &dir).unwrap();
        if !ds.merge_conflicts().is_empty() {
            skipped.push(model);
            continue;
        }

        let out = std::env::temp_dir().join(format!(
            "cim-values-{}-{}",
            std::process::id(),
            model.replace('/', "_")
        ));
        std::fs::create_dir_all(&out).unwrap();
        ds.save_as_loaded(&out).unwrap();

        let mut before = std::collections::BTreeMap::new();
        let mut after = std::collections::BTreeMap::new();
        for f in common::xml_files(&dir) {
            for (k, n) in common::value_census(&std::fs::read_to_string(&f).unwrap()) {
                *before.entry(k).or_insert(0) += n;
            }
        }
        for f in common::xml_files(&out) {
            for (k, n) in common::value_census(&std::fs::read_to_string(&f).unwrap()) {
                *after.entry(k).or_insert(0) += n;
            }
        }
        std::fs::remove_dir_all(&out).ok();

        let diff = common::value_census_diff(&before, &after, 8);
        assert!(
            diff.is_empty(),
            "{model}: {} value texts changed on re-export:\n{}",
            diff.len(),
            diff.join("\n")
        );
        checked_values += before.values().sum::<usize>();
        checked_models += 1;
    }

    assert!(
        checked_models >= 4,
        "only {checked_models} model sets compared"
    );
    assert!(
        checked_values > 100_000,
        "only {checked_values} values compared"
    );
    // Pinned, not tolerated: the one model set that legitimately cannot be reproduced is
    // named, so a second one appearing fails here rather than being absorbed.
    assert_eq!(
        skipped,
        [
            "MicroGrid/MicroGrid-Type3/CGMs",
            "MicroGrid/MicroGrid-Type3/IGMs"
        ],
        "the set of self-contradicting model sets changed"
    );
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
