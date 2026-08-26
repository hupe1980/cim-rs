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
mod vintage;

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

/// Resolve one vintage's RDFS files, which `scripts/fetch-specs.sh` places under `specs/`.
fn load(vintage: &'static vintage::Vintage) -> Result<ir::Schema> {
    let dir = root().join("specs").join(vintage.rdfs_dir);
    if !dir.is_dir() {
        bail!(
            "missing {}\nRun scripts/fetch-specs.sh first to download the standards artifacts.",
            dir.display()
        );
    }
    let mut sources = Vec::new();
    for spec in vintage.profiles {
        let path = dir.join(spec.file);
        if !path.is_file() {
            bail!(
                "vintage {} names {} but it is not present in {}",
                vintage.key,
                spec.file,
                dir.display()
            );
        }
        sources.push(ir::ProfileSource { path, spec });
    }
    ir::build(vintage.key, &sources)
}

fn codegen(check: bool) -> Result<()> {
    let out_dir = root().join("crates/cim/src/generated");
    let schemas: Vec<ir::Schema> = vintage::VINTAGES.iter().map(load).collect::<Result<_>>()?;
    emit::emit(&schemas, &out_dir, check)
}

fn inspect() -> Result<()> {
    for v in vintage::VINTAGES {
        let schema = match load(v) {
            Ok(s) => s,
            Err(e) => {
                println!("== {} ==\n  unavailable: {e}\n", v.key);
                continue;
            }
        };
        println!("== {} — {} ==", schema.vintage, v.title);
        println!("  profiles: {}", schema.profiles.len());
        for p in &schema.profiles {
            println!(
                "    {:<6} {:<52} {}",
                p.keyword, p.version_iri, p.source_file
            );
            for alias in &p.aliases {
                println!("           also {alias}");
            }
        }
        println!("  namespaces:");
        for (ns, prefix) in &schema.namespaces {
            println!("    {prefix:<8} {ns}");
        }
        let concrete = schema.classes.values().filter(|c| c.concrete).count();
        let attrs: usize = schema.classes.values().map(|c| c.attributes.len()).sum();
        println!(
            "  classes: {} ({concrete} concrete), attributes: {attrs}, enums: {} ({} values), datatypes: {}",
            schema.classes.len(),
            schema.enums.len(),
            schema.enums.values().map(|e| e.values.len()).sum::<usize>(),
            schema.datatypes.len()
        );

        // Attributes exclusive to one profile show whether the split worked.
        for (i, p) in schema.profiles.iter().enumerate() {
            let bit: u32 = 1 << i;
            let exclusive = schema
                .classes
                .values()
                .flat_map(|c| &c.attributes)
                .filter(|a| a.profiles == bit)
                .count();
            let total = schema
                .classes
                .values()
                .flat_map(|c| &c.attributes)
                .filter(|a| a.profiles & bit != 0)
                .count();
            println!(
                "    {:<6} {total:>5} attributes, {exclusive:>5} exclusive",
                p.keyword
            );
        }
        println!();
    }
    Ok(())
}
