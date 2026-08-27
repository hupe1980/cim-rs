# cim-rs

**A Rust implementation of the IEC Common Information Model for power systems.**

[![CI](https://github.com/hupe1980/cim-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/cim-rs/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-hupe1980.github.io%2Fcim--rs-b8410f)](https://hupe1980.github.io/cim-rs/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#licensing-and-attribution)

`cim-rs` reads, navigates, validates and writes CIM grid models in both profile sets
European transmission system operators use: **CGMES 3.0** (IEC TS 61970-600-1/-2:2021) and
**CGMES 2.4.15**.

It reads both published ENTSO-E conformity assessment corpora — 31 CGMES 3.0 model sets and
52 CGMES 2.4.15 archives, **523,842 objects in total** — without a single error, and writes
every one of them back **file for file, object for object, in the same serialized form** the
published models use.

It also exports the model as **ordinary RDF with every value typed from the profile**, so a
CGMES dataset can be handed to a triple store or validated against ENTSO-E's own SHACL
shapes — which CIM/XML itself cannot be — and it **computes** the `dm:DifferenceModel`
between two model states, not only reads and applies one.

📖 **[Documentation](https://hupe1980.github.io/cim-rs/)** ·
🦀 **[API reference](https://docs.rs/cim-rs)**

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::{SCHEMA, views::ACLineSegment};

// A CGMES model is a *set* of profile files describing the same objects.
// `Dataset::open` detects the vintage from the files; name SCHEMA to fix it.
let grid = Dataset::load_dir(SCHEMA, "MicroGrid-BE")?;

for line in grid.view::<ACLineSegment>() {
    let kv = line.base_voltage_in(&grid).and_then(|bv| bv.nominal_voltage());
    println!("{:?}  {kv:?} kV  r={:?} x={:?}", line.name(), line.r(), line.x());
}

// Structural checks, against the profile rather than against a guess.
let report = grid.validate();
println!("{} findings", report.len());
# Ok::<(), cim_rs::Error>(())
```

## Install

```bash
cargo add cim-rs                       # the library
cargo install cim-rs --features cli    # the `cim` command line
```

The package is `cim-rs`; the library it provides is `cim_rs`. (`cim` on crates.io has been
taken since 2022 by an unrelated tool.)

| Feature | Effect |
|---|---|
| `cgmes3` *(default)* | CGMES 3.0 schema, typed views and named constants |
| `cgmes2` | CGMES 2.4.15 schema, typed views and named constants |
| `zip` | Read and write model sets packaged as zip archives |
| `cli` | The `cim` command line (implies `zip`) |

Vintages are independent modules: enabling only the one you need keeps compile time and
binary size down. Both can be on at once — identifiers are per-vintage, so they cannot be
mixed by accident.

## From a shell

```bash
cim info     MicroGrid-BE/                  # files, object counts, profile coverage, findings
cim validate MicroGrid-BE/ --rule CIM0007   # exits 1 on any error
cim export   MicroGrid-BE/ --out out/       # write the model back as the file set it came from
cim rdf      MicroGrid-BE/ --out graphs/    # one typed RDF graph per profile with data
cim diff     before/ after/ > change.xml    # the change set between two model states
```

Six subcommands over a model set given as files, archives or directories, adding no
dependency of its own — see [the command-line reference][cli].

Where a command's result is a document, the document is the whole of standard output and
everything else goes to stderr, so the redirection above yields a file and not a file with a
report appended to it.

## Why this exists

The IEC CIM is the semantic model for exchanging power-grid data between EMS, DMS, market
and planning systems. The mature tooling is Java (PowSyBl), C++ (libcimpp) and Python
(PyCIM, CIMpy, pycgmes). Rust had no established implementation — yet Rust is exactly where
the gap hurts: high-throughput model servers, embedded and edge grid controllers, WASM
browser tooling, and safety-critical pipelines all want a fast, memory-safe,
dependency-light CIM core.

## What makes it correct

CGMES has a handful of properties that a plausible-looking implementation gets wrong.
`cim-rs` is built around all of them, and each is enforced by a test against the published
conformity models.

* **A model is a set of files, not a file.** The same `SynchronousMachine` gets its ratings
  from EQ and its dispatch from SSH, under one identifier. Objects merge by mRID, so load
  order does not matter and nothing is duplicated. The one thing merging cannot decide is a
  contradiction, so that is recorded and reported (`CIM0018`) rather than resolved silently.
* **An identifier is a UUID, not the way somebody spelled it.** Published files write UUIDs
  without hyphens, and references as absolute IRIs. Read as opaque text, those objects lose
  their `urn:uuid:` name in RDF and split in two the moment another file spells the same
  UUID the conforming way.
* **Attributes are declared far more widely than they are used.** `IdentifiedObject.mRID`
  is declared in ten of eleven profiles; writing from declarations alone inflated a 112 MiB
  model to 399 MiB. Each value records which file it came from.
* **A file may only name classes its own profile declares.** An SSH file writes
  `<cim:Equipment rdf:about="…">` for an `ACLineSegment` — and never a class whose mandatory
  attributes the data does not supply.
* **`rdf:ID` versus `rdf:about` is a property of the class within the profile**, not of the
  file. The RDFS says which is which; guessing from the profile keyword rewrote 49,255
  identifiers in one published file.
* **Compounds are values, not references.** `Location.mainAddress` holds a `StreetAddress`
  inline, and those nest. A parser that treats the nested elements as text does not fail —
  it fabricates a value.
* **A document says which vintage it is.** `xmlns:cim=".../CIM100#"` is CGMES 3.0,
  `".../CIM-schema-cim16#"` is 2.4.15. `reader::sniff` reads that off the root element and
  the `cim` tool uses it by default, because reading a model against the wrong vintage
  resolves no class at all — an empty model and an exit status of zero, which looks like a
  clean load. A mismatch is reported as `CIM0021` rather than left to be inferred.
* **Tolerance costs a report.** Lenient reading is the default because published models
  deviate, and everything the reader tolerates it also has to *say* — so the report is
  bounded, and says where it stopped.
* **Output has to satisfy someone else's parser.** Every document is checked against the XML
  and XML Namespaces recommendations, and every RDF export against the N-Triples grammar,
  across the whole published corpus. And by someone else's *implementation*: **PowSyBl**
  builds the identical network from a `cim-rs` re-export as from the published originals —
  same counts, same buses, same digest over every identifier — and **rdflib** parses the
  RDF export as a graph. Both run in CI. A writer and a reader that share a misconception
  agree with each other perfectly; this is what rules that out.
* **Some things only the *syntax* forbids.** A value cannot hold a character XML 1.0 has no
  representation for — most of the C0 range, escapes included — and `rdf:ID` is an `NCName`,
  which an identifier a producer chose freely need not be. Neither can happen in a
  conforming model, both arrive from a mis-encoded file, and neither is caught by an
  ordinary well-formedness check — such a document *is* well-formed XML. Reported as
  `CIM0019` and `CIM0020`; never written.

The reasoning behind each rule is in [the documentation][conformance] and in the rustdoc
next to the code it governs.

## Standard RDF, with the datatypes CIM/XML omits

CIM/XML is not RDF/XML: it predates the W3C recommendation, `rdf:parseType="Statements"` is
not RDF syntax, and `rdf:ID="_x"` denotes `urn:uuid:x` rather than a document fragment. The
larger gap is that **it carries no datatype information at all** — every value is element
text — while ENTSO-E's SHACL shapes constrain 3,137 properties by `sh:datatype`. ENTSO-E's
own interoperability reporting names the missing piece: *"there are no open libraries to
natively enhance the data based on the profile definitions."*

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;
use cim_rs::rdf::{RdfOptions, Syntax};

# let grid = Dataset::load(SCHEMA, ["EQ.xml"])?;
// One profile's graph, as a SHACL engine wants it.
let eq = SCHEMA.profile_by_keyword("EQ").unwrap();
let turtle = cim_rs::rdf::to_string(&grid, &RdfOptions::new(Syntax::Turtle).profiles(eq.mask()))?;
# Ok::<(), cim_rs::Error>(())
```

```turtle
<urn:uuid:0472a783-c766-11e1-8775-005056c00008>
    a cim:ACLineSegment ;
    cim:IdentifiedObject.name "BE-Line_1" ;
    cim:ACLineSegment.r "2.2"^^xsd:float ;
    cim:Equipment.aggregate "false"^^xsd:boolean ;
    cim:ConductingEquipment.BaseVoltage <urn:uuid:5dc9b970-cc86-4a2b-9e1a-0e2c8b0e6e12> .
```

Exports are **per profile**, because ENTSO-E's shapes are — objects, element classes and the
files' headers alike, so a Steady State Hypothesis graph is what an SSH file is and nothing
more. A profile the model says nothing about gets no graph rather than one made of headers.
`cargo xtask shacl` runs the whole thing against the published shapes with `pyshacl`, in CI;
every profile that carries data, in every published CGMES 3.0 conformity model, conforms.
[More][rdf]

## Difference models

IEC 61970-552's incremental exchange: the statements to retract, then the statements to
assert. `cim-rs` reads one, applies one, writes one — and **computes** one, which is the
operation an EMS performs every time it publishes an update.

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;
use cim_rs::diff::DiffOptions;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let base = Dataset::load_dir(SCHEMA, "before")?;
let updated = Dataset::load_dir(SCHEMA, "after")?;

let change = base.difference_to(&updated, &DiffOptions::default());
cim_rs::writer::write_difference(SCHEMA, &change.model, std::io::stdout(), &Default::default())?;
# Ok(()) }
```

Applying a computed change set to its base reproduces the target exactly — every value
present, none left over, class changes included. That is a test against the published
conformity models rather than a claim. [More][diff]

## Design

```text
cim_rs
├── generated from the official RDFS vocabularies, one module per vintage
│   ├── schema tables      classes, attributes, enums, datatypes, profiles
│   ├── typed views        zero-cost accessors per class
│   └── named constants    ClassId / AttrId / EnumValueId, spelled as the standard does
└── hand-written, schema-agnostic
    ├── Dataset            object store, mRID index, multi-profile merge
    ├── reader / writer    streaming CIM/XML (IEC 61970-552)
    ├── rdf                N-Triples / Turtle, typed from the profile
    ├── header             md:FullModel, dm:DifferenceModel
    ├── diff               change sets computed from two model states
    └── validate           structural checks with stable rule codes
```

| Vintage | Feature | Namespace | Classes | Attributes | Enums | Profiles |
|---|---|---|---|---|---|---|
| CGMES 3.0 | `cgmes3` *(default)* | `CIM100` + `eu` | 443 | 3,624 | 51 | 11 |
| CGMES 2.4.15 | `cgmes2` | `cim16` + `entsoe` | 401 | 3,539 | 48 | 12 |

Objects are stored **sparsely** — only attributes actually present are kept. That is a match
for the data, not a compromise: a `Terminal` has around thirty possible attributes and an
SSH file sets exactly one. Typed access comes from generated zero-cost views rather than a
struct per class, which keeps profile merging natural, memory proportional to real content,
and compile times low.

Everything model-specific is generated; everything else is written against the `schema`
interface. **Adopting a new CIM vintage is a regeneration, not a rewrite** — and CGMES
2.4.15 is the proof: a different namespace, a different extension prefix, an extra boundary
profile, and vocabularies that predate the self-describing ontology header, all handled
without a line of vintage-specific runtime code. [More][concepts]

## Performance

ENTSO-E `RealGrid` conformity model — 112 MiB, 188,547 objects, 1.1M values — release build,
Apple M-series. Documents are read from memory, so the numbers measure parsing.

| | |
|---|---|
| Read | **0.33 s** (343 MiB/s, 576k objects/s) |
| Write, as the file set it came from | **0.34 s** (335 MiB/s) |
| Write as RDF (N-Triples, 1.3M triples) | **0.55 s** |
| Validate | **0.04 s** (4.5M objects/s) |
| Diff against another model state | **0.07 s** (2.6M objects/s) |
| Inverse index | **0.03 s** |

Reproduce with `cargo bench -p cim-rs`; the benchmark also measures a synthetic model, so it
runs without the standards corpus. Absolute figures move by 20% or more with machine load,
so treat the ratios between rows as the stable part. [More][perf]

## Conformance

| Standard | Role |
|---|---|
| IEC 61970-301:2020+AMD1:2022 | CIM base semantics (CIM17) |
| IEC 61970-501:2006 | RDFS profile representation — the codegen input |
| IEC 61970-552:2016 | CIM/XML format: identity, headers, difference models |
| IEC TS 61970-600-1/-2:2021 | CGMES 3.0 profile set |

Profiles supported: **EQ, OP, SC, EQBD, SSH, TP, SV, DL, GL, DY** and the header vocabulary
in both vintages, plus **TPBD** in CGMES 2.4.15.

Checked against the published models on every `cargo test` with the corpus present: reading
both corpora with zero errors, per-file re-export, well-formedness under a conforming XML
parser, the RDF export against the N-Triples grammar and ENTSO-E's SHACL shapes, semantic
round-trip, difference models in both directions, pinned validation findings, and a
deterministic mutation campaign, and cross-validation against PowSyBl (Java) and rdflib
(Python) in pinned containers on both vintages. The library also builds for
`wasm32-unknown-unknown` in CI, so "pure Rust, no C dependencies" is a build rather than a
claim. 220 tests; corpus-backed tests skip cleanly on a fresh clone.
[The full record][conformance]

## Development

The standards artifacts are **not vendored**; `specs/` is gitignored and fetched, and
generated sources are committed. Every repository task is a subcommand of one program:

```bash
cargo xtask fetch-specs         # ENTSO-E RDFS + SHACL + conformity models -> specs/
cargo xtask codegen             # RDFS -> src/generated/
cargo test --workspace --all-features
```

Repository layout, adding a schema vintage, the `specs/` corpus and its licences, the
interop harnesses and the release process are in [CONTRIBUTING.md](CONTRIBUTING.md).

## Licensing and attribution

This crate is licensed **MIT OR Apache-2.0**.

The RDFS vocabularies the generated code is derived from are published by ENTSO-E and the
UCA International Users Group under the **Apache License 2.0**; generated files carry that
attribution. The IEC standard *documents* are copyrighted by the IEC, are not redistributed
here, and nothing in the build or test pipeline depends on them.

The ENTSO-E conformity test models are licensed **CC BY-SA 4.0** and owned by ENTSO-E. They
are used as a local test corpus only and are never redistributed with this crate.

[cli]: https://hupe1980.github.io/cim-rs/docs/cli/
[rdf]: https://hupe1980.github.io/cim-rs/docs/rdf/
[diff]: https://hupe1980.github.io/cim-rs/docs/difference-models/
[concepts]: https://hupe1980.github.io/cim-rs/docs/concepts/
[perf]: https://hupe1980.github.io/cim-rs/docs/performance/
[conformance]: https://hupe1980.github.io/cim-rs/docs/conformance/
