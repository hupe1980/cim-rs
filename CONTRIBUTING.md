# Contributing to cim-rs

Everything here is about working *on* the crate. For using it, start at the
[README](README.md) and the [guide](https://hupe1980.github.io/cim-rs/).

## Getting set up

The standards artifacts are **not vendored** — `specs/` is gitignored and fetched. Every
repository task is a subcommand of one program:

```bash
cargo xtask fetch-specs         # ENTSO-E RDFS + SHACL + conformity models -> specs/
cargo xtask codegen             # RDFS -> src/generated/
cargo xtask codegen --check     # CI gate: committed sources match the vocabularies
cargo xtask inspect             # summarise every parsed vintage
cargo xtask shacl [--vintage cgmes2]   # RDF export vs. ENTSO-E's published shapes (needs pyshacl)
cargo xtask crossvalidate       # PowSyBl and rdflib read our output (needs Docker)

cargo test --workspace --all-features
cargo bench -p cim-rs           # throughput, synthetic and (if fetched) RealGrid
```

`xtask` owns the artifact list as well as the vintage table, so `fetch-specs` finishes by
asking the generator whether every vocabulary file it needs actually arrived.

Running SHACL stays somebody else's job: there is no mature SHACL engine in Rust, so
`cargo xtask shacl` drives [`pyshacl`](https://github.com/RDFLib/pySHACL) and reads its
report. Point `PYSHACL` at a virtualenv if the binary is not on `PATH`:

```bash
python3 -m venv .venv && .venv/bin/pip install pyshacl
PYSHACL=.venv/bin/pyshacl cargo xtask shacl
```

## Layout

```text
./                 the published crate — the repository root *is* the crate
  src/
    schema.rs      static schema metadata: ClassDef, AttrDef, EnumDef, Mult, AttrKind
    error.rs       Error, Diagnostic, Report, and the CIM0001–CIM0022 rule codes
    mrid.rs        Mrid — UUID identity per 61970-552, lenient about real-world files
    value.rs       Value — the attribute value domain, with tolerant lexical parsing
    object.rs      Object + Slot + Compound — sparse storage with per-value provenance
    dataset.rs     Dataset — object store, mRID index, per-class and per-file indexes
    header.rs      md:FullModel, dm:DifferenceModel, statements
    reader.rs      streaming CIM/XML pull parser
    writer.rs      deterministic CIM/XML writer, profile- and file-filtered
    rdf.rs         N-Triples / Turtle writer, literals typed from the profile
    diff.rs        change sets computed from two model states
    validate.rs    structural checks with stable rule codes
    view.rs        TypedView / TypedRef machinery
    xml.rs         what XML and RDF/XML can represent: the Char production, NCName, IRIs
    load.rs        whole-model loading, difference application, zip, per-profile export
    bin/cim.rs     the command line, behind the `cli` feature
    generated/     per vintage: schema tables, named constants, typed views
  tests/           corpus sweeps, serialization, round-trip, robustness, assembly;
                   cli.rs drives the binary as a process, docs.rs checks the prose
  benches/         throughput, synthetic and (where fetched) RealGrid
xtask/             repo-internal, never published
  specs.rs         the artifact list: what `fetch-specs` downloads, and its checksums
  rdfs.rs          reader for the flat RDF/XML dialect CIM RDFS uses
  ir.rs            RDFS -> language-agnostic schema IR
  emit.rs          IR -> Rust source (rustfmt-formatted, committed)
  vintage.rs       which RDFS files make up each vintage
  shacl.rs         drives pyshacl over the per-profile RDF export
  crossvalidate.rs drives the containerised PowSyBl and rdflib harnesses
site/              the documentation guide (Zola), deployed to GitHub Pages
fuzz/              cargo-fuzz targets, a separate package outside the workspace
crossvalidate/     Dockerfiles for the PowSyBl (Java) and rdflib (Python) harnesses
```

The crate is the **root package** rather than a member under `crates/`: it is the only
published one, and at the root the repository README and the crate's front page are one file
instead of two that can drift. `exclude` in `Cargo.toml` keeps the generator, the fuzz
targets, the interop harnesses and the standards artifacts out of the published tarball, so
"one crate, nothing else" is enforced by `cargo package` in CI rather than by directory
layout.

Generated sources **are** committed: builds stay reproducible and fast for downstream users,
docs.rs works, and a schema change appears as a reviewable diff.

## Adding a schema vintage

Add an entry to `xtask/src/vintage.rs` — which RDFS files it is made of and, where the
vocabularies do not say so themselves, the profile IRIs their instance files declare — then
run `cargo xtask codegen`. Nothing in the hand-written core names a class, so a new vintage
is a regeneration rather than a rewrite.

## The `specs/` corpus

All standards inputs live in `specs/`, fetched by `cargo xtask fetch-specs` (idempotent,
cached, pinned refs, SHA-256 `MANIFEST.txt`):

| Path | Content | License |
|---|---|---|
| `application-profiles-library/` | ENTSO-E RDFS + SHACL + PROF for CGMES 3.0 (incl. Header AP, beta 501-Ed2 RDFS), CGMES 2.4 archive, NC profiles — pinned tag | Apache-2.0 |
| `cgmes-2.4.15/` | CGMES 2.4.15 RDFS (2016) + 2020 component refresh | Apache-2.0 (UCAIug/ENTSO-E) |
| `test-models/cas-2.0/` | MicroGrid, MiniGrid, SmallGrid, RealGrid, FullGrid conformity test configurations | CC BY-SA 4.0 |
| `test-models/cas-3.0.3/` | CGMES 3.0 conformity assessment test configurations | CC BY-SA 4.0 |
| `test-models/qocdc-3.2.1/` | ENTSO-E QoCDC quality-gate models — real TSO exports, conformant and deliberately not; swept by `tests/qocdc.rs` | CC BY-SA 4.0 |
| `docs/` | Public ENTSO-E PDFs: technical specs, RDF syntax user guide, implementation Q&A | public ENTSO-E docs |

The licensing consequence is load-bearing: CC BY-SA 4.0 test models are a **local and CI
corpus only** and are never redistributed inside the published crate; Apache-2.0 RDFS inputs
feed codegen, and the resulting Rust source carries the UCAIug attribution headers. IEC
standard *documents* are paywalled and deliberately absent — nothing in the build or test
pipeline may depend on them.

## Tests

`cargo test --workspace --all-features` is the whole suite. Corpus-backed tests skip cleanly
when `specs/` is absent, so a fresh clone is green.

Robustness is covered two ways: `tests/robustness.rs` runs a deterministic mutation campaign
(truncation at every byte, single-byte corruption, region deletion) on stable as part of
`cargo test`, and `fuzz/` holds `cargo-fuzz` targets for longer runs on nightly.

`tests/qocdc.rs` sweeps ENTSO-E's quality-check corpus — 100 model sets of real TSO output,
conformant and deliberately not — which is the complement of the conformity models: those
are correct by construction.

Fidelity is covered by three censuses in `tests/common/mod.rs`, each blind to what the next
one sees: `element_census` counts objects by class and identity style, `identifier_census`
compares identifier text, and `value_census` compares the text of every value. A model-level
round trip cannot stand in for any of them — it compares parsed values, so a number
re-exported in a different notation is equal to itself on both sides of the assertion.

Every Rust example in the README and on the site is compiled as a doctest, so documentation
whose code stops building fails `cargo test`. `tests/docs.rs` covers what a doctest cannot:
that every `#`-anchor resolves and that snippets name the library `cim_rs`.

`tests/concepts.rs` will look like it tests nothing. It guards the maintainers' internal
architecture notes, which live in a gitignored `concepts/` directory and are therefore
absent from a clone and from CI — so it skips there, and runs on the only machine that can
break them: contiguous `D`/`R` numbering, no dangling citation or link between the
documents, and the counts their prose states matching the entries there are.

Examples are four runnable programs, one task each; `build_model` needs no input at all:

```bash
cargo run --example build_model --features cgmes3
cargo run --example inspect     --features cgmes3 -- <model-dir>
cargo run --example to_rdf      --features cgmes3 -- <model-dir> EQ
cargo run --example changes     --features cgmes3 -- <base> <target>
```

## What runs when

`.github/workflows/ci.yml` is every push: the suite without the corpus, the feature matrix,
the corpus-backed conformance job, SHACL, cross-validation, `wasm32`, MSRV, a short fuzz run
and `cargo-deny`. `scheduled.yml` is weekly — all four QoCDC archives instead of the two the
per-push sweep can afford, and fifteen minutes per fuzz target instead of sixty seconds.
Neither bound in CI is a claim about what is worth checking; both are about what belongs in
a two-minute build.

## The site

`site/` is a [Zola](https://www.getzola.org/) site, built and deployed to GitHub Pages by
`.github/workflows/site.yml`:

```bash
cd site && zola serve      # http://127.0.0.1:1111
cd site && zola check      # every internal and external link
```

## Releasing

`CHANGELOG.md` leads with the version in the manifest — `tests/docs.rs` checks that, since
the release workflow compares the tag against the manifest and would not notice a changelog
left behind.

A tag `v<version>` is the whole trigger. The workflow's shape follows from one fact:
**publishing to crates.io cannot be undone** — a version can be yanked but never reused — so
everything that can say no runs before anything irreversible.

The tag is checked against the manifest first, because it costs nothing and is the mistake
most easily made. Then the CI gates again under `--locked`, since a tag can be pushed at a
commit CI never saw; then `cargo publish --dry-run`; then the `cim` binaries for four
targets — Linux on both architectures, macOS on Apple silicon, Windows on x86-64 — each with
a SHA-256 beside it. Only then does the publish happen.

`workflow_dispatch` runs everything except the two publish steps, so the release path can be
exercised without spending a version number to find out whether it works.
