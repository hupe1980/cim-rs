//! Standard RDF output.
//!
//! CIM/XML is not RDF/XML and carries no datatypes, so a CGMES consumer cannot hand it to
//! an RDF toolchain or validate it against the SHACL shapes ENTSO-E publishes beside the
//! RDFS this crate is generated from. These tests pin the mapping that closes that gap:
//! `urn:uuid:` subjects, predicates from the vocabulary, and every literal typed from the
//! profile.
//!
//! The end-to-end proof — exporting a published conformity model and validating it against
//! ENTSO-E's own shapes with a SHACL engine — is `cargo xtask shacl`, since running
//! SHACL is deliberately not this crate's job.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::MridForm;
use cim_rs::cgmes3::{SCHEMA, names::attributes as attrs, names::classes};
use cim_rs::prelude::*;
use cim_rs::rdf::{RdfOptions, Syntax};
use cim_rs::schema::Primitive;

const NS: &str = "http://iec.ch/TC57/CIM100#";
const A: &str = "22222222-2222-4222-8222-222222222222";
const B: &str = "33333333-3333-4333-8333-333333333333";

fn model() -> Dataset {
    let src = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.created>2021-03-25T23:16:27Z</md:Model.created>
    <md:Model.version>001</md:Model.version>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>Line "A"</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>1.25</cim:ACLineSegment.r>
    <cim:Equipment.aggregate>false</cim:Equipment.aggregate>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_{B}">
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
    <cim:Terminal.phases rdf:resource="{NS}PhaseCode.ABC"/>
    <cim:Terminal.ConductingEquipment rdf:resource="#_{A}"/>
  </cim:Terminal>
</rdf:RDF>"##
    );
    let mut ds = Dataset::new(SCHEMA);
    let outcome = cim_rs::reader::read_into(
        &mut ds,
        src.as_bytes(),
        Some("t.xml"),
        &ReadOptions::lenient(),
    )
    .unwrap();
    assert!(!outcome.report.has_errors(), "{}", outcome.report);
    ds
}

fn render(ds: &Dataset, options: &RdfOptions) -> String {
    cim_rs::rdf::to_string(ds, options).unwrap()
}

#[test]
fn every_literal_carries_the_type_the_profile_declares() {
    // This is the whole point: CIM/XML transports `1.25`, `1` and `false` as identical
    // element text, and ENTSO-E's shapes constrain 3,137 properties by `sh:datatype`.
    let nt = render(&model(), &RdfOptions::new(Syntax::NTriples));
    let n = common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}\n{nt}"));
    assert!(n > 8, "only {n} triples:\n{nt}");

    for expected in [
        // A float, and specifically `xsd:float` — not the `xsd:double` a 64-bit
        // representation would suggest. ENTSO-E's shapes say float.
        concat!(
            "<urn:uuid:22222222-2222-4222-8222-222222222222> ",
            "<http://iec.ch/TC57/CIM100#ACLineSegment.r> ",
            "\"1.25\"^^<http://www.w3.org/2001/XMLSchema#float> ."
        ),
        concat!(
            "<urn:uuid:33333333-3333-4333-8333-333333333333> ",
            "<http://iec.ch/TC57/CIM100#ACDCTerminal.sequenceNumber> ",
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer> ."
        ),
        concat!(
            "<urn:uuid:22222222-2222-4222-8222-222222222222> ",
            "<http://iec.ch/TC57/CIM100#Equipment.aggregate> ",
            "\"false\"^^<http://www.w3.org/2001/XMLSchema#boolean> ."
        ),
        // RDF 1.1 makes an untyped literal an `xsd:string` already, and the quotes in the
        // value have to be escaped rather than ending it.
        concat!(
            "<urn:uuid:22222222-2222-4222-8222-222222222222> ",
            "<http://iec.ch/TC57/CIM100#IdentifiedObject.name> ",
            "\"Line \\\"A\\\"\" ."
        ),
        // An enumeration is an IRI, not a string.
        concat!(
            "<urn:uuid:33333333-3333-4333-8333-333333333333> ",
            "<http://iec.ch/TC57/CIM100#Terminal.phases> ",
            "<http://iec.ch/TC57/CIM100#PhaseCode.ABC> ."
        ),
        // An association is the target's IRI, in the `urn:uuid:` form IEC 61970-552 gives
        // `rdf:ID="_<uuid>"`.
        concat!(
            "<urn:uuid:33333333-3333-4333-8333-333333333333> ",
            "<http://iec.ch/TC57/CIM100#Terminal.ConductingEquipment> ",
            "<urn:uuid:22222222-2222-4222-8222-222222222222> ."
        ),
        // Types, because a SHACL shape selects its targets with `sh:targetClass`.
        concat!(
            "<urn:uuid:22222222-2222-4222-8222-222222222222> ",
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ",
            "<http://iec.ch/TC57/CIM100#ACLineSegment> ."
        ),
    ] {
        assert!(nt.contains(expected), "missing:\n  {expected}\ngot:\n{nt}");
    }
}

