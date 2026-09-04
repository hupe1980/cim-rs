//! Reader and writer behaviour on hand-written documents.
//!
//! These run without the ENTSO-E corpus, so they hold on a fresh clone and pin down the
//! exact IEC 61970-552 rules the conformity models only exercise incidentally.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::cgmes3::{SCHEMA, names::attributes as attrs, names::classes, views};
use cim_rs::error::Rule;
use cim_rs::prelude::*;
use cim_rs::reader::{Strictness, read_into};

const NS: &str = "http://iec.ch/TC57/CIM100#";
const EU: &str = "http://iec.ch/TC57/CIM100-European#";

fn doc(body: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}" xmlns:eu="{EU}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
{body}
</rdf:RDF>"##
    )
}

fn header(profile: &str) -> String {
    format!(
        r##"  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.created>2021-03-25T23:16:27Z</md:Model.created>
    <md:Model.version>001</md:Model.version>
    <md:Model.profile>{profile}</md:Model.profile>
  </md:FullModel>"##
    )
}

fn read(source: &str) -> (Dataset, cim_rs::Report) {
    read_opts(source, &ReadOptions::lenient())
}

fn read_opts(source: &str, options: &ReadOptions) -> (Dataset, cim_rs::Report) {
    let mut ds = Dataset::new(SCHEMA);
    // The reader registers the header itself, so each object is recorded against the file
    // it came from.
    let outcome = read_into(&mut ds, source.as_bytes(), Some("test.xml"), options).unwrap();
    (ds, outcome.report)
}

const A: &str = "22222222-2222-4222-8222-222222222222";
const B: &str = "33333333-3333-4333-8333-333333333333";

#[test]
fn reads_the_identity_forms_iec_61970_552_defines() {
    // `rdf:ID` defines an object; `rdf:about` describes one defined elsewhere. Both must
    // resolve to the same identifier so the two files merge.
    let src = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>Line A</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
  <cim:ACLineSegment rdf:about="#_{A}">
    <cim:ACLineSegment.r>1.25</cim:ACLineSegment.r>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert!(report.is_empty(), "{report}");
    assert_eq!(ds.len(), 1, "rdf:ID and rdf:about must denote one object");

    let line = ds
        .view_by_mrid::<views::ACLineSegment>(&Mrid::parse(A))
        .expect("object present");
    assert_eq!(line.name(), Some("Line A"));
    assert_eq!(line.r(), Some(1.25));
}

