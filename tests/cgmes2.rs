//! CGMES 2.4.15 against its own published conformity corpus.
//!
//! This is the test that makes "a new vintage is a regeneration, not a rewrite" a claim
//! rather than an aspiration: nothing outside the generated tables is vintage-specific,
//! so the same reader, writer and validator are exercised here against a schema with a
//! different namespace, a different extension prefix, an extra boundary profile, and
//! vocabularies that predate the self-describing ontology header.

#![cfg(all(feature = "cgmes2", feature = "zip"))]

mod common;

use std::path::{Path, PathBuf};

use cim_rs::cgmes2::{SCHEMA, views};
use cim_rs::prelude::*;

/// CGMES 2.4.15 conformity models ship as zip archives.
fn cas2_archives(rel: &str) -> Option<Vec<PathBuf>> {
    let dir = common::specs()?.join("test-models/cas-2.0").join(rel);
    if !dir.is_dir() {
        return None;
    }
    let mut out = Vec::new();
    collect_zips(&dir, &mut out);
    out.sort();
    (!out.is_empty()).then_some(out)
}

fn collect_zips(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_zips(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            out.push(p);
        }
    }
}

#[test]
fn the_schema_is_the_cim16_vintage() {
    assert_eq!(SCHEMA.vintage, "cgmes2");

    // CGMES 2.4.15 uses the CIM16 namespace and the `entsoe` extension prefix, where
    // CGMES 3.0 uses CIM100 and `eu`.
    let iris: Vec<&str> = SCHEMA.namespaces.iter().map(|n| n.iri).collect();
    assert!(
        iris.contains(&"http://iec.ch/TC57/2013/CIM-schema-cim16#"),
        "expected the CIM16 namespace, got {iris:?}"
    );
    assert!(
        iris.contains(&"http://entsoe.eu/CIM/SchemaExtension/3/1#"),
        "expected the ENTSO-E extension namespace, got {iris:?}"
    );

    // The boundary profile that CGMES 3.0 dropped is present here.
    assert!(
        SCHEMA.profile_by_keyword("TPBD").is_some(),
        "CGMES 2.4.15 has a Topology Boundary profile"
    );

    // Profile IRIs as they appear in real 2.4.15 headers resolve.
    for iri in [
        "http://entsoe.eu/CIM/EquipmentCore/3/1",
        "http://entsoe.eu/CIM/EquipmentShortCircuit/3/1",
        "http://entsoe.eu/CIM/SteadyStateHypothesis/1/1",
        "http://entsoe.eu/CIM/StateVariables/4/1",
        "http://entsoe.eu/CIM/Topology/4/1",
        "http://entsoe.eu/CIM/DiagramLayout/3/1",
        "http://entsoe.eu/CIM/GeographicalLocation/2/1",
        "http://entsoe.eu/CIM/Dynamics/3/1",
    ] {
        assert!(
            SCHEMA.profile_by_iri(iri).is_some(),
            "profile IRI {iri} does not resolve"
        );
    }

    // Enumerations were parsed: these vocabularies carry no `enum` stereotype, so a
    // reader that relied on it would silently produce empty enumerations.
    assert!(
        SCHEMA.enum_values.len() > 100,
        "only {} enumeration literals",
        SCHEMA.enum_values.len()
    );
    let phase = SCHEMA
        .find_enum_value("http://iec.ch/TC57/2013/CIM-schema-cim16#", "PhaseCode.ABC")
        .expect("PhaseCode.ABC");
    assert_eq!(SCHEMA.enum_value(phase).label, "ABC");
}