#[test]
fn the_header_is_typed_from_the_header_profile() {
    // The exchange header is a profile of its own, and ENTSO-E ships shapes for it that
    // require `xsd:dateTime`, `xsd:integer` and `xsd:anyURI`. Writing the header by hand
    // as plain strings fails all of them, on every file.
    let nt = render(&model(), &RdfOptions::new(Syntax::NTriples));
    let md = "http://iec.ch/TC57/61970-552/ModelDescription/1#";
    let xsd = "http://www.w3.org/2001/XMLSchema#";
    for expected in [
        format!("<{md}Model.created> \"2021-03-25T23:16:27Z\"^^<{xsd}dateTime> ."),
        format!("<{md}Model.scenarioTime> \"2021-03-25T15:30:00Z\"^^<{xsd}dateTime> ."),
        format!("<{md}Model.version> \"001\"^^<{xsd}integer> ."),
        format!(
            "<{md}Model.profile> \"http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0\"^^<{xsd}anyURI> ."
        ),
        format!("<{md}FullModel> ."),
    ] {
        assert!(nt.contains(&expected), "missing:\n  {expected}\ngot:\n{nt}");
    }
}

#[test]
fn the_schemas_xsd_mapping_matches_the_published_shapes() {
    // Taken from ENTSO-E's CGMES 3.0 SHACL: of 3,137 `sh:datatype` constraints, 2,871
    // name `xsd:float`, 140 `xsd:boolean`, 76 `xsd:string`, 37 `xsd:integer`,
    // 7 `xsd:dateTime`, 2 `xsd:gMonthDay`, 2 `xsd:decimal` and 2 `xsd:anyURI`.
    let xsd = "http://www.w3.org/2001/XMLSchema#";
    assert_eq!(Primitive::Float.xsd_datatype(), format!("{xsd}float"));
    assert_eq!(Primitive::Boolean.xsd_datatype(), format!("{xsd}boolean"));
    assert_eq!(Primitive::Integer.xsd_datatype(), format!("{xsd}integer"));
    assert_eq!(Primitive::Decimal.xsd_datatype(), format!("{xsd}decimal"));
    assert_eq!(Primitive::DateTime.xsd_datatype(), format!("{xsd}dateTime"));
    assert_eq!(
        Primitive::MonthDay.xsd_datatype(),
        format!("{xsd}gMonthDay")
    );
    assert_eq!(Primitive::Uri.xsd_datatype(), format!("{xsd}anyURI"));
    // An untyped RDF 1.1 literal is an `xsd:string`, so writing the type is redundant.
    assert_eq!(Primitive::String.xsd_datatype(), "");

    // A unit-carrying CIM datatype serializes as its primitive, so it gets that type.
    let r = SCHEMA.find_attr(NS, "ACLineSegment.r").unwrap();
    let cim_rs::schema::AttrKind::Datatype(dt) = SCHEMA.attr(r).kind else {
        panic!("ACLineSegment.r should be a CIM datatype");
    };
    assert_eq!(SCHEMA.datatype(dt).value, Primitive::Float);
    assert_eq!(SCHEMA.datatype(dt).unit, Some("ohm"));
}

