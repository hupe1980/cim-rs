//! Walk a grid model with typed views: substations, their voltage levels, their equipment.
//!
//! ```text
//! cargo run --example inspect --features cgmes3 -- <model-directory-or-file>...
//! ```
//!
//! The point is navigation. CIM associations are stored as identifiers, not pointers,
//! because IEC 61970-501 serializes exactly one side of each — so following one *forwards*
//! is a lookup ([`TypedRef::get`], or the generated `…_in(&dataset)` accessor) and
//! following it *backwards* is an inverse index. Both are shown here: a voltage level
//! names its substation, and finding a substation's voltage levels means inverting that.

use cim_rs::cgmes3::{SCHEMA, views};
use cim_rs::prelude::*;

fn main() -> cim_rs::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: inspect <model-directory-or-file>...");
        std::process::exit(2);
    }

    // One walk, one sorted list, one dataset: several profile files describe the *same*
    // objects, so loading them together is what makes them a model rather than a pile.
    let files: Vec<_> = args
        .iter()
        .flat_map(|a| cim_rs::instance_files(std::path::Path::new(a)))
        .collect();
    let mut ds = Dataset::new(SCHEMA);
    let load = ds.load_files(&files, &ReadOptions::lenient())?;
    eprintln!(
        "{} file(s), {} object(s), {} read diagnostic(s)",
        files.len(),
        ds.len(),
        load.report.len()
    );

    // Reverse traversal is a scan unless it is indexed. Build the index once.
    let inverse = cim_rs::InverseIndex::build(&ds);

    for substation in ds.view::<views::Substation>() {
        println!("{}", substation.name().unwrap_or("(unnamed)"));

        // `VoltageLevel.Substation` is the serialized side, so the voltage levels of a
        // substation are the objects whose `Substation` role points at this one.
        for id in inverse.referrers(
            cim_rs::cgmes3::attributes::voltage_level::Substation,
            substation.mrid(),
        ) {
            let Some(vl) = ds.get(*id).map(views::VoltageLevel::from_object) else {
                continue;
            };
            let kv = vl.base_voltage_in(&ds).and_then(|bv| bv.nominal_voltage());
            println!("  {} — {} kV", vl.name().unwrap_or("(unnamed)"), fmt(kv));

            for id in inverse.referrers(
                cim_rs::cgmes3::attributes::equipment::EquipmentContainer,
                vl.mrid(),
            ) {
                let Some(o) = ds.get(*id) else { continue };
                let name = o
                    .get(cim_rs::cgmes3::attributes::identified_object::name)
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unnamed)");
                println!("    {:<24} {name}", SCHEMA.class(o.class()).name);
            }
        }
    }

    // A line's electrical parameters come from EQ; whether it is in service comes from SSH.
    // Both are on one object, which is the whole point of assembling the set.
    println!();
    for line in ds.view::<views::ACLineSegment>().take(10) {
        println!(
            "{:<28} r={} x={} in service={:?}",
            line.name().unwrap_or("(unnamed)"),
            fmt(line.r()),
            fmt(line.x()),
            line.in_service()
        );
    }
    Ok(())
}

fn fmt(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| format!("{v}"))
}
