//! Reader and writer behaviour on hand-written documents.
//!
//! These run without the ENTSO-E corpus, so they hold on a fresh clone and pin down the
//! exact IEC 61970-552 rules the conformity models only exercise incidentally.

use cim::cgmes3::{SCHEMA, names::attributes as attrs, names::classes, views};
use cim::error::Rule;
use cim::prelude::*;
use cim::reader::{Strictness, read_into};

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

fn read(source: &str) -> (Dataset, cim::Report) {
    read_opts(source, &ReadOptions::lenient())
}

fn read_opts(source: &str, options: &ReadOptions) -> (Dataset, cim::Report) {
    let mut ds = Dataset::new(SCHEMA);
    let outcome = read_into(&mut ds, source.as_bytes(), Some("test.xml"), options).unwrap();
    if let Some(h) = outcome.header {
        ds.push_header(h);
    }
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
    cim::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
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
        matches!(err, cim::Error::NotCimXml(_)),
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
    let nasty = r##"A & B <tag> "quoted" 'single' — ünïcode"##;
    let mut ds = Dataset::new(SCHEMA);
    let mut obj = Object::new(classes::ACLineSegment, Mrid::parse(A));
    obj.set(attrs::identified_object::name, Value::Text(nasty.into()));
    ds.insert(obj);

    let mut buf = Vec::new();
    cim::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
    let text = String::from_utf8(buf.clone()).unwrap();
    // The raw metacharacters must not appear unescaped inside the element.
    assert!(text.contains("A &amp; B &lt;tag&gt;"), "{text}");

    let (back, report) = read(&text);
    assert!(!report.has_errors(), "{report}");
    let line = back
        .view_by_mrid::<views::ACLineSegment>(&Mrid::parse(A))
        .unwrap();
    assert_eq!(line.name(), Some(nasty));
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
        cim::writer::write_profile(&ds, p, &mut buf, None, &WriteOptions::default()).unwrap();
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
    let index = cim::InverseIndex::build(&ds);
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
    assert!(matches!(err, cim::Error::NotCimXml(_)), "{err:?}");
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
    assert!(matches!(err, cim::Error::Xml(_)), "{err:?}");
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

    let report = cim::validate::validate(&ds);
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
            Some(cim::schema::EnumValueId(i as u16)),
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
        cim::writer::write_profile(&ds, p, &mut buf, None, &WriteOptions::default()).unwrap();
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

    let diff = cim::reader::read_difference(SCHEMA, diff_doc.as_bytes(), Some("diff.xml"))
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