#[test]
fn turtle_and_n_triples_describe_the_same_graph() {
    let ds = model();
    let nt = render(&ds, &RdfOptions::new(Syntax::NTriples));
    let ttl = render(&ds, &RdfOptions::new(Syntax::Turtle));
    let triples = common::check_ntriples(&nt).unwrap();

    // Turtle groups by subject: each block is one subject line plus one `;` per further
    // statement, so the two counts add up to the same graph.
    let subjects = ttl
        .lines()
        .filter(|l| l.starts_with('<') || l.starts_with("_:"))
        .count();
    let statements = subjects + ttl.matches(" ;\n").count();
    assert_eq!(
        statements, triples,
        "Turtle holds {statements} statements but N-Triples holds {triples}\n{ttl}"
    );

    // And the abbreviations are the ones a reader expects.
    assert!(
        ttl.contains("@prefix cim: <http://iec.ch/TC57/CIM100#> ."),
        "{ttl}"
    );
    assert!(ttl.contains("    a cim:ACLineSegment ;"), "{ttl}");
    assert!(
        ttl.contains("cim:ACLineSegment.r \"1.25\"^^xsd:float"),
        "{ttl}"
    );
    assert!(
        ttl.contains("cim:Terminal.phases cim:PhaseCode.ABC"),
        "{ttl}"
    );
}

#[test]
fn compounds_become_typed_blank_nodes() {
    // A compound has no identity, which is what a blank node is for. Flattening one to
    // text — what a reader without the profile does — invents a value.
    let src = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:Location rdf:ID="_{A}">
    <cim:Location.mainAddress rdf:parseType="Resource">
      <cim:StreetAddress.townDetail rdf:parseType="Resource">
        <cim:TownDetail.name>Brussels</cim:TownDetail.name>
      </cim:StreetAddress.townDetail>
    </cim:Location.mainAddress>
  </cim:Location>
</rdf:RDF>"##
    );
    let mut ds = Dataset::new(SCHEMA);
    cim_rs::reader::read_into(&mut ds, src.as_bytes(), None, &ReadOptions::lenient()).unwrap();

    let nt = render(&ds, &RdfOptions::new(Syntax::NTriples));
    common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}\n{nt}"));
    assert!(
        nt.contains("<http://iec.ch/TC57/CIM100#Location.mainAddress> _:c1 ."),
        "{nt}"
    );
    assert!(
        nt.contains("_:c1 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://iec.ch/TC57/CIM100#StreetAddress> ."),
        "{nt}"
    );
    assert!(
        nt.contains("_:c2 <http://iec.ch/TC57/CIM100#TownDetail.name> \"Brussels\" ."),
        "{nt}"
    );
}

#[test]
fn an_identifier_that_is_no_uuid_becomes_a_blank_node_or_a_named_one() {
    // IEC 61970-552 requires a UUID; this crate keeps a deviating identifier verbatim and
    // flags it. There is no `urn:uuid:` form for something that is not a UUID at all, and
    // inventing one would misreport what the document said — so it is a blank node unless
    // the caller supplies a base.
    let mut ds = Dataset::new(SCHEMA);
    let weird = Mrid::parse("BE-Line_7");
    assert_eq!(weird.form(), MridForm::Opaque);
    let mut a = Object::new(classes::ACLineSegment, weird.clone());
    a.set(attrs::identified_object::name, Value::Text("L".into()));
    ds.insert(a);
    let mut t = Object::new(classes::Terminal, Mrid::parse(B));
    t.set(
        attrs::terminal::ConductingEquipment,
        Value::Reference(weird.clone()),
    );
    ds.insert(t);

    let nt = render(&ds, &RdfOptions::new(Syntax::NTriples));
    common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}\n{nt}"));
    // The label has to be the same on both sides, or the reference stops joining. It is
    // hex-encoded here because `BE-Line_7` is not a bare blank-node label.
    let label = cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::NTriples))
        .unwrap()
        .split_whitespace()
        .find(|t| t.starts_with("_:"))
        .expect("a blank node")
        .to_owned();
    assert!(
        nt.contains(&format!(
            "{label} <http://iec.ch/TC57/CIM100#IdentifiedObject.name>"
        )),
        "{nt}"
    );
    assert!(
        nt.contains(&format!("Terminal.ConductingEquipment> {label} .")),
        "{nt}"
    );

    // With a base the object gets a global name instead.
    let named = render(
        &ds,
        &RdfOptions::new(Syntax::NTriples).with_base("https://example.invalid/id/"),
    );
    common::check_ntriples(&named).unwrap();
    assert!(
        named.contains("<https://example.invalid/id/BE-Line_7>"),
        "{named}"
    );
    assert!(!named.contains("_:"), "{named}");
}

