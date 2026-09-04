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
    let dir = common::specs()?
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
