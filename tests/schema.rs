//! Invariants every shipped schema vintage must satisfy.
//!
//! These hold the promises the architecture makes about generated tables, so that adding a
//! vintage or a third-party profile fails here rather than somewhere far away.

use cim_rs::schema::{ProfileId, Schema};

/// Every vintage this build has.
///
/// Written as a chain of conditional pushes rather than a literal because which vintages
/// exist is a feature-matrix question, and `cargo test --no-default-features` must still
/// compile this file down to an empty list.
#[allow(clippy::vec_init_then_push)]
fn schemas() -> Vec<&'static Schema> {
    #[allow(unused_mut)]
    let mut out: Vec<&'static Schema> = Vec::new();
    #[cfg(feature = "cgmes3")]
    out.push(cim_rs::cgmes3::SCHEMA);
    #[cfg(feature = "cgmes2")]
    out.push(cim_rs::cgmes2::SCHEMA);
    out
}

/// A profile set is data, and `ProfileMask` is 64 bits — so the design's promise that
/// twenty-nine published profiles "still leave room for a private one" is a claim about a
/// bound that nothing else checks.
///
/// It matters more than an off-by-one usually would. Rust masks an over-wide shift in
/// release builds, so a sixty-fifth profile would not fail: it would take profile 0's bit
/// and silently merge two profiles' data everywhere provenance is consulted — the writer's
/// file selection, the differ's scope, the RDF export's per-profile slice. `ProfileId::mask`
/// asserts, and this makes the assertion unreachable rather than merely present.
#[test]
fn schemas_fit_the_profile_mask() {
    for s in schemas() {
        assert!(
            s.profiles.len() <= ProfileId::MAX_PROFILES,
            "{} declares {} profiles, more than the {} bits of ProfileMask; \
             widen the type and regenerate",
            s.vintage,
            s.profiles.len(),
            ProfileId::MAX_PROFILES,
        );
        // And the masks are genuinely distinct, which is the property the bound exists for.
        let mut seen = 0u64;
        for i in 0..s.profiles.len() {
            let m = ProfileId(i as u16).mask();
            assert_eq!(
                seen & m,
                0,
                "{}: profile {i} aliases an earlier one",
                s.vintage
            );
            seen |= m;
        }
    }
}

/// Vintages are told apart by `Schema::vintage`, which `Dataset::merge` and
/// `Dataset::difference_to` compare to refuse mixing tables that mean different things.
#[test]
fn vintages_are_distinguishable() {
    let all = schemas();
    for (i, a) in all.iter().enumerate() {
        assert!(!a.vintage.is_empty());
        for b in &all[i + 1..] {
            assert_ne!(a.vintage, b.vintage, "two vintages share a name");
        }
    }
}

/// A document says which vintage it is; the caller should not have to guess.
///
/// Guessing wrong is expensive and quiet: reading a CGMES 2.4.15 file against the CGMES 3.0
/// tables resolves no class at all, so the result is an empty model plus one "unknown class"
/// warning per element — which a caller checking only the exit status reads as success.
#[test]
fn a_documents_namespaces_identify_its_vintage() {
    let cim16 = [
        "http://iec.ch/TC57/2013/CIM-schema-cim16#",
        "http://entsoe.eu/CIM/SchemaExtension/3/1#",
        "http://iec.ch/TC57/61970-552/ModelDescription/1#",
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    ];
    let cim100 = [
        "http://iec.ch/TC57/CIM100#",
        "http://iec.ch/TC57/CIM100-European#",
        "http://iec.ch/TC57/61970-552/ModelDescription/1#",
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    ];

    #[cfg(all(feature = "cgmes2", feature = "cgmes3"))]
    {
        use cim_rs::schema::Schema;
        assert_eq!(Schema::detect(cim16).map(|s| s.vintage), Some("cgmes2"));
        assert_eq!(Schema::detect(cim100).map(|s| s.vintage), Some("cgmes3"));

        // The margin is the point: the shared 61970-552 header vocabulary holds classes, so
        // every vintage scores something on every CIM/XML document and only the size of the
        // score separates them.
        let by = |ns: &[&str], v: &str| {
            cim_rs::VINTAGES
                .iter()
                .find(|s| s.vintage == v)
                .unwrap()
                .match_score(ns.iter().copied())
        };
        assert!(by(&cim16, "cgmes2") > 100 && by(&cim16, "cgmes3") < 10);
        assert!(by(&cim100, "cgmes3") > 100 && by(&cim100, "cgmes2") < 10);
    }

    // Nothing CIM about it is nothing to detect.
    assert!(cim_rs::schema::Schema::detect(["http://example.com/x#"]).is_none());
    assert!(cim_rs::schema::Schema::detect([]).is_none());
    let _ = (cim16, cim100);
}

/// `reader::sniff` answers the same question from a document, reading only its root.
#[test]
#[cfg(feature = "cgmes3")]
fn sniffing_a_document_reads_no_further_than_it_must() {
    let doc = concat!(
        r#"<?xml version="1.0"?>"#,
        r#"<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#""#,
        r#" xmlns:eu="http://iec.ch/TC57/CIM100-European#""#,
        r#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
        r#"<cim:Terminal rdf:ID="_x"/></rdf:RDF>"#
    );
    let found = cim_rs::reader::sniff(doc.as_bytes()).unwrap();
    assert_eq!(found.map(|s| s.vintage), Some("cgmes3"));

    // Not a CIM document, and truncated input: an answer of "no idea", not an error.
    assert!(
        cim_rs::reader::sniff(&b"<html><body>hi</body></html>"[..])
            .unwrap()
            .is_none()
    );
    assert!(cim_rs::reader::sniff(&b""[..]).unwrap().is_none());
}

/// Reading against the wrong vintage says so once, at the root, as an error.
#[test]
#[cfg(all(feature = "cgmes2", feature = "cgmes3"))]
fn a_vintage_mismatch_is_reported_before_it_becomes_noise() {
    let doc = concat!(
        r#"<?xml version="1.0"?>"#,
        r#"<rdf:RDF xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#""#,
        r#" xmlns:entsoe="http://entsoe.eu/CIM/SchemaExtension/3/1#""#,
        r#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
        r#"<cim:Terminal rdf:ID="_x"/></rdf:RDF>"#
    );
    let mut ds = cim_rs::Dataset::new(cim_rs::cgmes3::SCHEMA);
    let out = cim_rs::reader::read_into(
        &mut ds,
        doc.as_bytes(),
        Some("t.xml"),
        &cim_rs::ReadOptions::lenient(),
    )
    .unwrap();

    let mismatches: Vec<_> = out.report.by_rule(cim_rs::Rule::WrongVintage).collect();
    assert_eq!(mismatches.len(), 1, "expected exactly one: {}", out.report);
    let m = &mismatches[0];
    assert_eq!(m.severity, cim_rs::Severity::Error);
    assert!(m.message.contains("cgmes2"), "{}", m.message);

    // And the right pairing says nothing at all.
    let mut ok = cim_rs::Dataset::new(cim_rs::cgmes2::SCHEMA);
    let out = cim_rs::reader::read_into(
        &mut ok,
        doc.as_bytes(),
        Some("t.xml"),
        &cim_rs::ReadOptions::lenient(),
    )
    .unwrap();
    assert_eq!(out.report.by_rule(cim_rs::Rule::WrongVintage).count(), 0);
    assert_eq!(ok.len(), 1);
}
