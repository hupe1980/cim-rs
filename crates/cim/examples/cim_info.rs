//! Inspect a CGMES model set: what it contains, and whether it checks out.
//!
//! ```text
//! cargo run --example cim_info -- <file-or-directory>...
//! ```

use std::path::{Path, PathBuf};

use cim::cgmes3::{SCHEMA, views};
use cim::prelude::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: cim_info <file-or-directory>...");
        std::process::exit(2);
    }

    let mut files = Vec::new();
    for a in &args {
        collect(Path::new(a), &mut files);
    }
    if files.is_empty() {
        eprintln!("no CIM/XML or zip inputs found");
        std::process::exit(1);
    }
    files.sort();

    let mut ds = Dataset::new(SCHEMA);
    let load = match ds.load_files(&files, &ReadOptions::lenient()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    println!("== files ==");
    for (path, header) in &load.files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match header {
            Some(h) => {
                let profiles: Vec<&str> = h
                    .profiles
                    .iter()
                    .filter_map(|iri| SCHEMA.profile_by_iri(iri))
                    .map(|p| SCHEMA.profile(p).keyword)
                    .collect();
                println!(
                    "  {name:<48} {:<18} {}",
                    profiles.join("+"),
                    h.scenario_time.as_deref().unwrap_or("-")
                );
            }
            None => println!("  {name:<48} (no header)"),
        }
    }

    println!("\n== model ==");
    println!("  objects        {}", ds.len());
    println!(
        "  values         {}",
        ds.iter().map(|(_, o)| o.len()).sum::<usize>()
    );
    println!("  substations    {}", ds.count_view::<views::Substation>());
    println!(
        "  lines          {}",
        ds.count_view::<views::ACLineSegment>()
    );
    println!(
        "  transformers   {}",
        ds.count_view::<views::PowerTransformer>()
    );
    println!(
        "  machines       {}",
        ds.count_view::<views::SynchronousMachine>()
    );
    println!("  terminals      {}", ds.count_view::<views::Terminal>());

    println!("\n== profile coverage ==");
    for cov in cim::validate::profile_coverage(&ds) {
        if cov.objects == 0 {
            continue;
        }
        println!(
            "  {:<6} {:>7} objects {:>9} values{}",
            cov.keyword,
            cov.objects,
            cov.attributes,
            if cov.missing_required > 0 {
                format!("  ({} required attributes absent)", cov.missing_required)
            } else {
                String::new()
            }
        );
    }

    let mut report = load.report;
    report.extend(cim::validate::validate(&ds));

    println!("\n== diagnostics ==");
    if report.is_empty() {
        println!("  none");
    } else {
        for (rule, n) in report.summary() {
            println!("  {rule}  {n:>6}");
        }
        println!("\n  first findings:");
        for d in report.iter().take(10) {
            println!("    {d}");
        }
    }

    if report.has_errors() {
        std::process::exit(1);
    }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(rd) = std::fs::read_dir(path) else {
            return;
        };
        for e in rd.flatten() {
            collect(&e.path(), out);
        }
    } else if path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("xml") || x.eq_ignore_ascii_case("zip"))
    {
        out.push(path.to_path_buf());
    }
}
