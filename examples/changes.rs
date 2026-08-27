//! Compute the change set between two model states, and prove it by applying it.
//!
//! ```text
//! cargo run --example changes --features cgmes3 -- <base-dir> <target-dir> [PROFILE]
//! ```
//!
//! IEC 61970-552 defines `dm:DifferenceModel` as the incremental form of exchange: the
//! statements to retract, then the statements to assert. Reading one, applying one and
//! writing one imply a fourth operation — **producing** one — which is what an EMS does
//! every time it publishes an update.
//!
//! The property worth having is checked here rather than asserted: applying the computed
//! change set to the base reproduces the target, every value present and none left over.

use cim_rs::cgmes3::SCHEMA;
use cim_rs::diff::DiffOptions;
use cim_rs::prelude::*;

fn main() -> cim_rs::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [base_dir, target_dir, rest @ ..] = args.as_slice() else {
        eprintln!("usage: changes <base-dir> <target-dir> [PROFILE]");
        std::process::exit(2);
    };

    let base = Dataset::load_dir(SCHEMA, base_dir)?;
    let target = Dataset::load_dir(SCHEMA, target_dir)?;

    // Restricting the comparison to a profile uses the same rule the CIM/XML writer does,
    // so a change set limited to Steady State Hypothesis holds exactly what an SSH file
    // would and no equipment change leaks into it.
    let mut options = DiffOptions::default();
    if let Some(keyword) = rest.first() {
        let Some(p) = SCHEMA.profile_by_keyword(keyword) else {
            eprintln!("no profile {keyword:?}");
            std::process::exit(2);
        };
        options = options.profiles(p.mask());
    }

    let change = base.difference_to(&target, &options);
    eprintln!(
        "{} added, {} removed, {} changed — {} statement(s) to retract, {} to assert",
        change.added,
        change.removed,
        change.changed,
        change.model.reverse.len(),
        change.model.forward.len()
    );
    // Two things a statement-level difference cannot say — an object can only be emptied,
    // not deleted, and a compound has no statement form — are reported, not papered over.
    for d in change.report.iter().take(5) {
        eprintln!("  {d}");
    }

    // Apply it to a fresh copy of the base and check we arrive at the target.
    let mut replayed = Dataset::load_dir(SCHEMA, base_dir)?;
    let applied = replayed.apply_difference(&change.model);
    eprintln!("applied with {} finding(s)", applied.len());
    if options.profiles == 0 {
        // Only a whole-model change set is expected to reproduce the whole target.
        replayed.prune_empty();
        let remaining = replayed.difference_to(&target, &DiffOptions::default());
        eprintln!(
            "after applying, {} statement(s) still separate base from target",
            remaining.model.reverse.len() + remaining.model.forward.len()
        );
    }

    cim_rs::writer::write_difference(
        SCHEMA,
        &change.model,
        std::io::BufWriter::new(std::io::stdout().lock()),
        &Default::default(),
    )
}
