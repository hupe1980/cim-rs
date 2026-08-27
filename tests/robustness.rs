//! The reader must never panic, whatever bytes it is given.
//!
//! `cargo fuzz` needs a nightly toolchain, so the targets under `fuzz/` are not part of
//! the ordinary test run. This file exercises the same contract deterministically on
//! stable: a corpus of hand-written pathological documents, plus systematic mutation of
//! valid ones. It runs on every `cargo test`, so a regression cannot reach a release
//! unnoticed even if nobody has run the fuzzer lately.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::cgmes3::SCHEMA;
use cim_rs::prelude::*;
use cim_rs::reader::read_into;

const VALID: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:eu="http://iec.ch/TC57/CIM100-European#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:dm="http://iec.ch/TC57/61970-552/DifferenceModel/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <md:Model.DependentOn rdf:resource="urn:uuid:22222222-2222-4222-8222-222222222222"/>
  </md:FullModel>
  <cim:ACLineSegment rdf:ID="_33333333-3333-4333-8333-333333333333">
    <cim:IdentifiedObject.name>Line</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>1.5</cim:ACLineSegment.r>
    <eu:IdentifiedObject.shortName>L</eu:IdentifiedObject.shortName>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_44444444-4444-4444-8444-444444444444">
    <cim:Terminal.ConductingEquipment rdf:resource="#_33333333-3333-4333-8333-333333333333"/>
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
  </cim:Terminal>
  <cim:Location rdf:ID="_55555555-5555-4555-8555-555555555555">
    <cim:Location.mainAddress rdf:parseType="Resource">
      <cim:StreetAddress.postalCode>1000</cim:StreetAddress.postalCode>
      <cim:StreetAddress.townDetail rdf:parseType="Resource">
        <cim:TownDetail.name>Brussels</cim:TownDetail.name>
      </cim:StreetAddress.townDetail>
    </cim:Location.mainAddress>
  </cim:Location>
</rdf:RDF>"##;

/// Read `bytes` under both strictness settings and exercise everything downstream.
///
/// Any panic here is a bug: the documented contract is an error or a model plus
/// diagnostics, never a crash.
///
/// Returns how many of the two reads succeeded, so a caller can assert that a mutation
/// campaign is still reaching the code past the parser rather than erroring out early.
#[must_use]
fn exercise(bytes: &[u8]) -> usize {
    let mut succeeded = 0;
    for options in [ReadOptions::lenient(), ReadOptions::strict()] {
        let mut ds = Dataset::new(SCHEMA);
        let Ok(outcome) = read_into(&mut ds, bytes, Some("fuzz.xml"), &options) else {
            continue;
        };
        succeeded += 1;

        // Everything the API promises about a successfully read dataset must hold.
        let _ = cim_rs::validate::validate(&ds);
        let _ = ds.dangling_references();
        let _ = cim_rs::InverseIndex::build(&ds);
        let _ = cim_rs::validate::profile_coverage(&ds);
        for (id, o) in ds.iter() {
            assert!(ds.get(id).is_some());
            assert!(
                ds.by_mrid(o.mrid()).is_some(),
                "index disagrees with storage"
            );
            for slot in o.slots() {
                // Every stored attribute id must be one the schema defines, and must be
                // reachable from the object's class.
                let def = SCHEMA.attr(slot.attr);
                assert!(
                    SCHEMA.class(o.class()).all_attrs.contains(&slot.attr),
                    "{} stored on {} which does not have it",
                    def.name,
                    SCHEMA.class(o.class()).name
                );
            }
        }

        // Whatever was read must be writable, and our own output must be readable.
        let mut buf = Vec::new();
        if cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).is_ok() {
            // Re-reading with *our* reader is the weaker half of this, and on its own it
            // is the blind spot the well-formedness gate had: a tolerant reader accepting
            // its own output says nothing about anyone else's parser. This campaign is
            // where that matters most — it is the only place that generates untidy input
            // deliberately, so it is the only place a writer defect against untidy input
            // can show up at all.
            let text = String::from_utf8(buf.clone())
                .expect("the writer must produce UTF-8, whatever it was given");
            if let Err(e) = common::check_well_formed(&text) {
                panic!("a mutated document produced output that is not well-formed XML: {e}");
            }

            let mut back = Dataset::new(SCHEMA);
            let again = read_into(&mut back, buf.as_slice(), None, &ReadOptions::lenient())
                .expect("the writer must produce readable output");
            assert!(
                !again.report.has_errors(),
                "re-reading our own output produced errors:\n{}",
                again.report
            );
            assert_eq!(back.len(), ds.len(), "round-trip changed the object count");
        }

        if let Some(diff) = outcome.difference {
            let mut copy = Dataset::new(SCHEMA);
            let _ = read_into(&mut copy, bytes, None, &ReadOptions::lenient());
            let _ = copy.apply_difference(&diff);
        }
    }
    succeeded
}

