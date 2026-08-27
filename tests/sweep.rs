//! Read the entire published CGMES 3.0 conformity corpus and check nothing is lost.

#![cfg(feature = "cgmes3")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cim_rs::Severity;
use cim_rs::cgmes3::{SCHEMA, views};
use cim_rs::prelude::*;

/// Every directory in the corpus that directly contains instance files.
fn model_dirs(root: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut has_xml = false;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
                has_xml = true;
            }
        }
        if has_xml {
            out.push(d.to_path_buf());
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn every_conformity_model_reads_without_diagnostics() {
    let root = require_corpus!(common::cgmes3_models());
    let dirs = model_dirs(&root);
    assert!(
        dirs.len() > 10,
        "corpus looks incomplete: {} dirs",
        dirs.len()
    );

    let mut by_rule: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut samples: Vec<String> = Vec::new();
    let (mut files, mut objects) = (0usize, 0usize);
    let started = Instant::now();

    for dir in &dirs {
        let paths = common::xml_files(dir);
        let mut ds = Dataset::new(SCHEMA);
        let report = ds
            .load_files(&paths, &ReadOptions::lenient())
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        files += report.files.len();
        objects += ds.len();
        for (rule, n) in report.report.summary() {
            *by_rule.entry(rule.code()).or_default() += n;
        }
        if !report.report.is_empty() && samples.len() < 10 {
            let rel = dir.strip_prefix(&root).unwrap_or(dir);
            for d in report.report.iter().take(2) {
                samples.push(format!("{}: {d}", rel.display()));
            }
        }
    }

    println!(
        "{} model sets, {files} files, {objects} objects in {:.2}s",
        dirs.len(),
        started.elapsed().as_secs_f64()
    );
    for s in &samples {
        println!("  {s}");
    }
    assert!(
        by_rule.is_empty(),
        "reading the published conformity corpus produced diagnostics: {by_rule:?}"
    );
}

/// Export every model set as the files it was read from and compare the serialized form.
///
/// The projection-based round-trip tests compare the assembled *model*, which is blind to
/// how objects are identified and to which file they land in. Comparing per file, per
/// class, catches both — and both were wrong before: State Variables objects were written
/// with `rdf:about` where the published files use `rdf:ID`, and a merged model put every
/// modelling authority's equipment into every authority's Equipment file.
#[test]
fn every_model_set_re_exports_as_the_files_it_came_from() {
    let root = require_corpus!(common::cgmes3_models());
    let out = std::env::temp_dir().join(format!("cim-sweep-export-{}", std::process::id()));
    let mut checked = 0usize;
    let mut problems: Vec<String> = Vec::new();

    for dir in model_dirs(&root) {
        let paths = common::xml_files(&dir);
        let mut ds = Dataset::new(SCHEMA);
        ds.load_files(&paths, &ReadOptions::lenient()).unwrap();

        let _ = std::fs::remove_dir_all(&out);
        let saved = ds.save_as_loaded(&out).unwrap();
        let rel = dir
            .strip_prefix(&root)
            .unwrap_or(&dir)
            .display()
            .to_string();

        for original in &paths {
            let name = original.file_name().unwrap();
            let Some(written) = saved.written.iter().find(|p| p.file_name() == Some(name)) else {
                problems.push(format!("{rel}/{}: not re-exported", name.to_string_lossy()));
                continue;
            };
            let output = std::fs::read_to_string(written).unwrap();
            // Comparing censuses says the output holds the right objects; it says nothing
            // about whether an XML parser will accept the file at all.
            if let Err(e) = common::check_well_formed(&output) {
                problems.push(format!("{rel}/{}: {e}", name.to_string_lossy()));
            }
            let source = std::fs::read_to_string(original).unwrap();
            let before = common::element_census(&source);
            let after = common::element_census(&output);
            if before != after {
                problems.push(format!(
                    "{rel}/{}: {}",
                    name.to_string_lossy(),
                    common::census_diff(&before, &after)
                ));
            }
            // And the identifiers themselves, letter for letter. The census above compares
            // classes and identity styles, so it cannot see an identifier being *rewritten*
            // — which is how a lower-cased UUID reached the published boundary files'
            // export unnoticed.
            let ids_before = common::identifier_census(&source);
            let ids_after = common::identifier_census(&output);
            if ids_before != ids_after {
                let lost: Vec<&String> = ids_before.difference(&ids_after).take(3).collect();
                let gained: Vec<&String> = ids_after.difference(&ids_before).take(3).collect();
                problems.push(format!(
                    "{rel}/{}: identifiers changed — {} lost {lost:?}, {} gained {gained:?}",
                    name.to_string_lossy(),
                    ids_before.difference(&ids_after).count(),
                    ids_after.difference(&ids_before).count(),
                ));
            }
            checked += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&out);

    assert!(checked > 100, "only {checked} files compared");
    assert!(
        problems.is_empty(),
        "{} of {checked} files re-exported differently:\n  {}",
        problems.len(),
        problems
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    println!("{checked} files re-exported with the same objects and identity forms");
}

/// Validate every model set and pin exactly what the checks find.
///
/// Two errors survive, and both are properties of the published models rather than of this
/// crate: FullGrid's `Cut` — a class CGMES 3.0 added — is carried in the Steady State
/// Hypothesis file as a bare `<cim:Equipment>` with only `inService`, so the switch state
/// that profile makes mandatory is genuinely absent. Pinning the census rather than
/// asserting "no errors" keeps the test a regression guard: a new finding, or the
/// disappearance of one of these, fails.
#[test]
fn corpus_validation_findings_are_exactly_the_known_ones() {
    let root = require_corpus!(common::cgmes3_models());
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_rule: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut contradicting: BTreeMap<String, usize> = BTreeMap::new();

    for dir in model_dirs(&root) {
        let paths = common::xml_files(&dir);
        let mut ds = Dataset::new(SCHEMA);
        ds.load_files(&paths, &ReadOptions::lenient()).unwrap();
        if !ds.merge_conflicts().is_empty() {
            let rel = dir.strip_prefix(&root).unwrap_or(&dir);
            contradicting.insert(rel.display().to_string(), ds.merge_conflicts().len());
        }
        let report = ds.validate();
        for (rule, n) in report.summary() {
            *by_rule.entry(rule.code()).or_default() += n;
        }
        for d in report.iter().filter(|d| d.severity == Severity::Error) {
            *errors
                .entry(format!(
                    "{}: {}",
                    d.class.unwrap_or("?"),
                    d.attribute.unwrap_or(&d.message)
                ))
                .or_default() += 1;
        }
    }

    println!("validation findings across the corpus, by rule: {by_rule:?}");
    let expected: BTreeMap<String, usize> = [
        ("Cut: Switch.locked".to_owned(), 1),
        ("Cut: Switch.open".to_owned(), 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(errors, expected, "corpus validation errors changed");

    // Merging assembles a model from its profile files, and a contradiction between two of
    // them means one reading was discarded. Exactly two directories in the corpus produce
    // any, and neither is a defective model set: `MicroGrid-Type3/IGMs` and its assembled
    // counterpart `CGMs` each hold **24 hourly snapshots** of one grid, and this sweep
    // loads a directory at a time. Merging 24 states of the same objects is supposed to
    // contradict itself, and what contradicts is exactly what a time series varies —
    // dispatch (`EquivalentInjection.p/q`, `RotatingMachine.p`, `EnergyConsumer.p`),
    // switching (`ACDCTerminal.connected`, `Equipment.inService`), tap positions
    // (`TapChanger.step`) and the topology those produce (`Terminal.TopologicalNode`).
    // Every other model set in the corpus agrees with itself completely.
    //
    // Pinning the *location* rather than "none" is what makes this a regression guard: a
    // new entry means a model set genuinely disagrees with itself, and an empty result
    // means the detection stopped working.
    println!("directories whose files contradict each other: {contradicting:?}");
    let dirs: Vec<&str> = contradicting.keys().map(String::as_str).collect();
    assert_eq!(
        dirs,
        [
            "MicroGrid/MicroGrid-Type3/CGMs",
            "MicroGrid/MicroGrid-Type3/IGMs"
        ],
        "a different set of model sets now contradicts itself"
    );
    assert!(
        by_rule
            .get(cim_rs::Rule::ConflictingValue.code())
            .copied()
            .unwrap_or(0)
            > 0,
        "the merge-conflict check stopped reporting"
    );
}

/// Every model set must also come out as syntactically valid RDF.
///
/// The point of the RDF export is that somebody else's toolchain reads it, so "this crate
/// can read it back" is not the bar — the N-Triples grammar is. Running the whole corpus
/// through it is what catches an escaping mistake in the one identifier, name or IRI in a
/// quarter of a million objects that happens to need escaping.
///
/// Validating the result against ENTSO-E's SHACL shapes needs a SHACL engine and is
/// therefore a script rather than a test: `cargo xtask shacl`.
#[test]
fn every_model_set_exports_as_valid_rdf() {
    use cim_rs::rdf::{RdfOptions, Syntax};

    let root = require_corpus!(common::cgmes3_models());
    let (mut models, mut triples) = (0usize, 0usize);
    let mut problems: Vec<String> = Vec::new();

    for dir in model_dirs(&root) {
        let mut ds = Dataset::new(SCHEMA);
        ds.load_files(common::xml_files(&dir), &ReadOptions::lenient())
            .unwrap();
        let rel = dir
            .strip_prefix(&root)
            .unwrap_or(&dir)
            .display()
            .to_string();

        let nt = cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::NTriples)).unwrap();
        match common::check_ntriples(&nt) {
            Ok(n) => triples += n,
            Err(e) => problems.push(format!("{rel}: {e}")),
        }
        // Each profile's own graph too: that is the form a consumer validates against the
        // shapes for that profile, and it exercises the element-class rule as well.
        for (i, p) in SCHEMA.profiles.iter().enumerate() {
            let mask = cim_rs::ProfileId(i as u16).mask();
            let slice =
                cim_rs::rdf::to_string(&ds, &RdfOptions::new(Syntax::NTriples).profiles(mask))
                    .unwrap();
            if let Err(e) = common::check_ntriples(&slice) {
                problems.push(format!("{rel} [{}]: {e}", p.keyword));
            }
        }
        models += 1;
    }

    assert!(models > 10, "only {models} model sets");
    assert!(
        problems.is_empty(),
        "{} model sets produced invalid RDF:\n  {}",
        problems.len(),
        problems
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    println!("{models} model sets exported as {triples} valid N-Triples");
}

#[test]
fn microgrid_content_is_actually_populated() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let files = common::xml_files(&dir);
    let mut ds = Dataset::new(SCHEMA);
    ds.load_files(&files, &ReadOptions::lenient()).unwrap();

    let total_values: usize = ds.iter().map(|(_, o)| o.len()).sum();
    println!("objects={} attribute values={total_values}", ds.len());
    assert!(total_values > 1000);

    // Typed views must expose real numeric data.
    let lines: Vec<_> = ds.view::<views::ACLineSegment>().collect();
    assert!(!lines.is_empty(), "MicroGrid must contain AC line segments");
    assert!(
        lines.iter().any(|l| l.r().is_some()),
        "no resistance values read"
    );
    assert!(
        lines.iter().all(|l| l.name().is_some()),
        "every line has a name"
    );

    // Associations must resolve through the dataset.
    let terminals: Vec<_> = ds.view::<views::Terminal>().collect();
    let resolved = terminals
        .iter()
        .filter(|t| t.conducting_equipment_in(&ds).is_some())
        .count();
    assert_eq!(
        resolved,
        terminals.len(),
        "all terminals resolve their equipment"
    );

    // Profile merging: SSH data lands on objects that EQ defined.
    let with_ssh = terminals.iter().filter(|t| t.connected().is_some()).count();
    assert!(with_ssh > 0, "SSH attributes did not merge onto EQ objects");

    // Enumerations resolve to literals.
    let typed = ds
        .view::<views::SynchronousMachine>()
        .filter_map(|m| m.type_())
        .count();
    assert!(typed > 0, "enumeration values did not read");

    // Every object is attributed to the profiles its files declared.
    assert!(
        ds.iter().all(|(_, o)| o.profiles() != 0),
        "objects were not attributed to any profile"
    );
}

#[test]
fn objects_merge_rather_than_duplicate_across_profiles() {
    let dir = require_corpus!(common::cgmes3_model(
        "MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS"
    ));
    let files = common::xml_files(&dir);

    let mut forward = Dataset::new(SCHEMA);
    let r = forward.load_files(&files, &ReadOptions::lenient()).unwrap();
    // More object elements than objects proves merging actually happened.
    assert!(
        r.objects_read > forward.len(),
        "expected merging: {} elements, {} objects",
        r.objects_read,
        forward.len()
    );

    // Load order must not matter.
    let mut reversed: Vec<_> = files.clone();
    reversed.reverse();
    let mut backward = Dataset::new(SCHEMA);
    backward
        .load_files(&reversed, &ReadOptions::lenient())
        .unwrap();
    assert_eq!(
        forward.len(),
        backward.len(),
        "load order changed the model"
    );

    for (_, o) in forward.iter() {
        let other = backward
            .by_mrid(o.mrid())
            .unwrap_or_else(|| panic!("{} missing after reversed load", o.mrid()));
        assert_eq!(o.class(), other.class(), "{}: class differs", o.mrid());
        assert_eq!(
            o.len(),
            other.len(),
            "{}: attribute count differs",
            o.mrid()
        );
    }
}
