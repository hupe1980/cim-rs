# cim-rs

**A Rust implementation of the IEC Common Information Model for power systems.**

[![CI](https://github.com/hupe1980/cim-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/cim-rs/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

`cim` reads, navigates, validates and writes CIM grid models — principally **CGMES 3.0**
(IEC TS 61970-600-1/-2:2021), the format European transmission system operators exchange
grid models in.

It reads the complete published ENTSO-E CGMES 3.0 conformity assessment corpus — 31 model
sets, 361 files, 250,698 objects — **without a single diagnostic**, and round-trips every
one of them without losing a value.

```rust,no_run
use cim::prelude::*;
use cim::cgmes3::{SCHEMA, views::ACLineSegment};

// A CGMES model is a *set* of profile files describing the same objects.
let ds = Dataset::load(SCHEMA, [
    "model_EQ.xml",   // equipment: ratings, connectivity
    "model_SSH.xml",  // steady state hypothesis: dispatch, switch positions
    "model_TP.xml",   // topology
    "model_SV.xml",   // state variables: power flow results
])?;

for line in ds.view::<ACLineSegment>() {
    let kv = line.base_voltage_in(&ds).and_then(|bv| bv.nominal_voltage());
    println!("{:?}  {kv:?} kV  r={:?} x={:?}", line.name(), line.r(), line.x());
}

let report = cim::validate::validate(&ds);
println!("{} findings", report.len());
# Ok::<(), cim::Error>(())
```

## Why this exists

The IEC CIM is the semantic model for exchanging power-grid data between EMS, DMS, market
and planning systems. The mature tooling is Java (PowSyBl), C++ (libcimpp) and Python
(PyCIM, CIMpy, pycgmes). Rust had no established implementation — yet Rust is exactly
where the gap hurts: high-throughput model servers, embedded and edge grid controllers,
WASM browser tooling, and safety-critical pipelines all want a fast, memory-safe,
dependency-light CIM core.

## What makes it correct

CGMES has three properties that a naive implementation gets wrong. `cim` is built around
all three.

**A model is a set of files, not a file.** The same `SynchronousMachine` gets its ratings
from EQ and its dispatch from SSH, under one identifier. [`Dataset`] merges objects by
mRID across files, so load order does not matter and nothing is duplicated.

**Attributes are declared far more widely than they are used.** `IdentifiedObject.mRID` is
declared in ten of eleven CGMES 3.0 profiles. Deciding what to write from declarations
alone repeats every object in every profile file — in one measured case inflating a
112 MiB model to 399 MiB. `cim` records, per value, **which profile's file it came from**,
so an export reproduces the original file set.

**Inheritance crosses profile boundaries.** The Steady State Hypothesis profile declares
`Equipment.inService` without declaring `BusbarSection`, and real SSH files nonetheless
set `inService` on busbar sections. Export decisions are made from what an object actually
carries, never from class-level profile membership.

## Design

Two layers in one crate:

```text
cim
├── generated from the official RDFS vocabularies
│   ├── schema tables      443 classes, 3,624 attributes, 51 enums, 36 datatypes
│   ├── typed views        zero-cost accessors per class
│   └── named constants    ClassId / AttrId / EnumValueId, spelled as the standard does
└── hand-written, schema-agnostic
    ├── Dataset            object store, mRID index, multi-profile merge
    ├── reader / writer    streaming CIM/XML (IEC 61970-552)
    ├── header             md:FullModel, dm:DifferenceModel
    └── validate           structural checks with stable rule codes
```

Objects are stored **sparsely** — only attributes actually present are kept. That is a
match for the data, not a compromise: a `Terminal` has around thirty possible attributes
and an SSH file sets exactly one. Typed access comes from generated zero-cost views rather
than a generated struct per class, which keeps profile merging natural, memory
proportional to real content, and compile times low.

Everything model-specific is generated; everything else is written against the `schema`
interface. **Adopting a new CIM vintage is a regeneration, not a rewrite.**

## Performance

ENTSO-E `RealGrid` conformity model, release build, Apple M-series:

| | |
|---|---|
| Read | 112 MiB in **0.45 s** (248 MiB/s), 188,547 objects |
| Write | 110 MiB in **0.36 s** (304 MiB/s) |
| Validate | 1.1M values in **0.04 s** |
| Inverse index | **0.02 s** |

Reproduce with `cargo run --release --example bench -- <model directory>`.

## Installation

```toml
[dependencies]
cim = "0.1"
```

Optional features:

| Feature | Effect |
|---|---|
| `zip` | Read and write CGMES model sets packaged as zip archives |

## Usage

### Loading and inspecting

```rust,no_run
use cim::prelude::*;
use cim::cgmes3::SCHEMA;

let mut ds = Dataset::new(SCHEMA);
let load = ds.load_files(["EQ.xml", "SSH.xml"], &ReadOptions::lenient())?;

// Nothing is ever silently dropped: deviations become structured diagnostics.
for d in load.report.iter() {
    println!("{d}");   // warning[CIM0002] EQ.xml: unknown property <cim:Vendor.x>, skipped
}
# Ok::<(), cim::Error>(())
```

`ReadOptions::lenient()` is the default because published models carry vendor extensions
and identifier deviations. `ReadOptions::strict()` turns anything the schema does not
define into a hard error.

### Typed navigation

```rust
use cim::prelude::*;
use cim::cgmes3::{SCHEMA, views::{Terminal, SynchronousMachine}};

# let ds = Dataset::new(SCHEMA);
for t in ds.view::<Terminal>() {
    if let Some(eq) = t.conducting_equipment_in(&ds) {
        println!("{:?} on {:?}, connected={:?}", t.name(), eq.name(), t.connected());
    }
}

// Enumerations resolve to schema literals, not strings.
for m in ds.view::<SynchronousMachine>() {
    if let Some(kind) = m.type_() {
        println!("{:?}: {}", m.name(), SCHEMA.enum_value(kind).name);
    }
}
```

Associations are stored as identifiers, because IEC 61970-501 serializes exactly one side
of each. `x()` returns a `TypedRef`, `x_in(&dataset)` resolves it, and `InverseIndex`
turns repeated reverse lookups into hash lookups.

### Validation

```rust
use cim::prelude::*;
use cim::cgmes3::SCHEMA;
use cim::validate::{self, ValidateOptions};

# let ds = Dataset::new(SCHEMA);
let report = validate::validate_with(&ds, &ValidateOptions::thorough());
for (rule, count) in report.summary() {
    println!("{rule}: {count}");     // CIM0006: 43
}
if report.has_errors() { /* the model is not conforming */ }
```

Every finding carries a stable rule code (`CIM0001`–`CIM0015`), a severity, and the
object, class, attribute and source file it concerns — so CI can filter and fail on
specific classes of problem without matching message text.

Checks cover what the RDFS vocabulary justifies: cardinality, datatypes, reference
targets, reference resolution, identifier conformance, profile reach, abstract
instantiation, and header structure. Semantic rules that ENTSO-E publishes as **SHACL
shapes are out of scope** — run those with a SHACL engine against the same data.

### Writing

```rust,no_run
use cim::prelude::*;
use cim::cgmes3::SCHEMA;

# let ds = Dataset::new(SCHEMA);
# let dir = std::path::Path::new(".");
// One file per profile the model carries data for.
let written = ds.save_all_profiles(dir, "MyModel")?;
# Ok::<(), cim::Error>(())
```

Output is deterministic — objects in mRID order, attributes in schema order — so
re-writing an unchanged model is byte-identical and version diffs stay readable.

### Difference models

```rust,no_run
use cim::prelude::*;
use cim::cgmes3::SCHEMA;

# let mut ds = Dataset::new(SCHEMA);
# let bytes: &[u8] = b"";
if let Some(diff) = cim::reader::read_difference(SCHEMA, bytes, None)? {
    let report = ds.apply_difference(&diff);   // retract reverse, assert forward
    println!("{} findings", report.len());
}
# Ok::<(), cim::Error>(())
```

## Conformance

| Standard | Role |
|---|---|
| IEC 61970-301:2020+AMD1:2022 | CIM base semantics (CIM17) |
| IEC 61970-501 | RDFS profile representation — the codegen input |
| IEC 61970-552:2016 | CIM/XML format: identity, headers, difference models |
| IEC TS 61970-600-1/-2:2021 | CGMES 3.0 profile set — the interop target |

Profiles supported: **EQ, EQBD, OP, SC, SSH, TP, SV, DL, GL, DY**, plus the header
vocabulary.

## Development

The standards artifacts are **not vendored**. Fetch them, then regenerate:

```bash
scripts/fetch-specs.sh          # ENTSO-E RDFS + SHACL + conformity models -> specs/
cargo xtask codegen             # RDFS -> crates/cim/src/generated/
cargo xtask codegen --check     # CI gate: committed sources match the vocabularies
cargo xtask inspect             # summarise the parsed schema
cargo test --workspace --all-features
```

`specs/` is gitignored. Generated sources **are** committed: builds stay reproducible and
fast for downstream users, docs.rs works, and a schema change appears as a reviewable
diff. Tests that need the corpus skip cleanly when it is absent, so a fresh clone is green.

See [CONCEPT.md](CONCEPT.md) for the full design rationale and roadmap.

## Licensing and attribution

This crate is licensed **MIT OR Apache-2.0**.

The RDFS vocabularies the generated code is derived from are published by ENTSO-E and the
UCA International Users Group under the **Apache License 2.0**; generated files carry that
attribution. The IEC standard *documents* are copyrighted by the IEC, are not redistributed
here, and nothing in the build or test pipeline depends on them.

The ENTSO-E conformity test models are licensed **CC BY-SA 4.0** and owned by ENTSO-E. They
are used as a local test corpus only and are never redistributed with this crate.