#[test]
fn pathological_documents_do_not_panic() {
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("whitespace", "   \n\t  "),
        ("not xml", "hello world"),
        ("declaration only", r#"<?xml version="1.0"?>"#),
        ("unclosed root", "<rdf:RDF>"),
        ("unclosed element", "<rdf:RDF><cim:Terminal"),
        (
            "no namespaces",
            "<rdf:RDF><cim:Terminal rdf:ID=\"_x\"/></rdf:RDF>",
        ),
        ("wrong root", "<html><body>not cim</body></html>"),
        (
            "root without namespace declarations",
            r#"<rdf:RDF><md:FullModel rdf:about="urn:uuid:1"/></rdf:RDF>"#,
        ),
        (
            "object without identifier",
            r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <cim:Terminal><cim:IdentifiedObject.name>x</cim:IdentifiedObject.name></cim:Terminal></rdf:RDF>"##,
        ),
        (
            "deeply nested",
            r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <cim:Terminal rdf:ID="_a"><cim:IdentifiedObject.name><b><c><d>x</d></c></b></cim:IdentifiedObject.name></cim:Terminal></rdf:RDF>"##,
        ),
        (
            "attribute without an object",
            r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <cim:ACLineSegment.r>1.0</cim:ACLineSegment.r></rdf:RDF>"##,
        ),
        (
            "difference with no statements",
            r##"<rdf:RDF xmlns:dm="http://iec.ch/TC57/61970-552/DifferenceModel/1#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <dm:DifferenceModel rdf:about="urn:uuid:1"><dm:forwardDifferences rdf:parseType="Statements"/></dm:DifferenceModel></rdf:RDF>"##,
        ),
        (
            "self-referencing object",
            r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <cim:Terminal rdf:ID="_55555555-5555-4555-8555-555555555555">
                <cim:Terminal.ConductingEquipment rdf:resource="#_55555555-5555-4555-8555-555555555555"/>
                </cim:Terminal></rdf:RDF>"##,
        ),
        (
            "duplicate attribute on a single-valued field",
            r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <cim:ACLineSegment rdf:ID="_a"><cim:ACLineSegment.r>1</cim:ACLineSegment.r>
                <cim:ACLineSegment.r>2</cim:ACLineSegment.r></cim:ACLineSegment></rdf:RDF>"##,
        ),
        ("valid", VALID),
    ];

    for (name, src) in cases {
        // A panic in `exercise` fails the test and names the case.
        println!("case: {name}");
        let _ = exercise(src.as_bytes());
    }
    // The valid document must actually be readable, or the corpus proves nothing.
    assert_eq!(
        exercise(VALID.as_bytes()),
        2,
        "the valid document did not read"
    );
}