/// A UUID written without hyphens is still a UUID, and must get a `urn:uuid:` IRI.
///
/// Published CGMES 2.4.15 boundary files spell identifiers that way. Treating them as
/// opaque text — which is the obvious reading of "not the form 61970-552 requires" — put
/// every object of those files into the graph as a **blank node**, which has no global
/// name, cannot be the target of a SHACL `sh:targetNode`, and does not join to the same
/// object exported from a different file. The spelling is a property of the document; the
/// identity is not.
#[test]
fn a_compact_uuid_keeps_its_global_name_in_rdf() {
    let compact = Mrid::parse("_c1d5c14b8f8011e08e4d00247eb1f55e");
    assert_eq!(compact.form(), MridForm::Compact);

    let mut ds = Dataset::new(SCHEMA);
    let mut a = Object::new(classes::ACLineSegment, compact.clone());
    a.set(attrs::identified_object::name, Value::Text("L".into()));
    ds.insert(a);
    // A second file referring to the same UUID *with* hyphens must reach the same object.
    let mut t = Object::new(classes::Terminal, Mrid::parse(B));
    t.set(
        attrs::terminal::ConductingEquipment,
        Value::Reference(Mrid::parse("#_c1d5c14b-8f80-11e0-8e4d-00247eb1f55e")),
    );
    ds.insert(t);
    assert_eq!(ds.len(), 2, "the two spellings are one object");

    let nt = render(&ds, &RdfOptions::new(Syntax::NTriples));
    common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}\n{nt}"));
    let iri = "<urn:uuid:c1d5c14b-8f80-11e0-8e4d-00247eb1f55e>";
    assert!(!nt.contains("_:"), "no blank node should appear:\n{nt}");
    assert!(
        nt.contains(&format!(
            "{iri} <http://iec.ch/TC57/CIM100#IdentifiedObject.name>"
        )),
        "{nt}"
    );
    assert!(
        nt.contains(&format!("Terminal.ConductingEquipment> {iri} .")),
        "{nt}"
    );

    // …while the CIM/XML writer still reproduces the spelling the document used.
    let mut xml = Vec::new();
    cim_rs::writer::write(&ds, &mut xml, &Default::default()).unwrap();
    let xml = String::from_utf8(xml).unwrap();
    assert!(
        xml.contains(r#"rdf:ID="_c1d5c14b8f8011e08e4d00247eb1f55e""#),
        "{xml}"
    );
}

#[test]
fn a_profile_graph_holds_only_what_that_profile_describes() {
    // ENTSO-E's shapes are per profile, and a profile constrains a reference's target to
    // the classes *it* declares. Exporting the merged graph and validating it against one
    // profile's shapes therefore reports violations that no instance file could have: the
    // Steady State Hypothesis shapes reject a `cim:Breaker`, because an SSH file writes
    // `cim:Equipment`.
    let src = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:Breaker rdf:ID="_{A}">
    <cim:IdentifiedObject.mRID>{A}</cim:IdentifiedObject.mRID>
    <cim:IdentifiedObject.name>BR</cim:IdentifiedObject.name>
  </cim:Breaker>
</rdf:RDF>
"##
    );
    let mut ds = Dataset::new(SCHEMA);
    cim_rs::reader::read_into(&mut ds, src.as_bytes(), None, &ReadOptions::lenient()).unwrap();
    // A second file, serving SSH, that only says the breaker is in service.
    let ssh = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:44444444-4444-4444-8444-444444444444">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/SteadyStateHypothesis-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:Equipment rdf:about="#_{A}">
    <cim:Equipment.inService>true</cim:Equipment.inService>
  </cim:Equipment>
  <cim:Terminal rdf:about="#_{B}">
    <cim:ACDCTerminal.connected>true</cim:ACDCTerminal.connected>
  </cim:Terminal>
