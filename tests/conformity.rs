//! Read the published ENTSO-E CGMES 3.0 conformity assessment models.
//!
//! These are the models ENTSO-E provides for testing CGMES implementations, so they are
//! the right yardstick: if the library cannot read them faithfully, it cannot be used.

#![cfg(feature = "cgmes3")]

mod common;

use cim_rs::cgmes3::SCHEMA;
use cim_rs::prelude::*;

#[test]
fn reads_the_microgrid_base_case() {
    let root = require_corpus!(common::cgmes3_models());
    let dir = root.join("MicroGrid/MicroGid-BaseCase/MicroGrid-NL-MAS");
    let files = common::xml_files(&dir);
    assert!(
        !files.is_empty(),
        "no instance files under {}",
        dir.display()
    );

    let mut ds = Dataset::new(SCHEMA);
    let report = ds.load_files(&files, &ReadOptions::lenient()).unwrap();

    println!(
        "loaded {} files, {} objects read, {} distinct objects, {} diagnostics",
        report.files.len(),
        report.objects_read,
        ds.len(),
        report.report.len()
    );
    for (rule, n) in report.report.summary() {
        println!("  {rule}: {n}");
    }

    assert!(
        ds.len() > 100,
        "expected a populated model, got {}",
        ds.len()
    );
    // Every file must have produced a header.
    assert_eq!(ds.headers().len(), report.files.len());
}
