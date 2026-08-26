//! Difference models (IEC 61970-552 `dm:DifferenceModel`).
//!
//! A difference model expresses a change to a previously exchanged model as a set of
//! statements to retract followed by a set to assert. The corpus ships real ones, so the
//! tests below exercise the published form rather than a synthetic approximation.

mod common;

use cim::cgmes3::SCHEMA;
use cim::header::StatementValue;
use cim::prelude::*;

#[test]
fn reads_a_published_difference_model() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS-EQ_diff"
    ));
    let files = common::xml_files(&dir);
    let path = files.first().expect("a diff file");

    let file = std::fs::File::open(path).unwrap();
    let diff = cim::reader::read_difference(
        SCHEMA,
        std::io::BufReader::new(file),
        path.file_name().and_then(|n| n.to_str()),
    )
    .unwrap()
    .expect("document declares a dm:DifferenceModel");

    println!(
        "difference: {} reverse, {} forward statements, {} objects affected",
        diff.reverse.len(),
        diff.forward.len(),
        diff.affected_objects().len()
    );
    assert!(
        !diff.reverse.is_empty() || !diff.forward.is_empty(),
        "difference model carried no statements"
    );

    // The header must survive: it declares what the difference supersedes.
    assert_eq!(diff.header.kind, cim::ModelKind::Difference);
    assert!(
        !diff.header.supersedes.is_empty(),
        "a difference model names the model it supersedes"
    );
    assert!(!diff.header.profiles.is_empty());

    // Every statement must name a resolvable predicate.
    for s in diff.reverse.iter().chain(&diff.forward) {
        assert!(
            SCHEMA.find_attr(&s.predicate_ns, &s.predicate).is_some(),
            "unknown predicate {} in {}",
            s.predicate,
            s.predicate_ns
        );
    }
}

#[test]
fn applying_a_difference_changes_the_model_and_reverses_cleanly() {
    let base_dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let diff_dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS-EQ_diff"
    ));

    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(common::xml_files(&base_dir), &ReadOptions::lenient())
        .unwrap();

    let diff_path = common::xml_files(&diff_dir).remove(0);
    let diff = cim::reader::read_difference(
        SCHEMA,
        std::io::BufReader::new(std::fs::File::open(&diff_path).unwrap()),
        None,
    )
    .unwrap()
    .unwrap();

    // Record the values the difference is going to touch.
    let before: Vec<(Mrid, String, Option<String>)> = diff
        .forward
        .iter()
        .filter_map(|s| {
            let attr = SCHEMA.find_attr(&s.predicate_ns, &s.predicate)?;
            let obj = ds.by_mrid(&s.subject)?;
            Some((
                s.subject.clone(),
                s.predicate.clone(),
                obj.get(attr).map(|v| cim::value::display_value(SCHEMA, v)),
            ))
        })
        .collect();

    let report = ds.apply_difference(&diff);
    println!(
        "applied difference: {} diagnostics ({} errors)",
        report.len(),
        report.count(Severity::Error)
    );
    assert!(!report.has_errors(), "{report}");

    // Every forward statement must now be reflected in the model.
    let mut changed = 0;
    for s in &diff.forward {
        let Some(attr) = SCHEMA.find_attr(&s.predicate_ns, &s.predicate) else {
            continue;
        };
        let obj = ds
            .by_mrid(&s.subject)
            .unwrap_or_else(|| panic!("{} absent after applying difference", s.subject));
        let stored = obj
            .get(attr)
            .map(|v| cim::value::display_value(SCHEMA, v))
            .expect("forward statement must be asserted");
        // Compare in the same normalized form the library stores: references become
        // canonical mRIDs, enumeration IRIs become their qualified literal name, and
        // numbers are compared numerically since parsing normalizes their lexical form.
        let expected = match &s.value {
            StatementValue::Literal(t) => t.trim().to_owned(),
            StatementValue::Resource(iri) => match SCHEMA.attr(attr).kind {
                cim::schema::AttrKind::Enumeration(_) => iri
                    .rsplit_once('#')
                    .map(|(_, l)| l.to_owned())
                    .unwrap_or_else(|| iri.clone()),
                _ => Mrid::parse(iri).canonical(),
            },
        };
        let matches = stored == expected
            || match (stored.parse::<f64>(), expected.parse::<f64>()) {
                (Ok(a), Ok(b)) => (a - b).abs() < 1e-9,
                _ => false,
            };
        assert!(
            matches,
            "{}.{}: stored {stored:?} != {expected:?}",
            s.subject, s.predicate
        );
        changed += 1;
    }
    assert!(changed > 0, "difference asserted nothing");
    println!("{changed} forward statements verified in the model");

    // At least one value must genuinely differ from the base model, otherwise the test
    // would pass on a no-op difference.
    let differing = before
        .iter()
        .filter(|(mrid, pred, old)| {
            let attr = SCHEMA
                .find_attr("http://iec.ch/TC57/CIM100#", pred)
                .or_else(|| SCHEMA.find_attr("http://iec.ch/TC57/CIM100-European#", pred));
            match (attr, ds.by_mrid(mrid)) {
                (Some(a), Some(o)) => {
                    o.get(a).map(|v| cim::value::display_value(SCHEMA, v)) != *old
                }
                _ => false,
            }
        })
        .count();
    println!("{differing} values actually changed");
    assert!(differing > 0, "difference did not modify any value");
}

