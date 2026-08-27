+++
title = "Getting started"
description = "Install cim-rs, load a CGMES model set, navigate it with typed views and check it against the profile."
weight = 10
+++

## Install

```bash
cargo add cim-rs
```

The package is `cim-rs`; the library it provides is `cim_rs`, as Cargo's usual
hyphen-to-underscore rule implies. (`cim` on crates.io has been taken since 2022 by an
unrelated tool.)

| Feature | Effect |
|---|---|
| `cgmes3` *(default)* | CGMES 3.0 schema, typed views and named constants |
| `cgmes2` | CGMES 2.4.15 schema, typed views and named constants |
| `zip` | Read and write model sets packaged as zip archives |
| `cli` | The `cim` command line (implies `zip`) |

```bash
cargo add cim-rs --no-default-features --features cgmes2   # only the 2.4.15 vintage
cargo add cim-rs --features cgmes2,zip                     # both vintages, plus archives
cargo install cim-rs --features cli                        # the command line
```

Vintages are independent modules, so enabling only the one you need keeps compile time and
binary size down. Both can be on at once — identifiers are per-vintage and cannot be mixed
by accident.

## Load a model

A CGMES model is not a file. It is a **set** of profile files that describe the same
objects from different angles, and in practice it arrives as a directory or a zip archive.

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let grid = Dataset::load_dir(SCHEMA, "MicroGrid-BE")?;
    println!("{} objects from {} files", grid.len(), grid.headers().len());
    Ok(())
}
```

Naming the files works too, and so does merging datasets assembled in parallel:

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    // Name the files directly …
    let grid = Dataset::load(SCHEMA, ["EQ.xml", "SSH.xml", "TP.xml", "SV.xml"])?;
    println!("{} objects", grid.len());

    // … or parse them separately and combine. Merging by mRID does not care which
    // dataset an object arrived in, so this is the same model.
    let mut model = Dataset::new(SCHEMA);
    for part in cim_rs::instance_files("MicroGrid-BE".as_ref()) {
        model.merge(Dataset::load(SCHEMA, [part])?)?;
    }
    Ok(())
}
```

## Navigate it

Typed views are generated from the vocabulary — zero-cost wrappers over the stored object,
carrying the documentation the standard itself carries.

```rust
use cim_rs::prelude::*;
use cim_rs::cgmes3::{SCHEMA, views::{Terminal, SynchronousMachine}};

fn report(grid: &Dataset) {
    for terminal in grid.view::<Terminal>() {
        if let Some(equipment) = terminal.conducting_equipment_in(grid) {
            println!("{:?} on {:?}, connected={:?}",
                     terminal.name(), equipment.name(), terminal.connected());
        }
    }

    // Enumerations resolve to schema literals, not strings.
    for machine in grid.view::<SynchronousMachine>() {
        if let Some(kind) = machine.type_() {
            println!("{:?}: {}", machine.name(), SCHEMA.enum_value(kind).name);
        }
    }
}
```

`view::<T>()` walks the per-class buckets of `T` and its subclasses, so asking for every
`ACLineSegment` costs their number rather than the model's.

Associations are stored as identifiers, because IEC 61970-501 serializes exactly one side
of each. `x()` returns a `TypedRef`, `x_in(&dataset)` resolves it without allocating, and
`InverseIndex` turns repeated reverse lookups into hash lookups — see
[Concepts](@/docs/concepts.md#associations-are-identifiers-not-pointers).

## Check it

```rust
use cim_rs::prelude::*;
use cim_rs::ValidateOptions;

fn check(grid: &Dataset) -> bool {
    let report = grid.validate_with(&ValidateOptions::thorough());
    for (rule, count) in report.summary() {
        println!("{rule}: {count}");     // CIM0006: 43
    }
    !report.has_errors()
}
```

Every finding carries a stable rule code, a severity, and the object, class, attribute and
source file it concerns — so a pipeline can filter on a class of problem without matching
message text. The catalogue is in [Validation](@/docs/validation.md).

## From a shell

Everything above has a command-line equivalent:

```bash
cim info     MicroGrid-BE/
cim validate MicroGrid-BE/ --rule CIM0007
cim export   MicroGrid-BE/ --out out/
```

See [Command line](@/docs/cli.md).

## Runnable examples

The repository carries four, one task each. `build_model` needs no input files at all:

```bash
cargo run --example build_model --features cgmes3                  # build a model, write CIM/XML
cargo run --example inspect     --features cgmes3 -- <model-dir>   # walk it with typed views
cargo run --example to_rdf      --features cgmes3 -- <model-dir> EQ
cargo run --example changes     --features cgmes3 -- <base> <target>
```