#[test]
fn the_equipment_profile_is_split_by_stereotype() {
    // CGMES 2.4.15 keeps Equipment, Operation and ShortCircuit in one vocabulary file,
    // separated only by stereotypes. Each must end up with its own attributes.
    let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
    let sc = SCHEMA.profile_by_keyword("SC").unwrap();

    let r0 = SCHEMA
        .find_attr(
            "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            "ACLineSegment.r0",
        )
        .expect("ACLineSegment.r0 exists");
    let r = SCHEMA
        .find_attr(
            "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            "ACLineSegment.r",
        )
        .expect("ACLineSegment.r exists");

    assert!(
        SCHEMA.attr(r0).is_serialized_in(sc),
        "zero-sequence resistance belongs to ShortCircuit"
    );
    assert!(
        !SCHEMA.attr(r0).is_serialized_in(eq),
        "zero-sequence resistance must not be claimed by the core"
    );
    assert!(SCHEMA.attr(r).is_serialized_in(eq));
    assert!(!SCHEMA.attr(r).is_serialized_in(sc));
}

#[test]
fn reads_the_microgrid_2_4_15_model() {
    let archives = match cas2_archives("MicroGrid/BaseCase_BC") {
        Some(a) => a,
        None => {
            eprintln!("skipping: CGMES 2.4.15 corpus not present; run `cargo xtask fetch-specs`");
            return;
        }
    };
    // One archive holds one model authority set; take the first complete one.
    let archive = archives
        .iter()
        .find(|p| p.to_string_lossy().contains("BE"))
        .unwrap_or(&archives[0]);

    let mut ds = Dataset::new(SCHEMA);
    let report = ds
        .load_file(archive, &ReadOptions::lenient())
        .unwrap_or_else(|e| panic!("{}: {e}", archive.display()));

    println!(
        "{}: {} files, {} objects, {} diagnostics",
        archive.file_name().unwrap().to_string_lossy(),
        report.files.len(),
        ds.len(),
        report.report.len()
    );
    for (rule, n) in report.report.summary() {
        println!("  {rule}: {n}");
    }
    assert!(
        report.report.is_empty(),
        "reading a published 2.4.15 model produced diagnostics:\n{}",
        report.report
    );
    assert!(
        ds.len() > 100,
        "expected a populated model, got {}",
        ds.len()
    );

    // Typed views work exactly as they do for CGMES 3.0.
    let lines: Vec<_> = ds.view::<views::ACLineSegment>().collect();
    assert!(!lines.is_empty(), "no AC line segments");
    assert!(
        lines.iter().any(|l| l.r().is_some()),
        "no resistance values"
    );

    // Profile merging across the 2.4.15 file set.
    let terminals: Vec<_> = ds.view::<views::Terminal>().collect();
    assert!(!terminals.is_empty());
    assert!(
        terminals
            .iter()
            .any(|t| t.conducting_equipment_in(&ds).is_some()),
        "associations do not resolve"
    );
    assert!(
        ds.iter().all(|(_, o)| o.profiles() != 0),
        "objects were not attributed to any profile"
    );
}

/// Archives whose name marks them as difference test configurations.
///
/// These deliberately pair an old base model with a change set, so loading every file at
/// once mixes two states of the same objects. They are exercised by
/// [`applying_a_difference_reclassifies_an_object`] instead.
fn is_difference_archive(p: &Path) -> bool {
    p.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|n| n.contains("difference") || n.contains("_diff"))
}