#[test]
fn truncation_at_every_byte_boundary_does_not_panic() {
    // Truncation is the failure mode of a stream that dies mid-transfer, and it reaches
    // every parser state in turn.
    let bytes = VALID.as_bytes();
    let mut readable = 0;
    for cut in 0..bytes.len() {
        readable += exercise(&bytes[..cut]);
    }
    // Truncation inside the body still yields a readable prefix under lenient rules, so
    // the downstream assertions really do run rather than every case erroring out.
    assert!(
        readable > 50,
        "only {readable} truncations parsed; the campaign is not reaching the model"
    );
}

#[test]
fn single_byte_corruption_does_not_panic() {
    // Walk one corrupting byte through the document. This reaches malformed tags,
    // broken entities, invalid UTF-8 and truncated attribute values without needing a
    // fuzzer's randomness, and it is fully deterministic.
    let original = VALID.as_bytes();
    let mut readable = 0;
    let mut cases = 0;
    for replacement in [b'<', b'>', b'"', b'&', b'/', 0x00, 0xff] {
        for i in (0..original.len()).step_by(7) {
            let mut bytes = original.to_vec();
            bytes[i] = replacement;
            readable += exercise(&bytes);
            cases += 1;
        }
    }
    println!("{cases} corruptions, {readable} still readable");
    assert!(readable > cases / 4, "too few corruptions produced a model");
}

#[test]
fn deletion_of_each_region_does_not_panic() {
    let original = VALID.as_bytes();
    let chunk = 16;
    for start in (0..original.len()).step_by(chunk) {
        let end = (start + chunk).min(original.len());
        let mut bytes = Vec::with_capacity(original.len());
        bytes.extend_from_slice(&original[..start]);
        bytes.extend_from_slice(&original[end..]);
        let _ = exercise(&bytes);
    }
}

#[test]
fn pathological_sizes_are_handled() {
    // A very long attribute value, a very long element name, and many repetitions of one
    // attribute: each stresses a different growth path.
    let long_value = "x".repeat(1 << 16);
    let doc = format!(
        r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:ACLineSegment rdf:ID="_66666666-6666-4666-8666-666666666666">
    <cim:IdentifiedObject.name>{long_value}</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
</rdf:RDF>"##
    );
    assert_eq!(exercise(doc.as_bytes()), 2, "a long value must still read");

    let long_name = "A".repeat(4096);
    let doc = format!(
        r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:{long_name} rdf:ID="_a"/>
</rdf:RDF>"##
    );
    let _ = exercise(doc.as_bytes());

    let mut repeated = String::from(
        r##"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:Diagram rdf:ID="_77777777-7777-4777-8777-777777777777">"##,
    );
    for i in 0..5000 {
        repeated.push_str(&format!(
            "<cim:IdentifiedObject.name>n{i}</cim:IdentifiedObject.name>"
        ));
    }
    repeated.push_str("</cim:Diagram></rdf:RDF>");
    assert_eq!(
        exercise(repeated.as_bytes()),
        2,
        "many repetitions must still read"
    );
}

#[test]
fn invalid_utf8_is_an_error_not_a_panic() {
    let mut bytes = VALID.as_bytes().to_vec();
    // Splice an invalid continuation byte into element text.
    if let Some(pos) = VALID.find("Line") {
        bytes[pos] = 0x80;
    }
    let _ = exercise(&bytes);

    // A document that is entirely invalid UTF-8.
    let _ = exercise(&[0xff, 0xfe, 0xfd, 0x00, 0x80, 0x81]);
}

// ---------------------------------------------------------------------------
// Deviations a malformed document produces that a valid one never does
// ---------------------------------------------------------------------------

/// Read `doc` leniently and hand back the model and what the reader had to say.
fn read_lenient(doc: &str, options: &ReadOptions) -> (Dataset, cim_rs::Report) {
    let mut ds = Dataset::new(SCHEMA);
    let outcome = read_into(&mut ds, doc.as_bytes(), Some("probe.xml"), options).expect("read");
    (ds, outcome.report)
}

fn wrap(body: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
{body}
</rdf:RDF>"##
    )
}