</rdf:RDF>
"##
    );
    cim_rs::reader::read_into(&mut ds, ssh.as_bytes(), None, &ReadOptions::lenient()).unwrap();

    let ssh_profile = SCHEMA.profile_by_keyword("SSH").unwrap();
    let nt = render(
        &ds,
        &RdfOptions::new(Syntax::NTriples).profiles(ssh_profile.mask()),
    );
    common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}\n{nt}"));

    // Typed as the class SSH declares, exactly as the SSH file writes it.
    assert!(
        nt.contains("<http://iec.ch/TC57/CIM100#Equipment> ."),
        "{nt}"
    );
    assert!(
        !nt.contains("#Breaker>"),
        "SSH graph names a class SSH cannot:\n{nt}"
    );
    // The Equipment file's name is not part of the SSH graph.
    assert!(!nt.contains("\"BR\""), "{nt}");

    // The full graph, by contrast, keeps the most specific class.
    let all = render(&ds, &RdfOptions::new(Syntax::NTriples));
    assert!(all.contains("#Breaker> ."), "{all}");
    assert!(all.contains("\"BR\""), "{all}");
}

/// A profile's graph carries the headers of the files serving that profile and no others,
/// for the same reason it carries only that profile's objects.
///
/// Nothing else here can see this: the graph stays valid N-Triples, every literal keeps its
/// type, and the extra subjects are `md:FullModel`s the header shapes accept — while the SSH
/// graph asserts that the model declares the Equipment profile.
#[test]
fn a_profile_graph_carries_only_the_headers_of_files_serving_it() {
    let ds = two_file_model();
    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let ssh = SCHEMA.profile_by_keyword("SSH").unwrap();

    let ssh_graph = render(&ds, &RdfOptions::new(Syntax::NTriples).profiles(ssh.mask()));
    common::check_ntriples(&ssh_graph).unwrap_or_else(|e| panic!("{e}\n{ssh_graph}"));
    assert!(
        ssh_graph.contains("urn:uuid:44444444-4444-4444-8444-444444444444"),
        "the SSH file's own header is missing:\n{ssh_graph}"
    );
    assert!(
        !ssh_graph.contains("urn:uuid:11111111-1111-4111-8111-111111111111"),
        "the SSH graph asserts the Equipment file's header:\n{ssh_graph}"
    );
    assert!(
        !ssh_graph.contains("CoreEquipment-EU"),
        "the SSH graph declares a profile no SSH file declares:\n{ssh_graph}"
    );

    // And the other way round, so this is scoping rather than a header being dropped.
    let eq_graph = render(&ds, &RdfOptions::new(Syntax::NTriples).profiles(eq.mask()));
    assert!(eq_graph.contains("CoreEquipment-EU"), "{eq_graph}");
    assert!(!eq_graph.contains("SteadyStateHypothesis-EU"), "{eq_graph}");

    // The merged graph is the whole model, headers included.
    let all = render(&ds, &RdfOptions::new(Syntax::NTriples));
    assert!(all.contains("CoreEquipment-EU"), "{all}");
    assert!(all.contains("SteadyStateHypothesis-EU"), "{all}");
}