#[test]
fn reads_values_references_and_enumerations() {
    let src = doc(&format!(
        r##"{}
  <cim:Terminal rdf:ID="_{A}">
    <cim:IdentifiedObject.name>T1</cim:IdentifiedObject.name>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
    <cim:Terminal.ConductingEquipment rdf:resource="#_{B}"/>
    <cim:Terminal.phases rdf:resource="{NS}PhaseCode.ABC"/>
  </cim:Terminal>
  <cim:Breaker rdf:ID="_{B}">
    <cim:IdentifiedObject.name>BRK</cim:IdentifiedObject.name>
    <cim:Switch.normalOpen>false</cim:Switch.normalOpen>
  </cim:Breaker>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert!(report.is_empty(), "{report}");

    let t = ds
        .view_by_mrid::<views::Terminal>(&Mrid::parse(A))
        .expect("terminal");
    assert_eq!(t.sequence_number(), Some(2));

    // Enumeration literals resolve to schema ids, not raw strings.
    let phases = t.phases().expect("phases enum");
    assert_eq!(SCHEMA.enum_value(phases).name, "PhaseCode.ABC");

    // References resolve through the dataset to a typed view.
    let eq = t.conducting_equipment_in(&ds).expect("resolved equipment");
    assert_eq!(eq.name(), Some("BRK"));

    let brk = ds.view_by_mrid::<views::Breaker>(&Mrid::parse(B)).unwrap();
    assert_eq!(brk.normal_open(), Some(false));
}

#[test]
fn reads_attributes_from_the_european_extension_namespace() {
    // CGMES 3.0 mixes `cim:` and `eu:` attributes on the same object.
    let src = doc(&format!(
        r##"{}
  <cim:Terminal rdf:ID="_{A}">
    <cim:IdentifiedObject.name>T1</cim:IdentifiedObject.name>
    <eu:IdentifiedObject.shortName>T</eu:IdentifiedObject.shortName>
  </cim:Terminal>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert!(report.is_empty(), "{report}");

    let short = SCHEMA
        .find_attr(EU, "IdentifiedObject.shortName")
        .expect("eu attribute is in the schema");
    let obj = ds.by_mrid(&Mrid::parse(A)).unwrap();
    assert_eq!(obj.get(short).and_then(|v| v.as_str()), Some("T"));

    // Writing must put it back in the eu namespace, not cim.
    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(
        text.contains("<eu:IdentifiedObject.shortName>"),
        "extension namespace lost on write:\n{text}"
    );
}

#[test]
fn header_fields_are_read_including_repeated_profiles() {
    let src = doc(
        r##"  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.created>2021-03-25T23:16:27Z</md:Model.created>
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.description>Test</md:Model.description>
    <md:Model.modelingAuthoritySet>http://example.test/CGMES</md:Model.modelingAuthoritySet>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <md:Model.version>001</md:Model.version>
    <md:Model.DependentOn rdf:resource="urn:uuid:536f9bf1-3f8f-a546-87e3-7af2272f29b7"/>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/ShortCircuit-EU/3.0</md:Model.profile>
  </md:FullModel>"##,
    );
    let (ds, _) = read(&src);
    let h = &ds.headers()[0];
    assert_eq!(h.created.as_deref(), Some("2021-03-25T23:16:27Z"));
    assert_eq!(h.description.as_deref(), Some("Test"));
    assert_eq!(h.version.as_deref(), Some("001"));
    assert_eq!(
        h.modeling_authority_set.as_deref(),
        Some("http://example.test/CGMES")
    );
    // One file may declare several profiles; both must survive, in order.
    assert_eq!(h.profiles.len(), 2, "repeated md:Model.profile lost");
    assert!(h.declares_profile("http://iec.ch/TC57/ns/CIM/ShortCircuit-EU/3.0"));
    assert_eq!(h.dependent_on.len(), 1);
    assert!(h.dependent_on[0].is_uuid());

    // The declared profiles resolve, so the dataset knows its scope.
    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let sc = SCHEMA.profile_by_keyword("SC").unwrap();
    assert_eq!(ds.profiles(), eq.mask() | sc.mask());
}

#[test]
fn unknown_elements_are_reported_and_skipped_but_never_fail_by_default() {
    let src = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>Line A</cim:IdentifiedObject.name>
    <cim:ACLineSegment.vendorSpecific>7</cim:ACLineSegment.vendorSpecific>
  </cim:ACLineSegment>
  <cim:NotAClass rdf:ID="_{B}">
    <cim:NotAClass.field>x</cim:NotAClass.field>
  </cim:NotAClass>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);

    // The known object survives intact.
    assert_eq!(ds.len(), 1);
    let line = ds
        .view_by_mrid::<views::ACLineSegment>(&Mrid::parse(A))
        .unwrap();
    assert_eq!(line.name(), Some("Line A"));

    // Both deviations are reported rather than silently dropped.
    assert_eq!(
        report.by_rule(Rule::UnknownAttribute).count(),
        1,
        "{report}"
    );
    assert_eq!(report.by_rule(Rule::UnknownClass).count(), 1, "{report}");
    assert!(!report.has_errors(), "extensions are warnings, not errors");
    assert!(
        report
            .iter()
            .all(|d| d.source.as_deref() == Some("test.xml"))
    );
}

#[test]
fn strict_mode_refuses_what_the_schema_does_not_define() {
    let src = doc(&format!(
        r##"{}
  <cim:NotAClass rdf:ID="_{B}"/>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let mut ds = Dataset::new(SCHEMA);
    let err = read_into(&mut ds, src.as_bytes(), None, &ReadOptions::strict()).unwrap_err();
    assert!(
        matches!(err, cim_rs::Error::NotCimXml(_)),
        "expected a schema error, got {err:?}"
    );
}

#[test]
fn malformed_values_are_reported_without_losing_the_object() {
    let src = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>Line A</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>not-a-number</cim:ACLineSegment.r>
    <cim:ACLineSegment.x>3.5</cim:ACLineSegment.x>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert_eq!(report.by_rule(Rule::InvalidValue).count(), 1, "{report}");

    let line = ds
        .view_by_mrid::<views::ACLineSegment>(&Mrid::parse(A))
        .unwrap();
    assert_eq!(line.r(), None, "unparseable value must not be stored");
    assert_eq!(line.x(), Some(3.5), "neighbouring values are unaffected");
    assert_eq!(line.name(), Some("Line A"));
}

#[test]
fn xml_special_characters_survive_a_round_trip() {
    // Carriage return and tab are the interesting ones: an XML parser normalizes a literal
    // CR in content to LF, and every whitespace character inside an attribute to a space,
    // so a writer that emits them literally loses data no matter how the value is read.
    let nasty = "A & B <tag> \"quoted\" 'single' — ünïcode\r\n\ttabbed";
    let mut ds = Dataset::new(SCHEMA);
    let mut obj = Object::new(classes::ACLineSegment, Mrid::parse(A));
    obj.set(attrs::identified_object::name, Value::Text(nasty.into()));
    ds.insert(obj);

    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf.clone()).unwrap();
    common::assert_well_formed("escaped output", &text);
    // The raw metacharacters must not appear unescaped inside the element.
    assert!(text.contains("A &amp; B &lt;tag&gt;"), "{text}");
    assert!(
        text.contains("&#13;"),
        "carriage return not escaped:\n{text}"
    );
    // Quotes in element content are left as the producer wrote them.
    assert!(text.contains("\"quoted\" 'single'"), "{text}");

    let (back, report) = read(&text);
    assert!(!report.has_errors(), "{report}");
    let line = back
        .view_by_mrid::<views::ACLineSegment>(&Mrid::parse(A))
        .unwrap();
    assert_eq!(line.name(), Some(nasty));
}

#[test]
fn a_non_conforming_identifier_containing_whitespace_survives_the_attribute() {
    // Attribute-value normalization would turn these into spaces, silently changing the
    // identifier, so the writer escapes them numerically.
    let weird = "id\twith\nwhitespace";
    let mut ds = Dataset::new(SCHEMA);
    ds.insert(Object::new(classes::ACLineSegment, Mrid::parse(weird)));

    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf).unwrap();
    common::assert_well_formed("identifier output", &text);

    let (back, _) = read(&text);
    assert!(
        back.by_mrid(&Mrid::parse(weird)).is_some(),
        "identifier changed on the way through the attribute:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Well-formedness
//
// The crate's own reader is deliberately tolerant, so re-reading what the writer produced
// says only that this crate can read it back. Every document must also satisfy the XML and
// XML Namespaces recommendations, or no other tool in the CGMES ecosystem can open it at
// all — and for a long time none could: the schema's namespace table already contains the
// model description namespace, and the header wrote `xmlns:md` a second time.
// ---------------------------------------------------------------------------

/// Everything the writer can emit, in one document.
fn kitchen_sink() -> (Dataset, cim_rs::header::ModelHeader) {
    let src = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:eu="http://iec.ch/TC57/CIM100-European#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:dcat="http://www.w3.org/ns/dcat#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.created>2021-03-25T23:16:27Z</md:Model.created>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/GeographicalLocation-EU/3.0</md:Model.profile>
    <md:Model.DependentOn rdf:resource="urn:uuid:44444444-4444-4444-8444-444444444444"/>
    <dcat:landingPage rdf:resource="https://example.invalid/model"/>
  </md:FullModel>
  <cim:Location rdf:ID="_22222222-2222-4222-8222-222222222222">
    <cim:IdentifiedObject.name>Loc &amp; Co</cim:IdentifiedObject.name>
    <cim:Location.mainAddress rdf:parseType="Resource">
      <cim:StreetAddress.townDetail rdf:parseType="Resource">
        <cim:TownDetail.name>Brussels</cim:TownDetail.name>
      </cim:StreetAddress.townDetail>
    </cim:Location.mainAddress>
  </cim:Location>
  <cim:Terminal rdf:ID="_33333333-3333-4333-8333-333333333333">
    <cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
</rdf:RDF>"##;
    let (ds, report) = read(src);
    assert!(!report.has_errors(), "{report}");
    let header = ds.headers()[0].clone();
    (ds, header)
}

#[test]
fn every_document_the_writer_produces_is_well_formed_xml() {
    let (ds, header) = kitchen_sink();
    let render = |options: &WriteOptions| {
        let mut buf = Vec::new();
        cim_rs::writer::write(&ds, &mut buf, options).unwrap();
        String::from_utf8(buf).unwrap()
    };

    for (label, options) in [
        ("no header", WriteOptions::default()),
        (
            "with header",
            WriteOptions::default().with_header(header.clone()),
        ),
        (
            "compact",
            WriteOptions::default()
                .with_header(header.clone())
                .compact(),
        ),
        (
            "profile-filtered",
            WriteOptions::profiles(SCHEMA.profile_by_keyword("GL").unwrap().mask())
                .with_header(header.clone()),
        ),
    ] {
        let text = render(&options);
        common::assert_well_formed(label, &text);
        // A prefix declared twice is what an XML parser rejects; assert it directly so a
        // failure names the cause rather than only the symptom.
        assert_eq!(
            text.matches("xmlns:md=").count(),
            1,
            "{label}: xmlns:md declared more than once\n{text}"
        );
    }
}

#[test]
fn a_difference_document_is_well_formed_xml() {
    let diff = cim_rs::reader::read_difference(SCHEMA, DIFFERENCE.as_bytes(), Some("diff.xml"))
        .unwrap()
        .unwrap();
    let mut buf = Vec::new();
    cim_rs::writer::write_difference(SCHEMA, &diff, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf).unwrap();
    common::assert_well_formed("difference", &text);
    assert_eq!(text.matches("xmlns:dm=").count(), 1, "{text}");
    assert_eq!(text.matches("xmlns:md=").count(), 1, "{text}");
}

#[test]
fn the_rdf_namespace_is_never_declared_under_a_second_prefix() {
    // The CGMES 3.0 vocabulary binds the RDF namespace under a generated prefix of its
    // own. Emitting that alongside `rdf` would leave two prefixes for one namespace and,
    // worse, make `rdf` itself a candidate for renaming.
    let rdf_ns = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    assert!(
        SCHEMA.namespaces.iter().any(|n| n.iri == rdf_ns),
        "this test is only meaningful while the schema carries the RDF namespace"
    );
    let (ds, header) = kitchen_sink();
    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default().with_header(header)).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert_eq!(
        text.matches(rdf_ns).count(),
        1,
        "the RDF namespace appears more than once:\n{text}"
    );
    assert!(text.contains(&format!(r#"xmlns:rdf="{rdf_ns}""#)), "{text}");
}

/// The obvious call has to produce a document another implementation will take.
///
/// IEC 61970-552 gives every instance file an `md:FullModel`, and until this was pinned
/// the writer emitted none unless the caller built one — while `examples/build_model.rs`,
/// the example CI runs, said in as many words that "a conforming one is derived". The
/// derivation existed one layer up in [`Dataset::save_profile`], so both statements were
/// true of *something* and the example shipped headerless output.
#[test]
fn a_document_written_with_default_options_carries_a_derived_header() {
    let (ds, _) = kitchen_sink();
    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();

    let mut buf = Vec::new();
    cim_rs::writer::write_profile(&ds, eq, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf).unwrap();
    common::assert_well_formed("derived header", &text);

    assert!(text.contains("<md:FullModel"), "no header written:\n{text}");
    // It names the profile actually written, because that is what a consumer reads to
    // decide what the file is.
    assert!(
        text.contains(SCHEMA.profile(eq).version_iri),
        "the header does not declare the profile it serves:\n{text}"
    );
    // Deterministic and clock-free: the same model exports to the same header twice.
    let mut again = Vec::new();
    cim_rs::writer::write_profile(&ds, eq, &mut again, &WriteOptions::default()).unwrap();
    assert_eq!(text, String::from_utf8(again).unwrap());

    // And it is genuinely optional, rather than merely absent by default.
    let mut none = Vec::new();
    cim_rs::writer::write_profile(&ds, eq, &mut none, &WriteOptions::default().headerless())
        .unwrap();
    assert!(!String::from_utf8(none).unwrap().contains("FullModel"));
}

#[test]
fn a_header_property_with_no_prefix_is_given_one() {
    // A producer that binds its vocabulary as the *default* namespace leaves the property
    // with no prefix at all, and `<:Model.x>` is not a name.
    let src = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns="https://example.invalid/vendor#"
         xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <exportSettings>lossless</exportSettings>
  </md:FullModel>
</rdf:RDF>"##;
    let (ds, _) = read(src);
    let h = ds.headers()[0].clone();
    assert_eq!(h.extra.len(), 1, "{:?}", h.extra);
    assert_eq!(h.extra[0].prefix, "");

    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default().with_header(h)).unwrap();
    let text = String::from_utf8(buf).unwrap();
    common::assert_well_formed("default-namespace header property", &text);
    assert!(!text.contains("<:"), "{text}");
    // And the property still means the same thing after the round trip.
    let (again, _) = read(&text);
    let back = &again.headers()[0].extra[0];
    assert_eq!(back.ns, "https://example.invalid/vendor#");
    assert_eq!(back.local, "exportSettings");
}

#[test]
fn writing_a_profile_emits_only_that_profile_and_reparses() {
    let src = doc(&format!(
        r##"{}
  <cim:Terminal rdf:ID="_{A}">
    <cim:IdentifiedObject.name>T1</cim:IdentifiedObject.name>
  </cim:Terminal>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, _) = read(&src);

    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let ssh = SCHEMA.profile_by_keyword("SSH").unwrap();

    let render = |p| {
        let mut buf = Vec::new();
        cim_rs::writer::write_profile(&ds, p, &mut buf, &WriteOptions::default()).unwrap();
        String::from_utf8(buf).unwrap()
    };

    let eq_out = render(eq);
    assert!(eq_out.contains("IdentifiedObject.name"), "{eq_out}");

    // The name came from an Equipment file, so a Steady State Hypothesis export must not
    // repeat it, even though SSH's vocabulary also declares the attribute.
    let ssh_out = render(ssh);
    assert!(
        !ssh_out.contains("IdentifiedObject.name"),
        "EQ-sourced data leaked into the SSH export:\n{ssh_out}"
    );
}

#[test]
fn many_valued_attributes_keep_every_occurrence() {
    // `Model.profile` aside, associations such as `PositionPoint` repeat legitimately.
    let src = doc(&format!(
        r##"{}
  <cim:Diagram rdf:ID="_{A}">
    <cim:IdentifiedObject.name>D</cim:IdentifiedObject.name>
  </cim:Diagram>
  <cim:DiagramObject rdf:ID="_{B}">
    <cim:DiagramObject.Diagram rdf:resource="#_{A}"/>
  </cim:DiagramObject>"##,
        header("http://iec.ch/TC57/ns/CIM/DiagramLayout-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert!(report.is_empty(), "{report}");

    // The inverse side is derived, not stored, so it is found by scanning.
    let diagram_ref = SCHEMA.find_attr(NS, "DiagramObject.Diagram").unwrap();
    let target = Mrid::parse(A);
    let referrers: Vec<_> = ds.referrers(&target, diagram_ref).collect();
    assert_eq!(referrers.len(), 1);

    // The prebuilt index gives the same answer.
    let index = cim_rs::InverseIndex::build(&ds);
    assert_eq!(index.referrers(diagram_ref, &target).len(), 1);
}

#[test]
fn a_document_without_an_rdf_root_is_rejected() {
    let mut ds = Dataset::new(SCHEMA);
    let err = read_into(
        &mut ds,
        b"<?xml version=\"1.0\"?><html><body>not cim</body></html>".as_slice(),
        None,
        &ReadOptions::lenient(),
    )
    .unwrap_err();
    assert!(matches!(err, cim_rs::Error::NotCimXml(_)), "{err:?}");
}

#[test]
fn malformed_xml_is_an_error_not_a_panic() {
    let mut ds = Dataset::new(SCHEMA);
    let err = read_into(
        &mut ds,
        b"<rdf:RDF><unclosed".as_slice(),
        None,
        &ReadOptions::lenient(),
    )
    .unwrap_err();
    assert!(matches!(err, cim_rs::Error::Xml(_)), "{err:?}");
}

#[test]
fn non_conforming_identifiers_are_preserved_and_flagged() {
    let src = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="LINE-1">
    <cim:IdentifiedObject.name>Legacy</cim:IdentifiedObject.name>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let options = ReadOptions {
        strictness: Strictness::Lenient,
        report_non_conforming_mrids: true,
        ..Default::default()
    };
    let (ds, report) = read_opts(&src, &options);

    assert_eq!(
        report.by_rule(Rule::NonConformingMrid).count(),
        1,
        "{report}"
    );
    // The identifier is kept verbatim so the file can be written back unchanged.
    let obj = ds.by_mrid(&Mrid::parse("LINE-1")).expect("object present");
    assert!(!obj.mrid().is_uuid());
    assert_eq!(obj.mrid().canonical(), "LINE-1");
}

#[test]
fn validation_finds_dangling_references_and_wrong_targets() {
    let src = doc(&format!(
        r##"{}
  <cim:Terminal rdf:ID="_{A}">
    <cim:IdentifiedObject.name>T1</cim:IdentifiedObject.name>
    <cim:Terminal.ConductingEquipment rdf:resource="#_44444444-4444-4444-8444-444444444444"/>
  </cim:Terminal>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, _) = read(&src);

    let report = cim_rs::validate::validate(&ds);
    assert_eq!(
        report.by_rule(Rule::DanglingReference).count(),
        1,
        "{report}"
    );

    // Strict link policy turns the same condition into a hard error.
    let strict = Dataset::new(SCHEMA).with_link_policy(LinkPolicy::Strict);
    assert_eq!(strict.link_policy(), LinkPolicy::Strict);
    let mut strict = strict;
    read_into(&mut strict, src.as_bytes(), None, &ReadOptions::lenient()).unwrap();
    assert!(strict.check_links().is_err());
}

#[test]
fn schema_lookups_are_consistent() {
    // Every index entry must resolve back to the definition it names.
    for (i, c) in SCHEMA.classes.iter().enumerate() {
        let ns = SCHEMA.namespace(c.ns).iri;
        assert_eq!(
            SCHEMA.find_class(ns, c.name),
            Some(ClassId(i as u16)),
            "class {} not findable",
            c.name
        );
    }
    for (i, a) in SCHEMA.attributes.iter().enumerate() {
        let ns = SCHEMA.namespace(a.ns).iri;
        assert_eq!(
            SCHEMA.find_attr(ns, a.name),
            Some(AttrId(i as u16)),
            "attribute {} not findable",
            a.name
        );
        // Every attribute is reachable from the class that declares it.
        assert!(
            SCHEMA.class(a.owner).all_attrs.contains(&AttrId(i as u16)),
            "{} missing from {}",
            a.name,
            SCHEMA.class(a.owner).name
        );
    }
    for (i, v) in SCHEMA.enum_values.iter().enumerate() {
        let ns = SCHEMA.namespace(v.ns).iri;
        assert_eq!(
            SCHEMA.find_enum_value(ns, v.name),
            Some(cim_rs::schema::EnumValueId(i as u16)),
            "enum literal {} not findable",
            v.name
        );
    }
}

#[test]
fn inheritance_is_consistent() {
    for (i, c) in SCHEMA.classes.iter().enumerate() {
        let id = ClassId(i as u16);
        assert!(SCHEMA.is_a(id, id), "a class is itself");
        // Ancestors form a chain ending at a root, with no cycles.
        assert!(
            !c.ancestors.contains(&id),
            "{} appears in its own ancestors",
            c.name
        );
        if let Some(p) = c.parent {
            assert_eq!(c.ancestors.first(), Some(&p), "{}", c.name);
            assert!(
                SCHEMA.is_a(id, p),
                "{} is not a {}",
                c.name,
                SCHEMA.class(p).name
            );
        }
        // Inherited attributes really are the union up the chain.
        let own = c.own_attrs.len();
        let inherited: usize = c
            .ancestors
            .iter()
            .map(|a| SCHEMA.class(*a).own_attrs.len())
            .sum();
        assert_eq!(
            c.all_attrs.len(),
            own + inherited,
            "{} attribute closure is wrong",
            c.name
        );
    }
}

#[cfg(feature = "zip")]
#[test]
fn a_model_round_trips_through_a_zip_archive() {
    let src = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>Line A</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>1.25</cim:ACLineSegment.r>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, _) = read(&src);

    let dir = std::env::temp_dir().join(format!("cim-zip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let archive = dir.join("model.zip");

    let names = ds.save_zip(&archive, "Model").unwrap();
    assert!(!names.is_empty(), "archive contains no instance files");

    let mut back = Dataset::new(SCHEMA);
    let report = back.load_file(&archive, &ReadOptions::lenient()).unwrap();
    assert!(!report.report.has_errors(), "{}", report.report);
    assert_eq!(back.len(), ds.len());

    let line = back
        .view_by_mrid::<views::ACLineSegment>(&Mrid::parse(A))
        .expect("object survived the archive");
    assert_eq!(line.name(), Some("Line A"));
    assert_eq!(line.r(), Some(1.25));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn self_closing_object_elements_are_not_dropped() {
    // An object with no properties is legal and is written as `<cim:Class rdf:ID="…"/>`.
    // It must be stored, and must not disturb the object that follows it.
    let src = doc(&format!(
        r##"{}
  <cim:Terminal rdf:ID="_{A}"/>
  <cim:Breaker rdf:ID="_{B}">
    <cim:IdentifiedObject.name>BRK</cim:IdentifiedObject.name>
  </cim:Breaker>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert!(!report.has_errors(), "{report}");
    assert_eq!(ds.len(), 2, "the empty object was dropped");

    let t = ds.by_mrid(&Mrid::parse(A)).expect("empty object stored");
    assert_eq!(SCHEMA.class(t.class()).name, "Terminal");
    assert!(t.is_empty());

    // The following object kept its own class and data.
    let b = ds.view_by_mrid::<views::Breaker>(&Mrid::parse(B)).unwrap();
    assert_eq!(b.name(), Some("BRK"));
}

#[test]
fn conflicting_values_across_profiles_keep_both_files_populated() {
    // Two profiles disagreeing on a single-valued attribute is a data error, but the
    // export must still write the attribute into both files rather than silently
    // emptying one of them.
    let eq = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>From EQ</cim:IdentifiedObject.name>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let tp = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:about="#_{A}">
    <cim:IdentifiedObject.name>From TP</cim:IdentifiedObject.name>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/Topology-EU/3.0")
    ));

    let mut ds = Dataset::new(SCHEMA);
    for (src, name) in [(&eq, "EQ.xml"), (&tp, "TP.xml")] {
        let outcome =
            read_into(&mut ds, src.as_bytes(), Some(name), &ReadOptions::lenient()).unwrap();
        if let Some(h) = outcome.header {
            ds.push_header(h);
        }
    }
    assert_eq!(ds.len(), 1);

    let render = |kw: &str| {
        let p = SCHEMA.profile_by_keyword(kw).unwrap();
        let mut buf = Vec::new();
        cim_rs::writer::write_profile(&ds, p, &mut buf, &WriteOptions::default()).unwrap();
        String::from_utf8(buf).unwrap()
    };
    for kw in ["EQ", "TP"] {
        assert!(
            render(kw).contains("IdentifiedObject.name"),
            "the {kw} export lost the attribute entirely"
        );
    }
}

#[test]
fn one_identifier_naming_two_unrelated_classes_is_an_error() {
    // Two files describing the same object is how profiles compose and must be silent.
    // Two files giving one identifier to unrelated classes is a defect: one silently
    // absorbs the other.
    let src = doc(&format!(
        r##"{}
  <cim:LinearShuntCompensator rdf:ID="_{A}">
    <cim:IdentifiedObject.name>S</cim:IdentifiedObject.name>
  </cim:LinearShuntCompensator>
  <cim:NonlinearShuntCompensator rdf:about="#_{A}">
    <cim:ShuntCompensator.nomU>380</cim:ShuntCompensator.nomU>
  </cim:NonlinearShuntCompensator>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (_, report) = read(&src);
    assert_eq!(report.by_rule(Rule::DuplicateMrid).count(), 1, "{report}");
    assert!(report.has_errors());
}

#[test]
fn a_subclass_refining_a_base_class_is_not_a_conflict() {
    // A profile may describe an object through a base class; the more specific class
    // wins and no diagnostic is raised.
    let src = doc(&format!(
        r##"{}
  <cim:ConductingEquipment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>E</cim:IdentifiedObject.name>
  </cim:ConductingEquipment>
  <cim:Breaker rdf:about="#_{A}">
    <cim:Switch.normalOpen>true</cim:Switch.normalOpen>
  </cim:Breaker>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let (ds, report) = read(&src);
    assert_eq!(report.by_rule(Rule::DuplicateMrid).count(), 0, "{report}");

    let obj = ds.by_mrid(&Mrid::parse(A)).unwrap();
    assert_eq!(
        SCHEMA.class(obj.class()).name,
        "Breaker",
        "the more specific class must win"
    );
}

#[test]
fn a_difference_can_change_an_objects_class() {
    let base = doc(&format!(
        r##"{}
  <cim:LinearShuntCompensator rdf:ID="_{A}">
    <cim:IdentifiedObject.name>S</cim:IdentifiedObject.name>
    <cim:LinearShuntCompensator.bPerSection>0.000346</cim:LinearShuntCompensator.bPerSection>
  </cim:LinearShuntCompensator>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0")
    ));
    let diff_doc = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}" xmlns:dm="http://iec.ch/TC57/61970-552/DifferenceModel/1#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <dm:DifferenceModel rdf:about="urn:uuid:33333333-3333-4333-8333-333333333333">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <dm:reverseDifferences rdf:parseType="Statements">
      <cim:LinearShuntCompensator rdf:ID="_{A}">
        <cim:LinearShuntCompensator.bPerSection>0.000346</cim:LinearShuntCompensator.bPerSection>
      </cim:LinearShuntCompensator>
    </dm:reverseDifferences>
    <dm:forwardDifferences rdf:parseType="Statements">
      <cim:NonlinearShuntCompensator rdf:ID="_{A}">
        <cim:ShuntCompensator.nomU>380</cim:ShuntCompensator.nomU>
      </cim:NonlinearShuntCompensator>
    </dm:forwardDifferences>
  </dm:DifferenceModel>
</rdf:RDF>"##
    );

    let (mut ds, _) = read(&base);
    let subject = Mrid::parse(A);
    assert_eq!(
        SCHEMA.class(ds.by_mrid(&subject).unwrap().class()).name,
        "LinearShuntCompensator"
    );

    let diff = cim_rs::reader::read_difference(SCHEMA, diff_doc.as_bytes(), Some("diff.xml"))
        .unwrap()
        .expect("a difference model");
    // The statement group names its class, which is what makes reclassification possible.
    assert!(diff.forward.iter().all(|s| s.class.is_some()));

    let report = ds.apply_difference(&diff);
    assert!(!report.has_errors(), "{report}");

    let obj = ds.by_mrid(&subject).unwrap();
    assert_eq!(
        SCHEMA.class(obj.class()).name,
        "NonlinearShuntCompensator",
        "the difference did not reclassify the object"
    );
    // The old class's exclusive attribute was retracted; the new one's was asserted.
    let b_per_section = SCHEMA
        .find_attr(NS, "LinearShuntCompensator.bPerSection")
        .unwrap();
    assert!(!obj.has(b_per_section), "stale attribute survived");
    assert!(obj.has(SCHEMA.find_attr(NS, "ShuntCompensator.nomU").unwrap()));
    // Reclassification is reported so it is never silent.
    assert!(
        report.iter().any(|d| d.message.contains("reclassifies")),
        "{report}"
    );
}

#[test]
fn an_enum_literal_in_the_wrong_namespace_is_recovered_with_a_warning() {
    // Published models do misplace these: a CGMES 2.4.15 test model writes the ENTSO-E
    // extension literal `LimitTypeKind.patl` under the `cim` namespace. The qualified
    // name is unambiguous, so the value must be recovered rather than dropped.
    let eu_attr = SCHEMA
        .find_attr(EU, "OperationalLimitType.kind")
        .expect("the extension attribute exists");
    let correct = SCHEMA
        .find_enum_value(EU, "LimitKind.patl")
        .expect("the extension literal exists");

    let src = doc(&format!(
        r##"{}
  <cim:OperationalLimitType rdf:ID="_{A}">
    <cim:IdentifiedObject.name>PATL</cim:IdentifiedObject.name>
    <eu:OperationalLimitType.kind rdf:resource="{NS}LimitKind.patl"/>
  </cim:OperationalLimitType>"##,
        header("http://iec.ch/TC57/ns/CIM/Operation-EU/3.0")
    ));
    let (ds, report) = read(&src);

    let obj = ds.by_mrid(&Mrid::parse(A)).expect("object read");
    assert_eq!(
        obj.get(eu_attr).and_then(|v| v.as_enum()),
        Some(correct),
        "the value was dropped instead of recovered"
    );

    let recovered: Vec<_> = report.by_rule(Rule::InvalidValue).collect();
    assert_eq!(recovered.len(), 1, "{report}");
    assert_eq!(recovered[0].severity, Severity::Warning);
    assert!(recovered[0].message.contains("recovered"), "{report}");
    assert!(
        !report.has_errors(),
        "a recoverable mistake is not an error"
    );
}

#[test]
fn a_genuinely_unknown_enum_literal_is_still_an_error() {
    let src = doc(&format!(
        r##"{}
  <cim:OperationalLimitType rdf:ID="_{A}">
    <eu:OperationalLimitType.kind rdf:resource="{NS}LimitKind.doesNotExist"/>
  </cim:OperationalLimitType>"##,
        header("http://iec.ch/TC57/ns/CIM/Operation-EU/3.0")
    ));
    let (_, report) = read(&src);
    assert!(report.has_errors(), "{report}");
    assert_eq!(report.by_rule(Rule::InvalidValue).count(), 1);
}

// ---------------------------------------------------------------------------
// Compound values (IEC 61970-552 `rdf:parseType="Resource"`)
// ---------------------------------------------------------------------------

/// A postal address in the Geographical Location profile: a compound holding compounds.
const LOCATION_WITH_ADDRESS: &str = r##"  <cim:Location rdf:ID="_44444444-4444-4444-8444-444444444444">
    <cim:IdentifiedObject.name>Substation site</cim:IdentifiedObject.name>
    <cim:Location.mainAddress rdf:parseType="Resource">
      <cim:StreetAddress.postalCode>1000</cim:StreetAddress.postalCode>
      <cim:StreetAddress.townDetail rdf:parseType="Resource">
        <cim:TownDetail.name>Brussels</cim:TownDetail.name>
        <cim:TownDetail.country>BE</cim:TownDetail.country>
      </cim:StreetAddress.townDetail>
    </cim:Location.mainAddress>
  </cim:Location>"##;

#[test]
fn compound_values_are_read_as_structure_not_flattened_text() {
    // A compound has no identity, so it is written inside the property that holds it.
    // Treating the nested elements as text silently fabricates a value.
    let (ds, report) = read(&doc(&format!(
        "{}\n{LOCATION_WITH_ADDRESS}",
        header("http://iec.ch/TC57/ns/CIM/GeographicalLocation-EU/3.0")
    )));
    assert!(!report.has_errors(), "{report}");

    let loc = ds.view::<views::Location>().next().expect("no Location");
    let address = loc.main_address().expect("compound not read");
    assert_eq!(address.postal_code(), Some("1000"));

    let town = address.town_detail().expect("nested compound not read");
    assert_eq!(town.name(), Some("Brussels"));
    assert_eq!(town.country(), Some("BE"));
}

#[test]
fn compound_values_round_trip_through_the_writer() {
    let source = doc(&format!(
        "{}\n{LOCATION_WITH_ADDRESS}",
        header("http://iec.ch/TC57/ns/CIM/GeographicalLocation-EU/3.0")
    ));
    let (ds, _) = read(&source);

    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(
        text.contains(r#"rdf:parseType="Resource""#),
        "compounds must be written inline:\n{text}"
    );

    // And the structure survives a strict re-read.
    let mut back = Dataset::new(SCHEMA);
    let outcome = read_into(
        &mut back,
        text.as_bytes(),
        Some("out.xml"),
        &ReadOptions::strict(),
    )
    .unwrap();
    assert!(!outcome.report.has_errors(), "{}", outcome.report);
    let town = back
        .view::<views::Location>()
        .next()
        .unwrap()
        .main_address()
        .unwrap()
        .town_detail()
        .unwrap();
    assert_eq!(town.name(), Some("Brussels"));
}

#[test]
fn a_compound_cannot_be_a_top_level_object() {
    // Compounds have no mRID; an element claiming otherwise is a structural error rather
    // than an unknown class.
    let (_, report) = read(&doc(
        r##"  <cim:StreetAddress rdf:ID="_55555555-5555-4555-8555-555555555555">
    <cim:StreetAddress.postalCode>1000</cim:StreetAddress.postalCode>
  </cim:StreetAddress>"##,
    ));
    assert!(
        report
            .by_rule(Rule::Structure)
            .any(|d| d.message.contains("no identity")),
        "{report}"
    );
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

#[test]
fn header_properties_outside_the_model_vocabulary_survive_a_round_trip() {
    // CGMES 3.0 headers may carry W3C DCAT properties, and producers add their own.
    // Re-emitting one as `md:` element text would change an IRI into a literal.
    let src = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:dcat="http://www.w3.org/ns/dcat#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.created>2021-03-25T23:16:27Z</md:Model.created>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <dcat:keyword>EQ</dcat:keyword>
    <dcat:landingPage rdf:resource="https://example.invalid/model"/>
  </md:FullModel>
</rdf:RDF>"##;

    let (ds, _) = read(src);
    let h = &ds.headers()[0];
    assert_eq!(h.extra.len(), 2, "{:?}", h.extra);
    assert_eq!(h.extra[0].prefix, "dcat");
    assert_eq!(
        h.extra[0].value,
        cim_rs::header::HeaderValue::Text("EQ".to_owned())
    );
    assert_eq!(
        h.extra[1].value,
        cim_rs::header::HeaderValue::Resource("https://example.invalid/model".to_owned())
    );

    let mut buf = Vec::new();
    cim_rs::writer::write(
        &ds,
        &mut buf,
        &WriteOptions::default().with_header(h.clone()),
    )
    .unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(
        text.contains(r#"xmlns:dcat="http://www.w3.org/ns/dcat#""#),
        "{text}"
    );
    assert!(text.contains("<dcat:keyword>EQ</dcat:keyword>"), "{text}");
    assert!(
        text.contains(r#"<dcat:landingPage rdf:resource="https://example.invalid/model"/>"#),
        "{text}"
    );

    // And re-reading yields the same header.
    let (again, _) = read(&text);
    assert_eq!(again.headers()[0].extra, h.extra);
}

// ---------------------------------------------------------------------------
// Difference models
// ---------------------------------------------------------------------------

const DIFFERENCE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:dm="http://iec.ch/TC57/61970-552/DifferenceModel/1#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <dm:DifferenceModel rdf:about="urn:uuid:99999999-9999-4999-8999-999999999999">
    <md:Model.created>2021-11-19T23:16:27Z</md:Model.created>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
    <dm:reverseDifferences rdf:parseType="Statements">
      <rdf:Description rdf:about="#_22222222-2222-4222-8222-222222222222">
        <cim:IdentifiedObject.name>Line A</cim:IdentifiedObject.name>
      </rdf:Description>
    </dm:reverseDifferences>
    <dm:forwardDifferences rdf:parseType="Statements">
      <rdf:Description rdf:about="#_22222222-2222-4222-8222-222222222222">
        <cim:IdentifiedObject.name>Line A renamed</cim:IdentifiedObject.name>
      </rdf:Description>
      <cim:Breaker rdf:ID="_66666666-6666-4666-8666-666666666666">
        <cim:IdentifiedObject.name>New breaker</cim:IdentifiedObject.name>
      </cim:Breaker>
    </dm:forwardDifferences>
  </dm:DifferenceModel>
</rdf:RDF>"##;

#[test]
fn a_difference_model_round_trips_through_the_writer() {
    let diff = cim_rs::reader::read_difference(SCHEMA, DIFFERENCE.as_bytes(), Some("diff.xml"))
        .unwrap()
        .expect("no difference read");
    assert_eq!(diff.reverse.len(), 1);
    assert_eq!(diff.forward.len(), 2);
    // The statement group that introduces the breaker used `rdf:ID`; the one that renames
    // an existing object used `rdf:about`. Both forms have to survive.
    assert!(!diff.forward[0].defines_subject);
    assert!(diff.forward[1].defines_subject);

    let mut buf = Vec::new();
    cim_rs::writer::write_difference(SCHEMA, &diff, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(text.contains("dm:DifferenceModel"), "{text}");
    assert!(text.contains(r#"<cim:Breaker rdf:ID="_66666666"#), "{text}");
    assert!(
        text.contains(r##"<rdf:Description rdf:about="#_22222222"##),
        "{text}"
    );

    let again = cim_rs::reader::read_difference(SCHEMA, text.as_bytes(), Some("again.xml"))
        .unwrap()
        .expect("re-read produced no difference");
    assert_eq!(again.reverse, diff.reverse);
    assert_eq!(again.forward, diff.forward);
    assert_eq!(again.header.id, diff.header.id);
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[test]
fn diagnostics_carry_a_position_that_maps_back_to_a_line() {
    let src = doc(&format!(
        r##"  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:ACLineSegment.r>not a number</cim:ACLineSegment.r>
  </cim:ACLineSegment>"##
    ));
    let (_, report) = read(&src);
    let d = report
        .by_rule(Rule::InvalidValue)
        .next()
        .expect("no InvalidValue diagnostic");
    let pos = d.position.expect("no position recorded");
    let (line, _col) = cim_rs::error::line_and_column(&src, pos);
    assert!(
        src.lines()
            .nth(line as usize - 1)
            .unwrap()
            .contains("not a number"),
        "position {pos} mapped to line {line}, which is not the offending one"
    );
    // The rendered form makes the offset visible without pretending it is a line.
    assert!(d.to_string().contains(&format!("test.xml@{pos}")), "{d}");
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn a_value_of_the_wrong_shape_is_reported_against_the_schema() {
    // CIM/XML exchanges no datatype information, so only the profile can catch this.
    let mut ds = Dataset::new(SCHEMA);
    let mut obj = cim_rs::Object::new(classes::ACLineSegment, Mrid::parse(A));
    obj.set(attrs::ac_line_segment::r, Value::Text("oops".into()));
    obj.set(attrs::identified_object::name, Value::Integer(7));
    ds.insert(obj);

    let report = ds.validate_with(&cim_rs::validate::ValidateOptions::essential());
    let found: Vec<&str> = report
        .by_rule(Rule::DatatypeMismatch)
        .map(|d| d.attribute.unwrap_or(""))
        .collect();
    assert!(found.contains(&"ACLineSegment.r"), "{report}");
    assert!(found.contains(&"IdentifiedObject.name"), "{report}");
}

#[test]
fn an_object_known_only_through_a_base_class_is_not_a_wrong_reference_target() {
    // A Steady State Hypothesis file writes `<cim:Equipment rdf:about="…">` for equipment
    // the Equipment file defines. Until that file is loaded the object *is* an Equipment,
    // which is not evidence that a reference to it points at the wrong kind of thing.
    let (ds, _) = read(&doc(&format!(
        r##"  <cim:Equipment rdf:about="#_{A}">
    <cim:Equipment.inService>true</cim:Equipment.inService>
  </cim:Equipment>
  <cim:SvStatus rdf:ID="_{B}">
    <cim:SvStatus.inService>true</cim:SvStatus.inService>
    <cim:SvStatus.ConductingEquipment rdf:resource="#_{A}"/>
  </cim:SvStatus>"##
    )));
    let report = ds.validate_with(&cim_rs::validate::ValidateOptions::essential());
    assert_eq!(
        report.by_rule(Rule::WrongReferenceTarget).count(),
        0,
        "{report}"
    );
}

/// A header with no properties is still the file's header.
///
/// `<md:FullModel rdf:about="…"/>` is legal XML and degenerate CGMES, and the reader
/// registered a header only when it saw the *closing* tag — which a self-closing element
/// never produces. The document's real header was returned to the caller while the dataset
/// got a blank one in its place, so every object of that file lost its source slot and
/// `save_as_loaded` filed them under whichever header came first.
#[test]
fn a_self_closing_header_is_still_registered_with_the_dataset() {
    let src = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111"/>
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>L</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
</rdf:RDF>"##
    );

    let mut ds = Dataset::new(SCHEMA);
    let outcome = read_into(
        &mut ds,
        src.as_bytes(),
        Some("t.xml"),
        &ReadOptions::lenient(),
    )
    .unwrap();

    let returned = outcome.header.expect("the document has a header");
    assert_eq!(
        returned.id.as_ref().map(Mrid::canonical).as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );

    // Exactly one header slot, and it is the document's own — not a blank stand-in.
    assert_eq!(ds.headers().len(), 1, "{:?}", ds.headers());
    assert_eq!(ds.headers()[0].id, returned.id);
    assert_eq!(ds.headers()[0].source.as_deref(), Some("t.xml"));

    // …and the object is recorded against it, which is what an export depends on.
    let from_file: Vec<_> = ds.objects_from(0).collect();
    assert_eq!(from_file.len(), 1);
    assert_eq!(
        ds.get(from_file[0]).unwrap().mrid().canonical(),
        A,
        "the object was not attributed to the file it came from"
    );
}

// ---------------------------------------------------------------------------
// What the output syntaxes can represent
//
// Two constraints the object model cannot express, so nothing upstream of serialization
// enforces them and no corpus of conforming files can demonstrate them.
// ---------------------------------------------------------------------------

/// A character XML cannot represent is refused where it enters, not discovered on export.
///
/// Reachable from an ordinary file: `quick-xml` does not enforce XML 1.0's `Char`
/// production, so a mis-encoded or corrupted source document delivers a raw control
/// character to the reader without complaint.
#[test]
fn a_character_xml_cannot_represent_is_stripped_and_reported_on_read() {
    let src = doc(&format!(
        r##"{}
  <cim:ACLineSegment rdf:ID="_{A}">
    <cim:IdentifiedObject.name>bad{}name</cim:IdentifiedObject.name>
    <cim:IdentifiedObject.description><![CDATA[cdata{}here]]></cim:IdentifiedObject.description>
  </cim:ACLineSegment>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0"),
        '\u{0}',
        '\u{1}',
    ));
    let (ds, report) = read(&src);

    // Reported, once per value, naming the character rather than embedding it.
    let found: Vec<_> = report.by_rule(Rule::IllegalXmlCharacter).collect();
    assert_eq!(found.len(), 2, "{report}");
    assert!(found[0].message.contains("U+0000"), "{}", found[0].message);
    assert!(found[1].message.contains("U+0001"), "{}", found[1].message);

    // The value survives without the character, rather than the object being lost.
    let obj = ds.by_mrid(&Mrid::parse(A)).expect("object kept");
    assert_eq!(
        obj.get(attrs::identified_object::name).unwrap().as_str(),
        Some("badname")
    );
    assert_eq!(
        obj.get(attrs::identified_object::description)
            .unwrap()
            .as_str(),
        Some("cdatahere")
    );

    // And what comes out is a document another parser will take.
    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    common::assert_well_formed("re-export", &String::from_utf8(buf).unwrap());
}

/// The public API is the other way in, so the store is checked and the writer defends
/// itself — the same three-way arrangement as a value whose class does not have it.
#[test]
fn a_value_built_with_an_unwritable_character_is_reported_and_never_written() {
    let mut ds = Dataset::new(SCHEMA);
    let mut o = Object::new(classes::ACLineSegment, Mrid::parse(A));
    o.set(attrs::identified_object::name, "bad\u{0}\u{8}name".into());
    ds.insert(o);

    // `Object::set` cannot know better, so validation says so, with the object and
    // attribute attached rather than only in the message.
    let report = ds.validate();
    let found: Vec<_> = report.by_rule(Rule::IllegalXmlCharacter).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert_eq!(found[0].attribute, Some("IdentifiedObject.name"));
    assert_eq!(found[0].severity, Severity::Error);
    assert!(found[0].message.contains("U+0000"));

    // The writer cannot emit a document nothing can read, whatever it is handed.
    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let out = String::from_utf8(buf).unwrap();
    common::assert_well_formed("export of a hand-built model", &out);
    assert!(out.contains("<cim:IdentifiedObject.name>badname<"), "{out}");
}

/// `rdf:ID` is an XML `NCName`, and an identifier this crate keeps verbatim need not be one.
///
/// This is the defect a well-formedness check cannot see: `rdf:ID="_http://host/EQ.xml#Sub1"`
/// is perfectly well-formed XML and is not valid RDF/XML, so the document passes every
/// check the crate had and is refused by every RDF toolchain.
#[test]
fn an_identifier_that_is_an_iri_is_written_as_rdf_about_rather_than_a_broken_rdf_id() {
    let iri = "http://host/EQ.xml#Sub1";
    let src = doc(&format!(
        r##"{}
  <cim:Substation rdf:about="{iri}">
    <cim:IdentifiedObject.name>Sub</cim:IdentifiedObject.name>
  </cim:Substation>
  <cim:VoltageLevel rdf:ID="_{A}">
    <cim:VoltageLevel.Substation rdf:resource="{iri}"/>
  </cim:VoltageLevel>"##,
        header("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0"),
    ));
    let (ds, _) = read(&src);

    // The reference resolves: both spellings denote the one opaque identifier.
    let sub = Mrid::parse(iri);
    assert_eq!(sub.form_in_xml(), cim_rs::IdentifierForm::Iri);
    assert!(ds.by_mrid(&sub).is_some(), "the substation is in the model");
    assert!(
        ds.validate()
            .by_rule(Rule::DanglingReference)
            .next()
            .is_none()
    );

    let mut buf = Vec::new();
    cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let out = String::from_utf8(buf).unwrap();

    // Written as itself, which is both valid RDF/XML and what the source document said.
    assert!(out.contains(&format!(r#"rdf:about="{iri}""#)), "{out}");
    assert!(out.contains(&format!(r#"rdf:resource="{iri}""#)), "{out}");
    // Never as a name it cannot be, nor as a two-fragment IRI reference.
    assert!(!out.contains("rdf:ID=\"_http"), "{out}");
    assert!(!out.contains("\"#_http"), "{out}");

    // And it still round-trips to the same model.
    let (again, _) = read(&out);
    assert_eq!(again.content_id(), ds.content_id());
}

/// An identifier that is neither a name nor an IRI has no valid form at all. The object is
/// kept — losing it would be worse — and the limit is reported rather than papered over.
#[test]
fn an_identifier_with_no_valid_serialization_is_reported_rather_than_silently_broken() {
    let mut ds = Dataset::new(SCHEMA);
    let odd = Mrid::parse("has space");
    assert_eq!(odd.form_in_xml(), cim_rs::IdentifierForm::Unwritable);
    ds.insert(Object::new(classes::Substation, odd.clone()));

    let report = ds.validate();
    let found: Vec<_> = report.by_rule(Rule::UnserializableIdentifier).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].object.as_ref(), Some(&odd));

    // A conforming model never triggers it: every IEC 61970-552 identifier is a UUID.
    let mut clean = Dataset::new(SCHEMA);
    clean.insert(Object::new(classes::Substation, Mrid::parse(A)));
    assert!(
        clean
            .validate()
            .by_rule(Rule::UnserializableIdentifier)
            .next()
            .is_none()
    );
}

/// A document may name itself after its first object, and it is still one file.
///
/// IEC 61970-552 puts `md:FullModel` first and every published file does, so the reader
/// claims a header slot as soon as an object needs one — that is what gives a *headerless*
/// document's objects a file to be exported back into. The slot has to be filled in rather
/// than duplicated when the header does arrive, or the same file is described twice and its
/// objects hang off the placeholder.
#[test]
fn a_header_that_follows_its_objects_still_describes_the_file() {
    let src = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:ACLineSegment rdf:ID="_22222222-2222-4222-8222-222222222222">
    <cim:IdentifiedObject.name>Line</cim:IdentifiedObject.name>
  </cim:ACLineSegment>
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
  </md:FullModel>
</rdf:RDF>"##;
    let (ds, _) = read(src);

    assert_eq!(ds.headers().len(), 1, "{:?}", ds.headers());
    let h = &ds.headers()[0];
    assert!(h.id.is_some(), "the real header was lost: {h:?}");
    assert_eq!(h.profiles.len(), 1, "{h:?}");
    // And the object belongs to it, which is what an export writes by.
    assert_eq!(ds.objects_from(0).count(), 1);
}

/// A compound is an object in miniature, and its fields are checked like one's.
///
/// The reader refuses a field the compound's class does not declare, so this arrives only
/// through `Compound::set` — the same way an object's stray value does, which is why
/// `class_membership` checks the finished model rather than the paths into it. The
/// consequence differs though, and is worse: an object's stray value is dropped by the
/// writer's schema-ordered walk, while a compound's fields are written exactly as held, so
/// this one leaves in a document that is well-formed XML and not a conforming CGMES file.
#[test]
fn a_compound_field_its_class_does_not_declare_is_reported() {
    use cim_rs::object::Compound;

    let mut ds = Dataset::new(SCHEMA);
    let id = Mrid::parse("22222222-2222-4222-8222-222222222222");
    let mut location = Object::new(cim_rs::cgmes3::classes::Location, id.clone());
    let mut address = Compound::new(cim_rs::cgmes3::classes::StreetAddress);
    address.set(
        cim_rs::cgmes3::attributes::identified_object::name,
        Value::from("not a street"),
    );
    location.set(
        cim_rs::cgmes3::attributes::location::mainAddress,
        Value::from(address),
    );
    ds.insert(location);

    let found: Vec<String> = ds
        .validate()
        .iter()
        .filter(|d| d.rule == cim_rs::Rule::UnknownAttribute)
        .map(|d| d.to_string())
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("StreetAddress compound"), "{found:?}");

    // A field the class *does* declare is not reported, at any depth.
    let mut ds = Dataset::new(SCHEMA);
    let mut location = Object::new(cim_rs::cgmes3::classes::Location, id);
    let mut address = Compound::new(cim_rs::cgmes3::classes::StreetAddress);
    address.set(
        cim_rs::cgmes3::attributes::street_address::postalCode,
        Value::from("1000"),
    );
    location.set(
        cim_rs::cgmes3::attributes::location::mainAddress,
        Value::from(address),
    );
    ds.insert(location);
    assert!(
        !ds.validate()
            .iter()
            .any(|d| d.rule == cim_rs::Rule::UnknownAttribute),
        "a conforming compound was reported"
    );
}
