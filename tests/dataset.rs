//! Assembling a model: merging datasets, finding the files, and the one thing the store
//! can hold that no other check would ever look at.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::cgmes3::{SCHEMA, attributes, classes};
use cim_rs::prelude::*;
use cim_rs::reader::read_into;

fn doc(profile_iri: &str, id: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:{id}">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.profile>{profile_iri}</md:Model.profile>
  </md:FullModel>
{body}
</rdf:RDF>"#
    )
}

const EQ_IRI: &str = "http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0";
const SSH_IRI: &str = "http://iec.ch/TC57/ns/CIM/SteadyStateHypothesis-EU/3.0";
const LINE: &str = "33333333-3333-4333-8333-333333333333";

fn eq_file() -> String {
    doc(
        EQ_IRI,
        "11111111-1111-4111-8111-111111111111",
        &format!(
            r#"  <cim:ACLineSegment rdf:ID="_{LINE}">
    <cim:IdentifiedObject.name>Line</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>1.5</cim:ACLineSegment.r>
  </cim:ACLineSegment>"#
        ),
    )
}

fn ssh_file() -> String {
    doc(
        SSH_IRI,
        "22222222-2222-4222-8222-222222222222",
        &format!(
            r##"  <cim:Equipment rdf:about="#_{LINE}">
    <cim:Equipment.inService>true</cim:Equipment.inService>
  </cim:Equipment>"##
        ),
    )
}

fn load(texts: &[&str]) -> Dataset {
    let mut ds = Dataset::new(SCHEMA);
    for (i, t) in texts.iter().enumerate() {
        read_into(
            &mut ds,
            t.as_bytes(),
            Some(&format!("file{i}.xml")),
            &ReadOptions::lenient(),
        )
        .expect("read");
    }
    ds
}

/// Merging two datasets has to give the same model as loading their files in sequence —
/// otherwise parsing a model set in parallel is not the same operation as parsing it
/// serially, and nobody could use it.
#[test]
fn merging_datasets_equals_loading_their_files_in_order() {
    let (eq, ssh) = (eq_file(), ssh_file());
    let sequential = load(&[&eq, &ssh]);

    let mut assembled = load(&[&eq]);
    assembled.merge(load(&[&ssh])).expect("same vintage");

    assert_eq!(assembled.len(), sequential.len());
    assert_eq!(assembled.headers().len(), sequential.headers().len());
    // The merged model carries both profiles' values on the one object, under the class
    // Equipment introduced rather than the base class SSH referred to it by.
    for ds in [&assembled, &sequential] {
        let o = ds.by_mrid(&Mrid::parse(LINE)).expect("the line");
        assert_eq!(o.class(), classes::ACLineSegment);
        assert_eq!(
            o.get(attributes::ac_line_segment::r).unwrap().as_f64(),
            Some(1.5)
        );
        assert_eq!(
            o.get(attributes::equipment::inService).unwrap().as_bool(),
            Some(true)
        );
    }
    // And the per-file index survives the move, so an export still puts each object back
    // in the file it came from.
    assert_eq!(
        assembled.objects_from(1).count(),
        sequential.objects_from(1).count(),
        "the second file's objects"
    );
    // Same content, so the same content identifier — which is the strongest statement
    // available that the two assemblies produced one model.
    assert_eq!(assembled.content_id(), sequential.content_id());
}

#[test]
fn merging_across_vintages_is_refused_rather_than_silently_wrong() {
    // Only meaningful with both vintages compiled in; `AttrId`s index one vintage's tables,
    // so combining models would file values under whatever the other vintage has there.
    #[cfg(feature = "cgmes2")]
    {
        let mut three = Dataset::new(SCHEMA);
        let two = Dataset::new(cim_rs::cgmes2::SCHEMA);
        let err = three.merge(two).expect_err("vintages must not mix");
        assert!(
            matches!(err, cim_rs::Error::SchemaMismatch { .. }),
            "{err:?}"
        );
    }
}

/// Every command that touches a model set needs the same directory walk, and the order has
/// to be fixed for a load to be reproducible.
#[test]
fn instance_files_finds_a_model_set_in_a_directory() {
    let root = require_corpus!(common::cgmes3_model("MicroGrid/MicroGid-BaseCase"));
    let files = cim_rs::instance_files(&root);
    assert!(files.len() > 4, "found {} files", files.len());
    assert!(files.windows(2).all(|w| w[0] <= w[1]), "not sorted");
    assert!(files.iter().all(|p| p.is_file()));

    let ds = Dataset::load_dir(SCHEMA, &root).expect("load");
    assert!(ds.len() > 100, "{} objects", ds.len());
    assert_eq!(ds.headers().len(), files.len());

    // A single file is returned as itself, so callers need no special case.
    assert_eq!(cim_rs::instance_files(&files[0]), vec![files[0].clone()]);
}

