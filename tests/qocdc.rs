//! ENTSO-E's quality-check corpus: real exports, deliberately broken.
//!
//! The conformity models are correct by construction — they are what a *conforming*
//! producer emits, and every reader defect this crate has had lived in the complement of
//! that set. QoCDC is the complement: 100 model sets built for ENTSO-E's Quality of CGMES
//! Datasets Check, from real TSO exports (ELIA, 50Hertz, TenneT), each with a conformant
//! variant and non-conformant ones designed to make a checker say something.
//!
//! It was fetched by `cargo xtask fetch-specs` and read by nothing until this file existed,
//! which is the shape of a gate that looks like coverage. Pointing the reader at it found
//! two defects immediately, both about *sets* rather than documents: a single unreadable
//! file aborted the whole load with a message that did not name it, and vintage detection
//! gave up on the first file instead of asking the next one.
//!
//! What is asserted is deliberately about robustness rather than about findings. The
//! validation rules these models exercise are ENTSO-E's, not this crate's — running them is
//! a SHACL engine's job — so what matters here is that every set is *read*, that the two
//! deliberately unreadable files are named rather than fatal, and that nothing panics.

#![cfg(all(feature = "cgmes2", feature = "cgmes3", feature = "zip"))]

mod common;

use std::path::{Path, PathBuf};

use cim_rs::prelude::*;
use cim_rs::{Rule, Severity};

/// The outer archives `fetch-specs` leaves in place, smallest first.
///
/// TC1 and TC2 are 40 MiB together, cover every test case type, and run in about eight
/// seconds — a gate that takes a minute is a gate people turn off. TC3 and TC4 are 113 MiB
/// and 463 MiB of more of the same, which is worth running somewhere slower than every
/// `cargo test`: set `CIM_QOCDC_ALL=1` for all four, as the scheduled workflow does.
const QUICK: [&str; 2] = ["TC1.zip", "TC2.zip"];
const ALL: [&str; 4] = ["TC1.zip", "TC2.zip", "TC3.zip", "TC4.zip"];

fn archives() -> &'static [&'static str] {
    if std::env::var_os("CIM_QOCDC_ALL").is_some() {
        &ALL
    } else {
        &QUICK
    }
}

fn corpus() -> Option<PathBuf> {
    let dir = common::references()?
        .join("test-models/qocdc-3.2.1")
        .join("QoCDC v3.2.1 test models");
    dir.is_dir().then_some(dir)
}