/// A graph of nothing but headers is not an export of a profile, which is what keeps a
/// SHACL run from passing on an empty one.
#[test]
fn a_profile_the_model_says_nothing_about_has_no_content() {
    let ds = two_file_model();
    for keyword in ["EQ", "SSH"] {
        let p = SCHEMA.profile_by_keyword(keyword).unwrap();
        assert!(
            cim_rs::rdf::has_content(&ds, p.mask()),
            "{keyword} carries data"
        );
    }
    // Nothing in this model is a diagram, a geographical location or a dynamics model, and
    // no loaded file declares those profiles either.
    for keyword in ["DL", "GL", "DY", "SV", "TP"] {
        let p = SCHEMA.profile_by_keyword(keyword).unwrap();
        assert!(
            !cim_rs::rdf::has_content(&ds, p.mask()),
            "{keyword} reported as carrying data"
        );
        // And the graph that would be written for it really is empty of model content:
        // only the prologue survives, so nothing is being hidden by the predicate.
        let graph = render(&ds, &RdfOptions::new(Syntax::NTriples).profiles(p.mask()));
        assert_eq!(
            common::check_ntriples(&graph).unwrap(),
            0,
            "{keyword} graph is not empty:\n{graph}"
        );
    }
    assert!(cim_rs::rdf::has_content(&ds, 0), "the whole model has data");
    assert!(!cim_rs::rdf::has_content(&Dataset::new(SCHEMA), 0));
}

/// An Equipment file and a Steady State Hypothesis file describing one breaker.
fn two_file_model() -> Dataset {
    let eq = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:Breaker rdf:ID="_{A}">
    <cim:IdentifiedObject.mRID>{A}</cim:IdentifiedObject.mRID>
    <cim:IdentifiedObject.name>BR</cim:IdentifiedObject.name>
  </cim:Breaker>
</rdf:RDF>
"##
    );
    let ssh = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:44444444-4444-4444-8444-444444444444">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/SteadyStateHypothesis-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:Equipment rdf:about="#_{A}">
    <cim:Equipment.inService>true</cim:Equipment.inService>
  </cim:Equipment>
</rdf:RDF>
"##
    );
    let mut ds = Dataset::new(SCHEMA);
    for (src, name) in [(eq, "EQ.xml"), (ssh, "SSH.xml")] {
        cim_rs::reader::read_into(&mut ds, src.as_bytes(), Some(name), &ReadOptions::lenient())
            .unwrap();
    }
    ds
}

/// The Turtle the documentation shows is the Turtle the crate writes.
///
/// A sample is the first thing a reader believes and the last thing anyone re-runs. This one
/// was invented: it paired a `TopologicalNode`'s identifier from one model with an
/// `ACLineSegment`'s name from another, and read as plausible output for years.
#[test]
fn the_documented_turtle_sample_is_real_output() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-BE-MAS"
    ));
    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(common::xml_files(&dir), &ReadOptions::lenient())
        .unwrap();
    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let ttl =
        cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::Turtle).profiles(eq.mask())).unwrap();

    for line in [
        "<urn:uuid:17086487-56ba-4979-b8de-064025a6b4da>",
        "    a cim:ACLineSegment ;",
        "    cim:ACLineSegment.r \"2.2\"^^xsd:float ;",
        "    cim:ConductingEquipment.BaseVoltage <urn:uuid:a7f1d8de-d658-428a-821b-3a5ae5965fd1> ;",
        "    cim:Equipment.aggregate \"false\"^^xsd:boolean ;",
        "    cim:IdentifiedObject.name \"BE-Line_1\" ;",
        "    eu:IdentifiedObject.shortName \"BE-L_1\" .",
    ] {
        assert!(
            ttl.contains(line),
            "the README and site show a line the export does not produce:\n  {line}"
        );
    }
}

#[test]
fn an_empty_model_still_produces_a_valid_document() {
    let ds = Dataset::new(SCHEMA);
    let nt = render(&ds, &RdfOptions::new(Syntax::NTriples));
    assert_eq!(common::check_ntriples(&nt).unwrap(), 0);
    let ttl = render(&ds, &RdfOptions::new(Syntax::Turtle));
    assert!(ttl.contains("@prefix cim:"), "{ttl}");
    // No dangling subject left open at the end.
    assert!(!ttl.trim_end().ends_with(';'), "{ttl}");
}
