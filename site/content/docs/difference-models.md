+++
title = "Difference models"
description = "IEC 61970-552 incremental exchange with cim-rs: read, apply, write and — the operation an EMS actually performs — compute a dm:DifferenceModel from two model states."
weight = 60
+++

IEC 61970-552 defines `dm:DifferenceModel` as the incremental form of exchange: rather than
resending a whole grid model, a producer sends the statements to **retract** and the
statements to **assert**.

`cim-rs` does all four operations — read, apply, write, and *compute*. The fourth is the one
an EMS performs every time it publishes an update, and it is the side of the standard's own
round trip most implementations leave open.

## Computing one

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;
use cim_rs::diff::DiffOptions;

fn main() -> cim_rs::Result<()> {
    let base = Dataset::load_dir(SCHEMA, "before")?;
    let updated = Dataset::load_dir(SCHEMA, "after")?;

    // Restricted to the profile whose file will carry it.
    let ssh = SCHEMA.profile_by_keyword("SSH").expect("the SSH profile");
    let change = base.difference_to(&updated, &DiffOptions::default().profiles(ssh.mask()));

    println!("{} to retract, {} to assert",
             change.model.reverse.len(), change.model.forward.len());

    cim_rs::writer::write_difference(
        SCHEMA, &change.model, std::io::stdout(), &Default::default())
}
```

Profile filtering uses the same rule the CIM/XML writer uses, so a change set restricted to
Steady State Hypothesis contains exactly what an SSH file would and no equipment change
leaks into it.

**The property that makes this worth having is tested, not claimed:** applying
`base.difference_to(&target)` to the base reproduces the target — every value present, none
left over, class changes included — and the change set survives the trip through its own
serialized form on the way, because that is how it is actually delivered.

## Reading and applying one

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let mut grid = Dataset::load_dir(SCHEMA, "base")?;
    let file = std::io::BufReader::new(std::fs::File::open("change_DIFF.xml")?);

    if let Some(diff) = cim_rs::reader::read_difference(SCHEMA, file, None)? {
        let report = grid.apply_difference(&diff);   // retract, re-type, assert
        grid.prune_empty();                          // 552 has no "delete object" statement
        println!("{} finding(s)", report.len());
    }
    Ok(())
}
```

Difference files read as part of a model set are retained, so `save_as_loaded` writes the
whole set back including its change files.

### Order matters, and it is the standard's

Retract → **re-type** → assert. A reverse statement talks about the object as it *was*, so
it has to land while the object still has its old class; a forward statement talks about
what it becomes. The published conformity change sets replace a `LinearShuntCompensator`
with a `NonlinearShuntCompensator` under one identifier and then set attributes only the new
class has — the case that re-typing last gets exactly backwards.

A reclassification that moves an object sideways in the hierarchy leaves the old class's own
attributes with nowhere to live. They are shed and reported, rather than kept where nothing
would ever look at them again.

## What a statement-level difference cannot say

Two limits are the standard's, not this implementation's, and both are reported rather than
papered over.

**An object cannot be deleted, only emptied.** There is no statement meaning "this identifier
no longer denotes anything". Retracting everything an object said is the whole of what the
syntax can express, and the identifier is left behind with nothing attached. A computed
difference reports how many objects it could not delete, so an applied change set that does
not reproduce the target explains itself.

`Dataset::prune_empty` removes such shells, and is deliberately *not* wired into
`apply_difference`: a model legitimately contains objects described entirely by files that
are not loaded, and deleting those would be worse than keeping a shell.

**A compound has no statement form.** `Location.mainAddress` holds a `StreetAddress` inline,
and `rdf:parseType="Statements"` has nowhere to put one. A changed compound becomes a
diagnostic and stays out of the change set — emitting it as text would fabricate a value.

## Reclassification is a statement

IEC 61970-552 treats an object's type like any other statement. A computed difference
therefore names the class on the statement group when — and only when — the class changed,
and emits a group even where no attribute differs, since otherwise applying it would leave
the object as the class it used to be.

## From a shell

```bash
cim diff before/ after/ > change_DIFF.xml
cim diff before/ after/ --profile SSH --out ssh_DIFF.xml

# …and the receiving half: apply one to the model it was computed against.
cim apply before/ --change change_DIFF.xml --out updated/
```

`apply` names its change sets with `--change` rather than finding them among the inputs,
because "this file is part of my model" and "this file is a change to it" is a distinction
only the caller can make — and getting it wrong writes the change set back out as though it
were data. Several are applied in the order given, and objects the change leaves empty are
pruned, since retracting everything an object said is as close to deleting it as the
statement syntax gets.

The summary goes to stderr and the document to stdout, so redirecting gives you the change
file and still shows you what changed.

The `changes` example does the same from the library, and additionally replays the change
set onto a fresh copy of the base to show how far the result still is from the target.