/// Every directory that directly holds instance files or per-file archives.
fn model_sets(root: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut has_input = false;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("xml") || x.eq_ignore_ascii_case("zip"))
            {
                has_input = true;
            }
        }
        if has_input {
            out.push(d.to_path_buf());
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn every_quality_check_model_set_is_read_and_none_of_them_is_fatal() {
    let Some(corpus) = corpus() else {
        eprintln!("skipping: QoCDC corpus not present (cargo xtask fetch-specs)");
        return;
    };

    let work = std::env::temp_dir().join(format!("cim-qocdc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let archives = archives();
    for name in archives {
        let path = corpus.join(name);
        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        zip::ZipArchive::new(std::io::BufReader::new(file))
            .unwrap()
            .extract(&work)
            .unwrap();
    }

    let sets = model_sets(&work);
    assert!(sets.len() > 80, "only {} model sets extracted", sets.len());

    let mut undetected = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut objects = 0usize;

    for dir in &sets {
        let files = cim_rs::instance_files(dir);
        // Detection over the whole set, not its first file: one of these sets begins with
        // an archive that is deliberately not XML.
        let Some(schema) = cim_rs::load::detect_vintage(&files) else {
            undetected.push(dir.strip_prefix(&work).unwrap().display().to_string());
            continue;
        };
        let mut ds = Dataset::new(schema);
        let report = ds
            .load_files(&files, &ReadOptions::lenient())
            .unwrap_or_else(|e| panic!("{} was fatal: {e}", dir.display()));
        objects += ds.len();

        for d in report.report.iter() {
            if d.rule == Rule::UnreadableFile {
                assert_eq!(d.severity, Severity::Error, "{d}");
                let file = d.source.clone().unwrap_or_default();
                assert!(!file.is_empty(), "an unreadable file was not named: {d}");
                unreadable.push(format!(
                    "{}/{file}",
                    dir.strip_prefix(&work).unwrap().display()
                ));
            }
        }

        // Whatever it read, it must still be able to say what it holds and write it back.
        let _ = ds.validate();
    }
    std::fs::remove_dir_all(&work).ok();

    assert!(
        undetected.is_empty(),
        "no vintage detected for: {undetected:?}"
    );
    // ~41,900 across the two archives; a floor rather than the figure, since the
    // corpus is versioned and this is a "did it actually read anything" check.
    assert!(objects > 35_000, "only {objects} objects read");

    // Pinned by name, not by count: these are the files QoCDC breaks on purpose — one
    // archive holding the text `nonxmlfile`, and one document truncated inside an entity
    // reference, each present as both a loose `.xml` and a `.zip`. A third appearing is a
    // regression; these two disappearing means the check stopped looking.
    unreadable.sort();
    if archives.len() > QUICK.len() {
        // The larger archives bring their own broken files; what is pinned is the set the
        // quick run sees, so the scheduled run asserts the weaker thing — that every one of
        // them was *named*, which the loop above already checked, and that the count did
        // not collapse to zero.
        assert!(
            unreadable.len() >= 3,
            "the deliberately broken files went missing: {unreadable:?}"
        );
        return;
    }
    assert_eq!(
        unreadable,
        [
            "TC1/TC1_T11_NonConform_L1/Combinations/Combination_6/20210125T1900Z_1D_ELIA_EQ_001.zip",
            "TC1/TC1_T4_NonConform/Combination_ModelDescription/20210125T1900Z_1D_ttn_EQ_001.xml",
            "TC1/TC1_T4_NonConform/Combination_ModelDescription/20210125T1900Z_1D_ttn_EQ_001.zip",
        ],
        "the set of deliberately unreadable files changed"
    );
}

/// Every file of the quality corpus that reads cleanly is re-exported as the document it
/// was.
///
/// The conformity models are what a *conforming* producer emits, and they turn out not to
/// exercise two things real exports do constantly. This test is where both were found
/// (D55, D56, D57):
///
/// * a boundary set introduces a node and says nothing else about it —
///   `<cim:ConnectivityNode rdf:ID="_x"/>` with no property children — and selecting what
///   to write by *content* dropped 512 of 591 of them from the file that introduced them,
///   while the other file kept referring to them with `rdf:about`;
/// * a real header under-declares its profiles, naming only `EquipmentCore/3/1` while
///   carrying `LoadArea`, which lives in Equipment Operation — and the identity form then
///   flipped to `rdf:about`, pointing at a definition no file in the set contains;
/// * a real boundary file writes `rdf:ID="7d06eea0-…"` with no leading underscore, which
///   is not an XML `NCName` and therefore not a usable `rdf:ID`. That one *is* repaired on
///   write, because writing it back would be writing an invalid document — so what is
///   asserted here is that the repair was **reported**, not that it did not happen.
///
/// Sets that do not read cleanly are skipped rather than judged: QoCDC breaks files on
/// purpose, and a document this crate could not fully read is not evidence about how it
/// writes.
#[test]
fn every_readable_quality_check_file_re_exports_as_the_document_it_was() {
    let Some(corpus) = corpus() else {
        eprintln!("skipping: QoCDC corpus not present (cargo xtask fetch-specs)");
        return;
    };
    let work = std::env::temp_dir().join(format!("cim-qocdc-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    for name in QUICK {
        let file = std::fs::File::open(corpus.join(name)).unwrap();
        zip::ZipArchive::new(std::io::BufReader::new(file))
            .unwrap()
            .extract(&work)
            .unwrap();
    }
    let out = std::env::temp_dir().join(format!("cim-qocdc-rt-out-{}", std::process::id()));

    let (mut sets, mut files) = (0usize, 0usize);
    let mut problems: Vec<String> = Vec::new();
    let mut repaired = 0usize;

    for dir in model_sets(&work) {
        let rel = dir.strip_prefix(&work).unwrap().display().to_string();
        let inputs = cim_rs::instance_files(&dir);
        let Some(schema) = cim_rs::load::detect_vintage(&inputs) else {
            continue;
        };
        let mut ds = Dataset::new(schema);
        let Ok(report) = ds.load_files(&inputs, &ReadOptions::lenient()) else {
            continue;
        };
        // A set that did not read cleanly says nothing about how this crate writes.
        if report.report.iter().any(|d| d.severity == Severity::Error)
            || !ds.merge_conflicts().is_empty()
        {
            continue;
        }
        // Files whose identifiers this crate had to repair on the way in: the input is not
        // writable as it stands, so the output is *supposed* to differ (D56).
        let repaired_files: std::collections::BTreeSet<String> = report
            .report
            .iter()
            .filter(|d| d.rule == Rule::NonConformingMrid)
            .filter_map(|d| d.source.clone())
            .collect();
        repaired += repaired_files.len();

        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let saved = ds.save_as_loaded(&out).unwrap();
        sets += 1;

        for original in &inputs {
            if original
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
            {
                continue;
            }
            let name = original.file_name().unwrap().to_string_lossy().into_owned();
            let Some(written) = saved
                .written
                .iter()
                .find(|p| p.file_name().map(|n| n.to_string_lossy()) == Some(name.as_str().into()))
            else {
                continue;
            };
            let (Ok(src), Ok(dst)) = (
                std::fs::read_to_string(original),
                std::fs::read_to_string(written),
            ) else {
                continue;
            };
            files += 1;
            common::assert_well_formed(&format!("{rel}/{name}"), &dst);

            let (before, after) = (common::element_census(&src), common::element_census(&dst));
            if before != after {
                problems.push(format!(
                    "{rel}/{name}: {}",
                    common::census_diff(&before, &after)
                ));
            }
            let drift = common::value_census_diff(
                &common::value_census(&src),
                &common::value_census(&dst),
                3,
            );
            if !drift.is_empty() {
                problems.push(format!("{rel}/{name}: values {}", drift.join(" | ")));
            }
            if !repaired_files.contains(&name) {
                let (ib, ia) = (
                    common::identifier_census(&src),
                    common::identifier_census(&dst),
                );
                if ib != ia {
                    problems.push(format!(
                        "{rel}/{name}: identifiers lost {:?} gained {:?}",
                        ib.difference(&ia).take(3).collect::<Vec<_>>(),
                        ia.difference(&ib).take(3).collect::<Vec<_>>()
                    ));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&out);

    println!("quality corpus re-export: {sets} model sets, {files} files compared");
    assert!(
        problems.is_empty(),
        "{} real exports did not come back as themselves:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
    assert!(files > 200, "only {files} files compared");
    // Pinned rather than tolerated: one published boundary file writes an `rdf:ID` that is
    // not an `NCName`. If that stops happening the exclusion above is dead and should go.
    assert!(
        repaired > 0,
        "no identifier repair was reported — the exclusion above now hides nothing, \
         or the diagnostic stopped firing"
    );
}

/// The other two questions this corpus can answer: does a real export differ from itself,
/// and is its RDF projection interchange?
///
/// Reading found two defects (D43, D44) and comparing documents found three more (D55–D57).
/// A corpus answers the question you ask it, so these are the remaining ones this crate has
/// gates for — and they matter most here because QoCDC is overwhelmingly CGMES 2.4.15, the
/// vintage whose RDF export no SHACL engine can check: every one of its published shapes
/// files is invalid SHACL (D51), so the N-Triples grammar over real 2.4.15 data is the
/// strongest structural evidence available for that half of the crate.
#[test]
fn the_quality_corpus_does_not_differ_from_itself_and_exports_as_interchange() {
    let Some(corpus) = corpus() else {
        eprintln!("skipping: QoCDC corpus not present (cargo xtask fetch-specs)");
        return;
    };
    let work = std::env::temp_dir().join(format!("cim-qocdc-rdf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    for name in QUICK {
        let file = std::fs::File::open(corpus.join(name)).unwrap();
        zip::ZipArchive::new(std::io::BufReader::new(file))
            .unwrap()
            .extract(&work)
            .unwrap();
    }

    let (mut sets, mut triples) = (0usize, 0usize);
    let mut problems: Vec<String> = Vec::new();

    for dir in model_sets(&work) {
        let rel = dir.strip_prefix(&work).unwrap().display().to_string();
        let inputs = cim_rs::instance_files(&dir);
        let Some(schema) = cim_rs::load::detect_vintage(&inputs) else {
            continue;
        };
        let mut ds = Dataset::new(schema);
        let Ok(report) = ds.load_files(&inputs, &ReadOptions::lenient()) else {
            continue;
        };
        if report.report.iter().any(|d| d.severity == Severity::Error)
            || !ds.merge_conflicts().is_empty()
        {
            continue;
        }
        sets += 1;

        // A model differs from itself by nothing — the identity law of the difference
        // machinery, over inputs nobody designed for it.
        if !ds.difference_to(&ds, &Default::default()).is_empty() {
            problems.push(format!("{rel}: a model differs from itself"));
        }

        let mut buf = Vec::new();
        if cim_rs::rdf::write(
            &ds,
            &mut buf,
            &cim_rs::RdfOptions::new(cim_rs::Syntax::NTriples),
        )
        .is_err()
        {
            problems.push(format!("{rel}: RDF export failed"));
            continue;
        }
        match common::check_ntriples(&String::from_utf8(buf).unwrap()) {
            Ok(n) => triples += n,
            Err(e) => problems.push(format!("{rel}: {e}")),
        }
    }
    let _ = std::fs::remove_dir_all(&work);

    println!("quality corpus: {sets} model sets, {triples} triples exported");
    assert!(problems.is_empty(), "{}", problems.join("\n  "));
    assert!(sets > 80, "only {sets} model sets qualified");
    assert!(triples > 200_000, "only {triples} triples exported");
}
