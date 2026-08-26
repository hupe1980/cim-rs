//! Throughput measurement against a published conformity model.
//!
//! Run with: cargo run --release --example bench -- <model directory>

use std::path::{Path, PathBuf};
use std::time::Instant;

use cim::cgmes3::SCHEMA;
use cim::prelude::*;

fn xml_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: bench <model directory>");
    let files = xml_files(Path::new(&dir));
    let mib = |b: u64| b as f64 / (1u64 << 20) as f64;
    let bytes: u64 = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    println!("{} files, {:.1} MiB", files.len(), mib(bytes));

    let t = Instant::now();
    let mut ds = Dataset::new(SCHEMA);
    let report = ds.load_files(&files, &ReadOptions::lenient()).unwrap();
    let read = t.elapsed();
    let values: usize = ds.iter().map(|(_, o)| o.len()).sum();
    println!(
        "read   {:>7.2}s  {:>6.1} MiB/s  {} objects, {} values, {} diagnostics",
        read.as_secs_f64(),
        mib(bytes) / read.as_secs_f64(),
        ds.len(),
        values,
        report.report.len()
    );

    let t = Instant::now();
    let index = cim::InverseIndex::build(&ds);
    println!("index  {:>7.2}s", t.elapsed().as_secs_f64());
    std::hint::black_box(&index);

    let t = Instant::now();
    let mut written = 0u64;
    for cov in cim::validate::profile_coverage(&ds) {
        if cov.objects == 0 {
            continue;
        }
        let mut buf = Vec::new();
        cim::writer::write_profile(&ds, cov.profile, &mut buf, None, &WriteOptions::default())
            .unwrap();
        written += buf.len() as u64;
    }
    let write = t.elapsed();
    println!(
        "write  {:>7.2}s  {:>6.1} MiB/s  {:.1} MiB out",
        write.as_secs_f64(),
        mib(written) / write.as_secs_f64(),
        mib(written)
    );

    let t = Instant::now();
    let vr = cim::validate::validate(&ds);
    println!(
        "check  {:>7.2}s  {} diagnostics",
        t.elapsed().as_secs_f64(),
        vr.len()
    );
    for (rule, n) in vr.summary().iter().take(6) {
        println!("         {rule}: {n}");
    }
}