#[test]
fn every_complete_2_4_15_archive_reads_cleanly() {
    // The whole published 2.4.15 corpus: MicroGrid, MiniGrid, SmallGrid, RealGrid,
    // FullGrid and the standalone configurations.
    let archives = match cas2_archives("") {
        Some(a) => a,
        None => {
            eprintln!("skipping: CGMES 2.4.15 corpus not present");
            return;
        }
    };

    let mut total_objects = 0usize;
    let mut checked = 0usize;
    let mut warnings = 0usize;
    let mut findings: Vec<String> = Vec::new();
    for archive in archives.iter().filter(|p| !is_difference_archive(p)) {
        checked += 1;
        let mut ds = Dataset::new(SCHEMA);
        match ds.load_file(archive, &ReadOptions::lenient()) {
            Ok(r) => {
                total_objects += ds.len();
                // Warnings are acceptable: a handful of published models write an
                // extension enumeration literal under the wrong namespace, which is
                // recovered and reported. Errors are not.
                let name = archive.file_name().unwrap().to_string_lossy();
                for d in r
                    .report
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .take(2)
                {
                    findings.push(format!("{name}: {d}"));
                }
                warnings += r.report.count(Severity::Warning);
            }
            Err(e) => findings.push(format!("{}: LOAD ERROR {e}", archive.display())),
        }
    }
    println!("{checked} complete archives, {total_objects} objects, {warnings} warning(s)");
    for f in findings.iter().take(20) {
        println!("  {f}");
    }
    assert!(checked > 30, "corpus looks incomplete: {checked} archives");
    assert!(
        findings.is_empty(),
        "{} error(s) across the published 2.4.15 corpus",
        findings.len()
    );
}

#[test]
fn applying_a_difference_reclassifies_an_object() {
    // The Type 4 configuration replaces a LinearShuntCompensator with a
    // NonlinearShuntCompensator under the same identifier. Applying the difference must
    // change the object's class, not merely its attributes. Other configurations change
    // only attributes, so every difference archive is tried until one reclassifies.
    let archives = match cas2_archives("MicroGrid") {
        Some(a) => a,
        None => {
            eprintln!("skipping: CGMES 2.4.15 corpus not present");
            return;
        }
    };
    let diff_archives: Vec<_> = archives
        .iter()
        .filter(|p| is_difference_archive(p))
        .collect();
    assert!(
        !diff_archives.is_empty(),
        "the corpus has no difference configurations"
    );

    let tmp_root = std::env::temp_dir().join(format!("cim-cgmes2-diff-{}", std::process::id()));
    let mut total_reclassified = 0usize;
    let mut applied = 0usize;

    for (i, archive) in diff_archives.iter().enumerate() {
        let tmp = tmp_root.join(i.to_string());
        std::fs::create_dir_all(&tmp).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
        zip.extract(&tmp).unwrap();

        let mut all: Vec<PathBuf> = Vec::new();
        collect_xml(&tmp, &mut all);
        all.sort();
        let (diffs, base): (Vec<_>, Vec<_>) = all
            .iter()
            .cloned()
            .partition(|p| p.to_string_lossy().to_ascii_uppercase().contains("_DIFF"));
        if diffs.is_empty() || base.is_empty() {
            continue;
        }

        // The base files of a difference configuration are not necessarily consistent
        // with each other: a Type 4 archive pairs the *old* Equipment file with the
        // *updated* Steady State Hypothesis file, and only applying the change set
        // reconciles them. That inconsistency is reported, and is expected here.
        let mut ds = Dataset::new(SCHEMA);
        let load = ds.load_files(&base, &ReadOptions::lenient()).unwrap();
        let pre_existing_conflicts = load.report.by_rule(cim_rs::Rule::DuplicateMrid).count();

        for path in &diffs {
            let file = std::fs::File::open(path).unwrap();
            let Some(diff) =
                cim_rs::reader::read_difference(SCHEMA, std::io::BufReader::new(file), None)
                    .unwrap()
            else {
                continue;
            };

            // Objects whose class the change set will replace, one entry each.
            let mut pending: Vec<(Mrid, &'static str)> = Vec::new();
            for st in &diff.forward {
                let Some(q) = st.class.as_ref() else { continue };
                let Some(target) = SCHEMA.find_class(&q.ns, &q.local) else {
                    continue;
                };
                let Some(current) = ds.by_mrid(&st.subject).map(|o| o.class()) else {
                    continue;
                };
                if current != target && !pending.iter().any(|(m, _)| *m == st.subject) {
                    pending.push((st.subject.clone(), SCHEMA.class(current).name));
                }
            }

            let report = ds.apply_difference(&diff);
            assert!(
                !report.has_errors(),
                "{}: applying the difference failed:\n{report}",
                path.display()
            );
            applied += 1;

            for (mrid, old) in pending {
                let now = SCHEMA.class(ds.by_mrid(&mrid).unwrap().class()).name;
                assert_ne!(now, old, "{mrid} kept its old class {old}");
                println!(
                    "  {}: {mrid} {old} -> {now}",
                    archive.file_name().unwrap().to_string_lossy()
                );
                total_reclassified += 1;
            }
        }

        // Applying the change set must reconcile the archive: whatever inconsistency the
        // unapplied base showed is gone once the model is brought up to date.
        if pre_existing_conflicts > 0 {
            let after = cim_rs::validate::validate_with(
                &ds,
                &cim_rs::validate::ValidateOptions::essential(),
            );
            assert!(
                !after.has_errors(),
                "{}: still inconsistent after applying the difference:\n{after}",
                archive.display()
            );
            println!(
                "  {}: {pre_existing_conflicts} conflict(s) before, none after",
                archive.file_name().unwrap().to_string_lossy()
            );
        }
    }

    println!("{applied} difference(s) applied, {total_reclassified} object(s) reclassified");
    assert!(applied > 0, "no difference model was applied");
    assert!(
        total_reclassified > 0,
        "no object was reclassified across {} difference archives",
        diff_archives.len()
    );

    std::fs::remove_dir_all(&tmp_root).ok();
}

fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_xml(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
            out.push(p);
        }
    }
}

