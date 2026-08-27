//! Difference models (IEC 61970-552 `dm:DifferenceModel`).
//!
//! A difference model expresses a change to a previously exchanged model as a set of
//! statements to retract followed by a set to assert. The corpus ships real ones, so the
//! tests below exercise the published form rather than a synthetic approximation.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::cgmes3::SCHEMA;
use cim_rs::diff::DiffOptions;
use cim_rs::header::StatementValue;
use cim_rs::prelude::*;

/// Every value of a model, in a form two datasets can be compared by.
fn projection(ds: &Dataset) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out = std::collections::BTreeMap::new();
    for (_, o) in ds.iter() {
        let mut values: Vec<String> = o
            .values()
            .map(|(a, v)| {
                format!(
                    "{}={}",
                    SCHEMA.attr(a).name,
                    cim_rs::value::display_value(SCHEMA, v)
                )
            })
            .collect();
        values.sort();
        out.insert(
            format!("{} {}", SCHEMA.class(o.class()).name, o.mrid()),
            values,
        );
    }
    out
}

#[test]
fn reads_a_published_difference_model() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS-EQ_diff"
    ));
    let files = common::xml_files(&dir);
    let path = files.first().expect("a diff file");

    let file = std::fs::File::open(path).unwrap();
    let diff = cim_rs::reader::read_difference(
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
    assert_eq!(diff.header.kind, cim_rs::ModelKind::Difference);
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
    let diff = cim_rs::reader::read_difference(
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
                obj.get(attr)
                    .map(|v| cim_rs::value::display_value(SCHEMA, v)),
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
            .map(|v| cim_rs::value::display_value(SCHEMA, v))
            .expect("forward statement must be asserted");
        // Compare in the same normalized form the library stores: references become
        // canonical mRIDs, enumeration IRIs become their qualified literal name, and
        // numbers are compared numerically since parsing normalizes their lexical form.
        let expected = match &s.value {
            StatementValue::Literal(t) => t.trim().to_owned(),
            StatementValue::Resource(iri) => match SCHEMA.attr(attr).kind {
                cim_rs::schema::AttrKind::Enumeration(_) => iri
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
                    o.get(a).map(|v| cim_rs::value::display_value(SCHEMA, v)) != *old
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
    cim_rs::reader::read_into(
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

    let diff = cim_rs::reader::read_difference(SCHEMA, diff_doc.as_bytes(), Some("diff.xml"))
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

// ---------------------------------------------------------------------------
// Computing a difference
// ---------------------------------------------------------------------------

/// The property that makes a computed change set worth anything: applying it works.
///
/// `base.difference_to(&target)` is only correct if feeding the result back through
/// `apply_difference` turns the base into the target — every value present, none left over,
/// classes included. Anything less and the change set is a plausible-looking document that
/// silently loses data, which is the failure mode a hand-checked example would not catch.
#[test]
fn a_computed_difference_turns_the_base_into_the_target() {
    let base_dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let diff_dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS-EQ_diff"
    ));

    // Build a genuinely different target by applying a published change set to the base.
    let mut base = Dataset::new(SCHEMA);
    base.load_files(common::xml_files(&base_dir), &ReadOptions::lenient())
        .unwrap();
    let mut target = Dataset::new(SCHEMA);
    target
        .load_files(common::xml_files(&base_dir), &ReadOptions::lenient())
        .unwrap();
    let published = cim_rs::reader::read_difference(
        SCHEMA,
        std::io::BufReader::new(std::fs::File::open(&common::xml_files(&diff_dir)[0]).unwrap()),
        None,
    )
    .unwrap()
    .unwrap();
    assert!(!target.apply_difference(&published).has_errors());
    assert_ne!(
        projection(&base),
        projection(&target),
        "the two models must actually differ"
    );

    // Now recover the change from the two states, and check it reproduces the target.
    let computed = base.difference_to(&target, &DiffOptions::default());
    println!(
        "computed: {} reverse, {} forward ({} added, {} removed, {} changed)",
        computed.model.reverse.len(),
        computed.model.forward.len(),
        computed.added,
        computed.removed,
        computed.changed
    );
    assert!(!computed.is_empty(), "no change was detected");
    assert!(!computed.report.has_errors(), "{}", computed.report);

    let report = base.apply_difference(&computed.model);
    assert!(!report.has_errors(), "{report}");
    assert_eq!(
        projection(&base),
        projection(&target),
        "applying the computed difference did not reproduce the target"
    );

    // And a model differs from itself by nothing at all.
    let none = target.difference_to(&target, &DiffOptions::default());
    assert!(none.is_empty(), "a model differs from itself");
}

/// Additions, removals, edits, repeated values and a class change, each on purpose.
#[test]
fn a_computed_difference_covers_every_kind_of_change() {
    use cim_rs::cgmes3::{attributes as attrs, classes};
    use cim_rs::object::Object;
    use cim_rs::value::Value;

    let a = Mrid::parse("11111111-1111-4111-8111-111111111111");
    let b = Mrid::parse("22222222-2222-4222-8222-222222222222");
    let c = Mrid::parse("33333333-3333-4333-8333-333333333333");

    let mut base = Dataset::new(SCHEMA);
    // `a`: an attribute will change. `b`: the object goes away. `c` is not there yet.
    let mut o = Object::new(classes::ACLineSegment, a.clone());
    o.set(attrs::identified_object::name, Value::Text("L".into()));
    o.set(attrs::ac_line_segment::r, Value::Float(1.5));
    base.insert(o);
    let mut o = Object::new(classes::Breaker, b.clone());
    o.set(attrs::identified_object::name, Value::Text("BR".into()));
    base.insert(o);

    let mut target = Dataset::new(SCHEMA);
    let mut o = Object::new(classes::ACLineSegment, a.clone());
    o.set(attrs::identified_object::name, Value::Text("L".into()));
    o.set(attrs::ac_line_segment::r, Value::Float(2.75));
    target.insert(o);
    let mut o = Object::new(classes::Disconnector, c.clone());
    o.set(attrs::identified_object::name, Value::Text("DS".into()));
    target.insert(o);

    let d = base.difference_to(&target, &DiffOptions::default());
    assert_eq!((d.added, d.removed, d.changed), (1, 1, 1));

    // The change set must survive a trip through the serialized form, because that is how
    // it is actually delivered.
    let mut xml = Vec::new();
    cim_rs::writer::write_difference(SCHEMA, &d.model, &mut xml, &Default::default()).unwrap();
    let xml = String::from_utf8(xml).unwrap();
    common::assert_well_formed("computed difference", &xml);
    let reread = cim_rs::reader::read_difference(SCHEMA, xml.as_bytes(), None)
        .unwrap()
        .expect("a dm:DifferenceModel");

    let report = base.apply_difference(&reread);
    assert!(!report.has_errors(), "{report}");
    // The removed object is left as an empty shell — 61970-552 has no "delete" statement.
    assert!(base.by_mrid(&b).is_some_and(|o| o.is_empty()));
    assert_eq!(base.prune_empty(), 1);
    assert_eq!(projection(&base), projection(&target));
}

/// A change set restricted to a profile carries that profile's values and no others.
#[test]
fn a_computed_difference_can_be_restricted_to_one_profile() {
    let base_dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let files = common::xml_files(&base_dir);

    let mut base = Dataset::new(SCHEMA);
    base.load_files(&files, &ReadOptions::lenient()).unwrap();
    let mut target = Dataset::new(SCHEMA);
    target.load_files(&files, &ReadOptions::lenient()).unwrap();

    // Change one Steady State Hypothesis value.
    let ssh = SCHEMA.profile_by_keyword("SSH").unwrap();
    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let attr = SCHEMA
        .find_attr("http://iec.ch/TC57/CIM100#", "RotatingMachine.p")
        .unwrap();
    let victim = target
        .iter()
        .find(|(_, o)| o.has(attr))
        .map(|(id, _)| id)
        .expect("the model sets RotatingMachine.p");
    let old = target
        .get(victim)
        .unwrap()
        .get(attr)
        .unwrap()
        .as_f64()
        .unwrap();
    target
        .get_mut(victim)
        .unwrap()
        .set_in(ssh.mask(), attr, cim_rs::Value::Float(old + 1.0));

    let in_ssh = base.difference_to(&target, &DiffOptions::default().profiles(ssh.mask()));
    assert!(
        !in_ssh.is_empty(),
        "the SSH change is missing from an SSH diff"
    );

    let in_eq = base.difference_to(&target, &DiffOptions::default().profiles(eq.mask()));
    assert!(
        in_eq.is_empty(),
        "an SSH-only change leaked into an Equipment change set: {} statements",
        in_eq.model.forward.len()
    );
}

/// A change file re-exported as itself, even when its header carries no identifier.
///
/// `save_as_loaded` reproduces the file set a model was read from, and a change file holds
/// statements rather than objects, so they have to be matched back to the header. The
/// header's identifier is the obvious key and is not enough: `md:DifferenceModel` without an
/// `rdf:about` is a defect the reader tolerates (`CIM0013`), and matching on identifier
/// alone drops such a file from the export.
#[test]
fn a_change_file_without_an_identifier_is_still_re_exported() {
    const NS: &str = "http://iec.ch/TC57/CIM100#";
    const SUBJECT: &str = "22222222-2222-4222-8222-222222222222";
    let change = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:dm="http://iec.ch/TC57/61970-552/DifferenceModel/1#">
  <dm:DifferenceModel>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/SteadyStateHypothesis-EU/3.0</md:Model.profile>
    <dm:forwardDifferences rdf:parseType="Statements">
      <rdf:Description rdf:about="#_{SUBJECT}">
        <cim:Equipment.inService>true</cim:Equipment.inService>
      </rdf:Description>
    </dm:forwardDifferences>
  </dm:DifferenceModel>
</rdf:RDF>
"##
    );
    let dir = std::env::temp_dir().join(format!("cim-diff-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in/change_DIFF.xml"), change).unwrap();

    let ds = Dataset::load_dir(SCHEMA, dir.join("in")).unwrap();
    assert_eq!(ds.differences().len(), 1);
    assert!(
        ds.headers()[0].id.is_none(),
        "this test is about a header with no identifier"
    );

    let saved = ds.save_as_loaded(&dir.join("out")).unwrap();
    assert!(
        saved.skipped.is_empty(),
        "the change file was dropped: {:?}",
        saved.skipped
    );
    assert_eq!(saved.written.len(), 1, "{saved:?}");

    let text = std::fs::read_to_string(&saved.written[0]).unwrap();
    common::assert_well_formed("re-exported change file", &text);
    assert!(text.contains("dm:forwardDifferences"), "{text}");
    assert!(text.contains("Equipment.inService"), "{text}");
    // Re-reading it gives the statement back.
    let again = Dataset::load_dir(SCHEMA, dir.join("out")).unwrap();
    assert_eq!(again.differences().len(), 1);
    assert_eq!(again.differences()[0].forward.len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}
