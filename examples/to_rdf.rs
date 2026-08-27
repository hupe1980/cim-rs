//! Export one profile of a model as standard RDF, with the datatypes CIM/XML omits.
//!
//! ```text
//! cargo run --example to_rdf --features cgmes3 -- <model-directory> [PROFILE] > eq.ttl
//! ```
//!
//! CIM/XML carries **no datatype information at all** — every value is element text — while
//! ENTSO-E's SHACL shapes constrain thousands of properties by `sh:datatype`. A graph
//! loaded straight from CIM/XML fails all of them, because every literal in it is a string.
//! The profile is the only thing that knows, and this crate holds the profile.
//!
//! Export is **per profile**, not per model, and that is the part that is easy to get
//! wrong: a CGMES profile constrains a reference's target to the classes *it* declares, so
//! validating a merged graph against one profile's shapes reports violations no instance
//! file could have had.

use cim_rs::cgmes3::SCHEMA;
use cim_rs::prelude::*;
use cim_rs::rdf::{RdfOptions, Syntax};

fn main() -> cim_rs::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: to_rdf <model-directory> [PROFILE]");
        std::process::exit(2);
    };
    let keyword = args.next().unwrap_or_else(|| "EQ".to_owned());
    let Some(profile) = SCHEMA.profile_by_keyword(&keyword) else {
        let known: Vec<&str> = SCHEMA.profiles.iter().map(|p| p.keyword).collect();
        eprintln!("no profile {keyword:?}; this vintage has {known:?}");
        std::process::exit(2);
    };

    let ds = Dataset::load_dir(SCHEMA, &input)?;
    eprintln!(
        "{} object(s); writing the {} slice as Turtle",
        ds.len(),
        SCHEMA.profile(profile).keyword
    );

    cim_rs::rdf::write(
        &ds,
        std::io::BufWriter::new(std::io::stdout().lock()),
        &RdfOptions::new(Syntax::Turtle).profiles(profile.mask()),
    )
}