#[test]
fn reverse_statements_retract_values() {
    // A synthetic difference exercises retraction precisely, which the published diffs
    // do only incidentally.
    let doc = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:ACLineSegment rdf:ID="_22222222-2222-4222-8222-222222222222">
    <cim:IdentifiedObject.name>Line A</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>1.5</cim:ACLineSegment.r>
  </cim:ACLineSegment>
</rdf:RDF>"##;

    let diff_doc = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:dm="http://iec.ch/TC57/61970-552/DifferenceModel/1#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <dm:DifferenceModel rdf:about="urn:uuid:33333333-3333-4333-8333-333333333333">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <md:Model.Supersedes rdf:resource="urn:uuid:11111111-1111-4111-8111-111111111111"/>
    <dm:reverseDifferences rdf:parseType="Statements">
      <rdf:Description rdf:about="#_22222222-2222-4222-8222-222222222222">
        <cim:ACLineSegment.r>1.5</cim:ACLineSegment.r>
      </rdf:Description>
    </dm:reverseDifferences>
    <dm:forwardDifferences rdf:parseType="Statements">
      <rdf:Description rdf:about="#_22222222-2222-4222-8222-222222222222">
        <cim:ACLineSegment.r>2.75</cim:ACLineSegment.r>
      </rdf:Description>
    </dm:forwardDifferences>
  </dm:DifferenceModel>
</rdf:RDF>"##;

    let mut ds = Dataset::new(SCHEMA);
    cim::reader::read_into(
        &mut ds,
        doc.as_bytes(),
        Some("base.xml"),
        &ReadOptions::strict(),
    )
    .unwrap();

    let r_attr = SCHEMA
        .find_attr("http://iec.ch/TC57/CIM100#", "ACLineSegment.r")
        .unwrap();
    let subject = Mrid::parse("22222222-2222-4222-8222-222222222222");
    assert_eq!(
        ds.by_mrid(&subject).unwrap().get(r_attr).unwrap().as_f64(),
        Some(1.5)
    );

    let diff = cim::reader::read_difference(SCHEMA, diff_doc.as_bytes(), Some("diff.xml"))
        .unwrap()
        .unwrap();
    assert_eq!(diff.reverse.len(), 1);
    assert_eq!(diff.forward.len(), 1);

    let report = ds.apply_difference(&diff);
    assert!(!report.has_errors(), "{report}");

    // The old value was retracted and the new one asserted — exactly one remains.
    let obj = ds.by_mrid(&subject).unwrap();
    assert_eq!(obj.count(r_attr), 1, "retraction left a stale value behind");
    assert_eq!(obj.get(r_attr).unwrap().as_f64(), Some(2.75));
}
