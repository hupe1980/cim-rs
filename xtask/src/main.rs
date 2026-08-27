//! Repository automation for cim-rs.
//!
//! This crate is never published. It is the whole of the repository's tooling: fetching
//! the standards artifacts, turning the RDFS vocabularies under `specs/` into the Rust
//! sources committed under `src/generated/`, and driving the interoperability check.
//!
//! Usage:
//!   cargo xtask fetch-specs       download the standards artifacts into specs/
//!   cargo xtask fetch-specs --clean   discard specs/ first
//!   cargo xtask codegen           regenerate sources in place
//!   cargo xtask codegen --check   fail if the committed sources are stale
//!   cargo xtask inspect           print a summary of the parsed schema
//!   cargo xtask shacl [model]     validate the RDF export against ENTSO-E's shapes
//!   cargo xtask crossvalidate [model]  check our output against PowSyBl and rdflib

mod crossvalidate;
mod emit;
mod ir;
mod rdfs;
mod shacl;
mod specs;
mod vintage;

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

const USAGE: &str = "\
usage: cargo xtask <command>

  fetch-specs [--clean]           download the standards artifacts into specs/
  codegen [--check]               regenerate src/generated from the RDFS vocabularies
  inspect                         summarise every parsed vintage
  shacl [model-dir]               validate the RDF export against ENTSO-E's SHACL shapes
  crossvalidate [model] [--vintage KEY]
                                  check our output against PowSyBl and rdflib (needs Docker)
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let flag = |name: &str| args.iter().any(|a| a == name);
    /// Options that take a value, so the value is not mistaken for the positional argument.
    const VALUED: &[&str] = &["--vintage"];
    let value = |name: &str| -> Option<&str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    // The first bare word after the command — skipping any word that is a flag's *value*.
    // Without the skip, `crossvalidate --vintage cgmes2` takes `cgmes2` for the model path
    // and then fails to find a model set by that name, which reads as a corpus problem
    // rather than as an argument one.
    let positional = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(i, a)| {
            !a.starts_with('-')
                && !args
                    .get(i.wrapping_sub(1))
                    .is_some_and(|prev| VALUED.contains(&prev.as_str()))
        })
        .map(|(_, a)| a.as_str());

    match cmd {
        "fetch-specs" => specs::fetch(&root(), flag("--clean")),
        "codegen" => codegen(flag("--check")),
        "inspect" => inspect(),
        "shacl" => shacl::check(&root(), positional),
        // `--vintage` picks which schema the export runs against; the reference
        // implementations detect the vintage from the files themselves.
        "crossvalidate" => {
            crossvalidate::check(&root(), positional, value("--vintage").unwrap_or("cgmes3"))
        }
        _ => {
            eprintln!("{USAGE}");
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

/// Resolve one vintage's RDFS files, which `cargo xtask fetch-specs` places under `specs/`.
fn load(vintage: &'static vintage::Vintage) -> Result<ir::Schema> {
    let dir = root().join("specs").join(vintage.rdfs_dir);
    if !dir.is_dir() {
        bail!(
            "missing {}\nRun `cargo xtask fetch-specs` first to download the standards \
             artifacts.",
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
    let out_dir = root().join("src/generated");
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
