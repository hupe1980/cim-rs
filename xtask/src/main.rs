//! Repository automation for cim-rs.
//!
//! This crate is never published; it turns the RDFS vocabularies under `specs/`
//! into the Rust sources committed under `crates/cim/src/generated/`.
//!
//! Usage:
//!   cargo xtask codegen           regenerate sources in place
//!   cargo xtask codegen --check   fail if the committed sources are stale
//!   cargo xtask inspect           print a summary of the parsed schema

mod emit;
mod ir;
mod rdfs;

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let check = args.iter().any(|a| a == "--check");

    match cmd {
        "codegen" => codegen(check),
        "inspect" => inspect(),
        _ => {
            eprintln!("usage: cargo xtask <codegen [--check] | inspect>");
            Ok(())
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Locate the CGMES 3.0 RDFS directory, which the fetch script places under `specs/`.
fn cgmes3_rdfs_dir() -> Result<PathBuf> {
    let dir = root().join("specs/application-profiles-library/CGMES/CurrentRelease/RDFS");
    if !dir.is_dir() {
        bail!(
            "missing {}\nRun scripts/fetch-specs.sh first to download the standards artifacts.",
            dir.display()
        );
    }
    Ok(dir)
}

fn load_cgmes3() -> Result<ir::Schema> {
    let dir = cgmes3_rdfs_dir()?;
    let mut sources = ir::discover_profiles(
        &dir,
        &["61970-600-2_", "-AP-Voc-RDFS2020", "-AP-Voc-RDFS2019"],
    )?;
    // The 2019 header vocabulary is superseded by the 2020 one shipped alongside it.
    sources.retain(|s| !s.path.to_string_lossy().contains("Header-AP-Voc-RDFS2019"));
    ir::build("cgmes3", &sources)
}

fn codegen(check: bool) -> Result<()> {
    let schema = load_cgmes3()?;
    let out_dir = root().join("crates/cim/src/generated");
    emit::emit(&schema, &out_dir, check)
}

fn inspect() -> Result<()> {
    let schema = load_cgmes3()?;
    println!("vintage: {}", schema.vintage);
    println!("profiles: {}", schema.profiles.len());
    for p in &schema.profiles {
        println!("  {:<6} {:<44} {}", p.keyword, p.version_iri, p.source_file);
    }
    println!("namespaces:");
    for (ns, prefix) in &schema.namespaces {
        println!("  {prefix:<8} {ns}");
    }

    let concrete = schema.classes.values().filter(|c| c.concrete).count();
    let attrs: usize = schema.classes.values().map(|c| c.attributes.len()).sum();
    println!(
        "classes: {} ({} concrete, {} abstract)",
        schema.classes.len(),
        concrete,
        schema.classes.len() - concrete
    );
    println!("attributes (declared): {attrs}");
    println!("enums: {}", schema.enums.len());
    println!(
        "enum values: {}",
        schema.enums.values().map(|e| e.values.len()).sum::<usize>()
    );
    println!("datatypes: {}", schema.datatypes.len());

    // Deepest inheritance chain drives the size of flattened structs.
    let mut worst = ("", 0usize, 0usize);
    for name in schema.classes.keys() {
        let all = schema.all_attributes(name).len();
        let depth = schema.ancestors(name).len();
        if all > worst.2 {
            worst = (&name.local, depth, all);
        }
    }
    println!(
        "widest class: {} (depth {}, {} total attributes)",
        worst.0, worst.1, worst.2
    );

    let total_flat: usize = schema
        .classes
        .values()
        .filter(|c| c.concrete)
        .map(|c| schema.all_attributes(&c.name).len())
        .sum();
    println!("sum of flattened fields over concrete classes: {total_flat}");

    println!("\nsample datatypes:");
    for dt in schema.datatypes.values().take(6) {
        println!(
            "  {:<20} {:?} unit={:?} mult={:?}",
            dt.name.local, dt.value, dt.unit, dt.multiplier
        );
    }
    println!("\nsample class:");
    if let Some((n, _)) = schema
        .classes
        .iter()
        .find(|(n, _)| n.local == "ACLineSegment")
    {
        for (owner, a) in schema.all_attributes(n).iter().take(12) {
            println!(
                "  {:<24}.{:<22} {:?} {:?}",
                owner.name.local, a.label, a.kind, a.multiplicity
            );
        }
    }
    Ok(())
}