#[test]
fn a_2_4_15_model_round_trips() {
    let archives = match cas2_archives("MicroGrid/BaseCase_BC") {
        Some(a) => a,
        None => {
            eprintln!("skipping: CGMES 2.4.15 corpus not present");
            return;
        }
    };
    let archive = &archives[0];

    let mut original = Dataset::new(SCHEMA);
    original
        .load_file(archive, &ReadOptions::lenient())
        .unwrap();

    let dir = std::env::temp_dir().join(format!("cim-cgmes2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let saved = original.save_as_loaded(&dir).unwrap();
    assert!(
        saved.skipped.is_empty(),
        "unresolved headers: {:?}",
        saved.skipped
    );
    assert_eq!(
        saved.written.len(),
        original.headers().len(),
        "one output file per input file"
    );

    let mut back = Dataset::new(SCHEMA);
    back.load_files(&saved.written, &ReadOptions::lenient())
        .unwrap();

    assert_eq!(back.len(), original.len(), "object count changed");
    for (_, o) in original.iter() {
        let other = back
            .by_mrid(o.mrid())
            .unwrap_or_else(|| panic!("{} lost in round-trip", o.mrid()));
        assert_eq!(o.class(), other.class(), "{}: class changed", o.mrid());
        assert_eq!(
            o.len(),
            other.len(),
            "{}: attribute count changed",
            o.mrid()
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// The same serialized-form comparison the CGMES 3.0 sweep makes, for the older vintage.
///
/// 2.4.15 exercises it differently: its profiles are named by IRIs the vocabulary does not
/// state, its Equipment vocabulary is split by stereotype rather than by file, and it has a
/// Topology Boundary profile CGMES 3.0 dropped. Nothing about the identification rule is
/// vintage-specific, and this is what says so.
#[test]
fn a_2_4_15_model_re_exports_as_the_files_it_came_from() {
    let archives = match cas2_archives("MicroGrid/BaseCase_BC") {
        Some(a) => a,
        None => {
            eprintln!("skipping: CGMES 2.4.15 corpus not present");
            return;
        }
    };

    let src = std::env::temp_dir().join(format!("cim-cgmes2-src-{}", std::process::id()));
    let out = std::env::temp_dir().join(format!("cim-cgmes2-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(&src).unwrap();

    // Unpack the archive so the input files can be read back for comparison.
    {
        let file = std::fs::File::open(&archives[0]).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
        zip.extract(&src).unwrap();
    }
    let mut inputs = Vec::new();
    collect_xml(&src, &mut inputs);
    inputs.sort();
    assert!(!inputs.is_empty(), "archive held no instance files");

    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(&inputs, &ReadOptions::lenient()).unwrap();
    let _ = std::fs::remove_dir_all(&out);
    let saved = ds.save_as_loaded(&out).unwrap();

    let mut checked = 0;
    for original in &inputs {
        let name = original.file_name().unwrap();
        let Some(written) = saved.written.iter().find(|p| p.file_name() == Some(name)) else {
            panic!("{} was not re-exported", name.to_string_lossy());
        };
        let output = std::fs::read_to_string(written).unwrap();
        common::assert_well_formed(&name.to_string_lossy(), &output);
        let source = std::fs::read_to_string(original).unwrap();
        let before = common::element_census(&source);
        let after = common::element_census(&output);
        assert_eq!(
            before,
            after,
            "{}: {}",
            name.to_string_lossy(),
            common::census_diff(&before, &after)
        );

        // And what the objects *say*, text for text. This vintage exports numbers in forms
        // a formatter would not choose either, so it is the second place the value census
        // earns its keep.
        let values_before = common::value_census(&source);
        let values_after = common::value_census(&output);
        let drift = common::value_census_diff(&values_before, &values_after, 8);
        assert!(
            drift.is_empty(),
            "{}: {} value texts changed:\n{}",
            name.to_string_lossy(),
            drift.len(),
            drift.join("\n")
        );

        // And letter for letter. This vintage is where it matters: the boundary set writes
        // identifiers in *mixed* case — `_24C12434-E42B-497f-928F-119C6AE92079` — and an
        // `Mrid` that remembered hyphenation but not case rewrote all 26 of them. The
        // census above cannot see that, since the class and the identity style are
        // unchanged; PowSyBl could, because an IIDM identifier is the mRID string.
        let ids_before = common::identifier_census(&source);
        let ids_after = common::identifier_census(&output);
        assert_eq!(
            ids_before,
            ids_after,
            "{}: identifiers were rewritten — lost {:?}, gained {:?}",
            name.to_string_lossy(),
            ids_before
                .difference(&ids_after)
                .take(3)
                .collect::<Vec<_>>(),
            ids_after
                .difference(&ids_before)
                .take(3)
                .collect::<Vec<_>>(),
        );
        checked += 1;
    }
    println!("{checked} CGMES 2.4.15 files re-exported unchanged");

    std::fs::remove_dir_all(&src).ok();
    std::fs::remove_dir_all(&out).ok();
}

/// The RDF export is vintage-agnostic too, including the datatypes.
///
/// Nothing in `cim_rs::rdf` names a class, a profile or a namespace: types come from the
/// schema, and CGMES 2.4.15 has its own header vocabulary in its own `cim16` namespace.
/// If any of that had leaked into the writer, this is where it would show.
#[test]
fn the_rdf_export_works_on_the_older_vintage_as_well() {
    use cim_rs::rdf::{RdfOptions, Syntax};

    let archives = require_corpus!(cas2_archives("MicroGrid"));
    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(&archives[..1], &ReadOptions::lenient())
        .unwrap();
    assert!(!ds.is_empty(), "archive held no objects");

    let nt = cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::NTriples)).unwrap();
    let triples = common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}"));
    assert!(triples > 100, "only {triples} triples");

    // The CIM16 namespace, not CIM100, and literals typed from the 2.4.15 vocabulary.
    assert!(
        nt.contains("http://iec.ch/TC57/2013/CIM-schema-cim16#"),
        "wrong namespace"
    );
    assert!(
        !nt.contains("CIM100"),
        "CGMES 3.0 namespace leaked into a 2.4.15 export"
    );
    assert!(
        nt.contains("^^<http://www.w3.org/2001/XMLSchema#float>"),
        "no typed floats: the datatype mapping did not reach this vintage"
    );

    // And Turtle describes the same graph.
    let ttl = cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::Turtle)).unwrap();
    let subjects = ttl
        .lines()
        .filter(|l| l.starts_with('<') || l.starts_with("_:"))
        .count();
    assert_eq!(subjects + ttl.matches(" ;\n").count(), triples);
}

/// A published boundary file's hyphen-less identifiers must still be `urn:uuid:` names.
///
/// `CGMES_v2.4.15_FullGridTestConfiguration_BD_v1` writes nine of its objects as
/// `rdf:ID="_1fa19c281c8f4e1eaad9e1cab70f923e"`. That is the UUID
/// `1fa19c28-1c8f-4e1e-aad9-e1cab70f923e` with the hyphens left out, and reading it as
/// opaque text — the obvious reading of "not the form IEC 61970-552 requires" — gave those
/// objects **blank nodes** in the RDF export: no global name, unreachable by a SHACL
/// `sh:targetNode`, and not joinable with the same object exported from another file.
#[test]
fn a_published_boundary_files_compact_identifiers_are_named_in_rdf() {
    use cim_rs::rdf::{RdfOptions, Syntax};

    let archives = require_corpus!(cas2_archives("FullGrid"));
    let boundary = archives
        .iter()
        .find(|p| p.to_string_lossy().contains("_BD_"))
        .expect("the FullGrid boundary archive");

    let mut ds = Dataset::new(SCHEMA);
    ds.load_file(boundary, &ReadOptions::lenient()).unwrap();

    let compact = ds
        .iter()
        .filter(|(_, o)| o.mrid().form() == cim_rs::MridForm::Compact)
        .count();
    assert!(
        compact > 0,
        "this archive is the one that spells identifiers without hyphens"
    );

    // Every one of them is a UUID, so every one of them gets a global name. The archive
    // also carries genuinely opaque identifiers — `…923e_X`, a suffixed node name — and
    // those stay blank nodes, which is the correct answer for something that is not a UUID.
    let nt = cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::NTriples)).unwrap();
    common::check_ntriples(&nt).unwrap_or_else(|e| panic!("{e}"));
    for (_, o) in ds.iter() {
        let m = o.mrid();
        if m.form() == cim_rs::MridForm::Opaque {
            continue;
        }
        assert!(
            nt.contains(&format!("<{}>", m.to_urn())),
            "{m} ({:?}) has no urn:uuid: subject in the graph",
            m.form()
        );
    }
    assert!(nt.contains("<urn:uuid:1fa19c28-1c8f-4e1e-aad9-e1cab70f923e>"));

    // …and the CIM/XML writer still puts the file's own spelling back.
    let out = std::env::temp_dir().join(format!("cim-compact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let saved = ds.save_as_loaded(&out).unwrap();
    let written: String = saved
        .written
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert!(
        written.contains("_1fa19c281c8f4e1eaad9e1cab70f923e"),
        "the export re-hyphenated an identifier the published file wrote without hyphens"
    );
    assert!(
        !written.contains("1fa19c28-1c8f-4e1e-aad9-e1cab70f923e"),
        "the export wrote both spellings of one identifier"
    );
    std::fs::remove_dir_all(&out).ok();
}

#[test]
fn the_two_vintages_are_independent() {
    // Identifiers are per-vintage; mixing them would be a silent correctness disaster.
    #[cfg(feature = "cgmes3")]
    {
        let cim3 = cim_rs::cgmes3::SCHEMA;
        assert_ne!(cim3.vintage, SCHEMA.vintage);
        // The CIM100 namespace is not part of the 2.4.15 schema, and vice versa.
        assert!(SCHEMA.ns_id("http://iec.ch/TC57/CIM100#").is_none());
        assert!(
            cim3.ns_id("http://iec.ch/TC57/2013/CIM-schema-cim16#")
                .is_none()
        );
    }
}