/// A value filed under an attribute the object's class does not have is examined by
/// nothing — the writer emits a class's own attributes and so does every other check — so
/// it would be dropped without a word on the next export. It is the one thing validation
/// has to ask of the store rather than of a document.
#[test]
fn a_value_its_class_cannot_carry_is_reported_rather_than_silently_dropped() {
    let mut ds = Dataset::new(SCHEMA);
    let mut o = Object::new(classes::Terminal, Mrid::parse(LINE));
    // `ACLineSegment.r` on a `Terminal`: the public setter cannot know better.
    o.set(attributes::ac_line_segment::r, Value::Float(1.5));
    ds.insert(o);

    let report = ds.validate();
    let finding = report
        .by_rule(cim_rs::Rule::UnknownAttribute)
        .find(|d| d.attribute == Some("ACLineSegment.r"))
        .expect("the stranded value is reported");
    assert_eq!(finding.severity, cim_rs::Severity::Error);
    assert_eq!(finding.class, Some("Terminal"));

    // And the writer really would drop it, which is what makes the finding worth having.
    let mut out = Vec::new();
    cim_rs::writer::write(&ds, &mut out, &Default::default()).expect("write");
    assert!(
        !String::from_utf8(out).unwrap().contains("ACLineSegment.r"),
        "the value is not written, which is exactly why it must be reported"
    );
}

/// Moving an object sideways in the hierarchy leaves the old class's own attributes with
/// nowhere to live. They are shed and reported rather than kept where nothing would see
/// them again.
#[test]
fn reclassifying_sideways_sheds_the_values_the_new_class_cannot_carry() {
    let mut ds = Dataset::new(SCHEMA);
    let mut o = Object::new(classes::LinearShuntCompensator, Mrid::parse(LINE));
    o.set(
        attributes::linear_shunt_compensator::bPerSection,
        Value::Float(0.000346),
    );
    o.set(attributes::shunt_compensator::nomU, Value::Float(380.0));
    let id = ds.insert(o);

    let shed = ds.reclassify(id, classes::NonlinearShuntCompensator);
    assert_eq!(shed.len(), 1, "only the linear-only attribute goes");
    assert_eq!(
        shed[0].attr,
        attributes::linear_shunt_compensator::bPerSection
    );

    let o = ds.get(id).expect("still there");
    assert_eq!(o.class(), classes::NonlinearShuntCompensator);
    // Everything the new class does have is untouched.
    assert_eq!(
        o.get(attributes::shunt_compensator::nomU).unwrap().as_f64(),
        Some(380.0)
    );
    assert_eq!(
        ds.validate()
            .by_rule(cim_rs::Rule::UnknownAttribute)
            .count(),
        0,
        "nothing is left stranded on the new class"
    );

    // Narrowing to a subclass sheds nothing: a subclass has every attribute its parent has.
    let mut ds = Dataset::new(SCHEMA);
    let mut o = Object::new(classes::Equipment, Mrid::parse(LINE));
    o.set(attributes::equipment::inService, Value::Boolean(true));
    let id = ds.insert(o);
    assert!(ds.reclassify(id, classes::ACLineSegment).is_empty());
}

/// `Dataset::open` reads the vintage out of the files rather than taking it as an argument.
#[test]
fn opening_a_model_set_needs_no_schema_argument() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let ds = Dataset::open(&dir).unwrap();
    assert_eq!(ds.schema().vintage, "cgmes3");
    assert!(ds.len() > 100, "{} objects", ds.len());

    // The same model, loaded the explicit way, is the same model.
    let explicit = Dataset::load_dir(SCHEMA, &dir).unwrap();
    assert_eq!(ds.content_id(), explicit.content_id());
}

/// Something that is not a CIM input is an error naming what this build does have, rather
/// than an empty dataset that looks like a successful load.
#[test]
fn opening_something_that_is_not_cim_says_so() {
    let not_cim = std::env::temp_dir().join(format!("cim-open-{}.xml", std::process::id()));
    std::fs::write(&not_cim, "<html><body>not a grid model</body></html>").unwrap();

    let err = Dataset::open(&not_cim).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("no CIM vintage recognised"), "{msg}");
    assert!(
        msg.contains("cgmes3"),
        "it should name what is compiled in: {msg}"
    );

    // An empty directory is the same answer, not a silent empty model.
    let empty = std::env::temp_dir().join(format!("cim-open-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    assert!(Dataset::open(&empty).is_err());

    let _ = std::fs::remove_file(&not_cim);
    let _ = std::fs::remove_dir_all(&empty);
}