/// A value is the text of its own element and of no other.
///
/// IEC 61970-552 property elements hold text, one `rdf:resource`, or an inline compound —
/// never mixed content. A document that nests markup inside a value used to have that
/// markup's text read *as* the value, and the value closed at the nested element's end
/// tag: `<name><x>ignored</x></name>` came back as `ignored`.
#[test]
fn markup_nested_inside_a_value_does_not_become_the_value() {
    let doc = wrap(
        r##"  <cim:ACLineSegment rdf:ID="_33333333-3333-4333-8333-333333333333">
    <cim:IdentifiedObject.name><vendor:tag xmlns:vendor="http://v/">ignored</vendor:tag></cim:IdentifiedObject.name>
    <cim:IdentifiedObject.description>kept</cim:IdentifiedObject.description>
  </cim:ACLineSegment>"##,
    );
    let (ds, report) = read_lenient(&doc, &ReadOptions::lenient());
    let o = ds
        .by_mrid(&Mrid::parse("33333333-3333-4333-8333-333333333333"))
        .expect("the line");

    assert_eq!(
        o.get(cim_rs::cgmes3::attributes::identified_object::name)
            .and_then(|v| v.as_str()),
        Some(""),
        "the nested element's text is not the value"
    );
    // The property still ends at its own tag, so what follows it is read normally.
    assert_eq!(
        o.get(cim_rs::cgmes3::attributes::identified_object::description)
            .and_then(|v| v.as_str()),
        Some("kept")
    );
    assert!(
        report
            .by_rule(cim_rs::Rule::Structure)
            .any(|d| d.message.contains("inside a value")),
        "the deviation is reported: {report}"
    );
}

/// Objects with no identifier at all are still distinct objects.
///
/// Giving them all the same empty identifier merged them into one, so the later one's
/// values overwrote the earlier one's — under a class they need not even share.
#[test]
fn objects_without_an_identifier_do_not_collapse_into_one() {
    let doc = wrap(
        r##"  <cim:ACLineSegment>
    <cim:IdentifiedObject.name>first</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
  <cim:Terminal>
    <cim:IdentifiedObject.name>second</cim:IdentifiedObject.name>
  </cim:Terminal>"##,
    );
    let (ds, report) = read_lenient(&doc, &ReadOptions::lenient());
    assert_eq!(ds.len(), 2, "two elements, two objects");

    let mut names: Vec<&str> = ds
        .iter()
        .filter_map(|(_, o)| {
            o.get(cim_rs::cgmes3::attributes::identified_object::name)?
                .as_str()
        })
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["first", "second"]);

    // Nothing is invented: the stand-in is opaque, not a plausible-looking UUID, so every
    // later check still sees that the document was wrong.
    assert!(ds.iter().all(|(_, o)| !o.mrid().is_uuid()));
    assert_eq!(report.by_rule(cim_rs::Rule::Structure).count(), 2);
}

/// A lenient read answers a broken document with a diagnostic per problem, so the report
/// grows with how broken the input is. Past the cap it stops growing and says so.
#[test]
fn the_report_a_broken_document_produces_is_bounded() {
    let mut body = String::new();
    for i in 0..5_000 {
        body.push_str(&format!("  <cim:NoSuchClass{i}/>\n"));
    }
    let doc = wrap(&body);

    let (_, capped) = read_lenient(&doc, &ReadOptions::lenient().with_max_diagnostics(100));
    assert_eq!(capped.len(), 101, "100 findings plus the note that says so");
    assert!(
        capped
            .iter()
            .last()
            .is_some_and(|d| d.message.contains("stopped recording")),
        "the truncation is stated rather than silent"
    );

    // Zero means no limit, for a caller that wants every finding.
    let (_, all) = read_lenient(&doc, &ReadOptions::lenient().with_max_diagnostics(0));
    assert_eq!(all.len(), 5_000);
}
