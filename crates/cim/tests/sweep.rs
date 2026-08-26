//! Read the entire published CGMES 3.0 conformity corpus and check nothing is lost.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cim::cgmes3::{SCHEMA, views};
use cim::prelude::*;

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
