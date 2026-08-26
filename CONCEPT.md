# cim-rs — Concept

**A best-in-class, single-crate Rust implementation of the IEC Common Information Model (CIM) for power systems.**

Status: **M0–M2 implemented** (see [Roadmap](#8-roadmap)) · License: MIT OR Apache-2.0 · Edition: Rust 2024

> This document records the design rationale and the decisions behind it. For usage, see
> the [README](README.md).

---

## 1. Problem & Motivation

The IEC CIM (IEC 61970 / 61968 / 62325) is *the* semantic model for exchanging power-grid
data between EMS, DMS, market and planning systems. In Europe, every TSO exchanges grid
models as **CGMES** (Common Grid Model Exchange Standard, IEC 61970-600) instance files —
RDF/XML documents containing tens of thousands of interlinked objects.

The mature tooling is Java (PowSyBl), C++ (libcimpp) and Python (PyCIM, CIMpy, and the
cimgen-generated [pycgmes](https://pypi.org/project/pycgmes/)). Rust has **no established
CIM implementation** — only an early
multi-crate experiment ([cimoxide](https://github.com/m-mirz/cimoxide)) with negligible
adoption. Yet Rust is exactly where the gap hurts: high-throughput model servers,
embedded/edge grid controllers, WASM-based browser tooling, and safety-critical pipelines
all want a fast, memory-safe, dependency-light CIM core.

`cim-rs` fills that gap: a **single crate** providing typed CIM data models, standards-
compliant CIM/XML (RDF) reading and writing, profile-aware validation, and ergonomic
model navigation — fast enough to stream nation-scale grid models and strict enough to
pass interoperability testing.

## 2. Standards Baseline (researched & verified)

| Standard | Role | Version targeted |
|---|---|---|
| IEC 61970-301 | CIM base semantics (grid/EMS) | :2020 + AMD1:2022 → **CIM17** (namespace `CIM100`) |
| IEC 61968-11 | Distribution extensions | via canonical CIM schema |
| IEC 62325-301 | Market extensions | out of scope v1 (see Roadmap) |
| IEC 61970-501 | RDFS profile representation | :2006 — input format for codegen |
| IEC 61970-552 | CIM/XML model exchange format | :2016 — file format, headers, difference models |
| IEC 61970-600-1/-2 | **CGMES 3.0** profile set | :2021 — primary interop target |
| CGMES 2.4.15 | Legacy profile set (CIM16) | planned (M4); the design cost is already paid |

Notes that shape the design:

- **CIM18** (IEC 61970-301 Ed. 7) is in development at WG13 but unpublished; the
  architecture must make adopting a new schema vintage a *regeneration*, not a rewrite.
- **IEC 61970-501 Ed. 2 is in CD stage**: ENTSO-E's profile library already ships beta
  RDFS artifacts in the new Ed.2 serialization alongside the current "RDFS2020" form —
  the codegen frontend must abstract over RDFS serialization vintages, not hardcode one.
- **The exchange header is itself a profile**: CGMES 3.0 publishes a Header AP RDFS
  (61970-600-2), so `md:FullModel` handling can be largely generated, not hand-coded.
- **NC/RCP extension profiles** (ENTSO-E Network Code Profiles v2.4, Regional
  Coordination Processes) are published in exactly the same RDFS+SHACL form — the
  generator must treat "a profile" as data, so third-party and custom profiles work
  without touching the core.
- **CGMES 3.0 profiles**: EQ (incl. Operation & ShortCircuit), SSH, SV, TP, DL, GL, DY,
  plus boundary (EQBD). CGMES 2.4.15 additionally has TPBD. Profiles ship as separate
  instance files that cross-reference each other — the library must model *datasets*, not
  just single documents.
- **Serialization is a constrained RDF/XML subset** defined by 61970-552: `rdf:ID`/
  `rdf:about` object identity (UUID rules), `md:FullModel` headers (`Model.profile`,
  `Model.DependentOn`, `Model.scenarioTime`, …), and `dm:` difference models. We do not
  need (or want) a general RDF toolchain on the hot path.
- **Licensing is clean**: the CIM UML and generated RDFS artifacts are copyrighted by
  UCAIug and licensed **Apache 2.0** (IEC copyrights cover the standard *documents*, not
  the machine-readable model). ENTSO-E publishes CGMES RDFS + SHACL in its
  [application-profiles-library](https://github.com/entsoe/application-profiles-library).
  Generating and redistributing Rust code from these artifacts is permitted.

## 3. Goals & Non-Goals

### Goals

1. **Typed model** — generated, schema-checked typed access to the full CGMES 3.0 profile
   set, with rustdoc carried over from the standard's own class and attribute
   documentation. (Realized as zero-cost views over sparse storage rather than a struct
   per class; see [4.2](#42-mapping-cim-onto-rust--the-decision-that-shaped-everything)
   for the measurements behind that choice.)
2. **Round-trip fidelity** — read CIM/XML → object model → write CIM/XML with no
   semantic loss, including headers, difference models, boundary files and zip archives.
3. **Dataset semantics** — first-class multi-file model assembly (EQ+SSH+TP+SV+…),
   cross-profile reference resolution, and tolerant handling of dangling references.
4. **Validation** — profile-derived structural checks (cardinality, datatype, class
   membership) generated from RDFS; clear, machine-usable diagnostics.
5. **Performance** — streaming parser, cached name resolution, no per-object heap churn;
   target: parse a 100 MB model in seconds, not minutes, on commodity hardware.
   *Achieved: 112 MiB in 0.45 s.*
6. **Single crate, small surface** — one published crate `cim-rs` (lib name `cim`),
   features gating profiles and optional integrations; minimal dependency tree.
7. **Portability** — pure-Rust, `wasm32` compatible (no C deps), usable from Python/JS
   later via thin binding crates (out of scope for the crate itself).

### Non-Goals (v1)

- Power-flow / state estimation / topology processing (leave to consumers; provide the
  data layer they need).
- A general RDF store, SPARQL engine, or OWL reasoner.
- SHACL execution (we *emit* data validatable by ENTSO-E's SHACL shapes; running SHACL
  is delegated to existing tools, revisited post-v1).
- GUI / diagram rendering (we expose DL/GL data; drawing is a consumer concern).
- IEC 62325 market profiles and 61968 message envelopes (schema hooks exist; profiles
  can be generated later without architectural change).

## 4. Architecture

Two layers in one published crate:

```text
crates/cim/                        the published crate
  src/
    schema.rs      static schema metadata: ClassDef, AttrDef, EnumDef, Mult, AttrKind
    mrid.rs        Mrid — UUID identity per 61970-552, lenient about real-world files
    value.rs       Value — the attribute value domain, with tolerant lexical parsing
    object.rs      Object + Slot — sparse storage with per-value profile provenance
    dataset.rs     Dataset — object store, mRID index, multi-profile merge, InverseIndex
    header.rs      md:FullModel, dm:DifferenceModel, statements
    reader.rs      streaming CIM/XML pull parser
    writer.rs      deterministic CIM/XML writer, profile-filtered
    validate.rs    structural checks with stable rule codes
    view.rs        TypedView / TypedRef machinery
    load.rs        whole-model loading, difference application, zip, per-profile export
    generated/cgmes3/
      tables.rs    static schema tables
      names.rs     compile-time ClassId / AttrId / EnumValueId constants
      views.rs     zero-cost typed accessors, one per class
xtask/                             repo-internal, never published
  rdfs.rs          reader for the flat RDF/XML dialect CIM RDFS uses
  ir.rs            RDFS -> language-agnostic schema IR
  emit.rs          IR -> Rust source (rustfmt-formatted, committed)
```

### 4.1 Code generation

- **Input**: the official ENTSO-E RDFS vocabularies, fetched into the gitignored `specs/`
  by `scripts/fetch-specs.sh` (pinned tag, SHA-256 manifest). Nothing third-party is
  vendored.
- **Generator**: an `xtask` binary in the workspace, never published, so the published
  surface stays a single crate. It parses RDFS into an IR (classes, inheritance,
  attributes, associations with roles and cardinality, CIM datatypes, enumerations), then
  renders Rust.
- **Generated code is committed, not built by `build.rs`**: reproducible builds, fast
  `cargo build` for users, working docs.rs, reviewable diffs when a schema vintage
  changes. Output is rustfmt-formatted so `cargo fmt --check` and
  `cargo xtask codegen --check` cannot contradict each other. Both run in CI.

Parsed from CGMES 3.0: **11 profiles, 443 classes (364 concrete), 3,624 attributes,
51 enumerations with 554 literals, 36 CIM datatypes**, across the `cim:` and European
`eu:` namespaces.

### 4.2 Mapping CIM onto Rust — the decision that shaped everything

The obvious design is a generated struct per class with inheritance flattened into
fields. It was rejected after measuring the real schema, for three reasons that only
become visible in actual CGMES data.

**CIM objects are extremely sparse.** A `Terminal` has around thirty possible attributes;
a Steady State Hypothesis file sets exactly one. Across the corpus, 250,698 objects carry
1.1 million values — an average of four each, against a mean of 21 flattened fields per
concrete class. Struct-per-class would spend most of its memory on `None`.

**The same object is described by several files.** A `SynchronousMachine` gets its
ratings from EQ and its dispatch from SSH under one identifier, so a class type must hold
the union of every profile's attributes — and merging must be an operation on that union,
not a replacement.

**A heterogeneous arena of the largest variant is wasteful.** The widest class carries 76
attributes; a `Vec<AnyObject>` would pad every object to that size.

The design adopted instead:

- **Sparse dynamic storage.** `Object` holds `Vec<Slot>` sorted by `AttrId`, with repeats
  adjacent for many-valued attributes. Lookup is a partition point; memory is
  proportional to real content.
- **Generated zero-cost typed views.** `ACLineSegment<'a>(&'a Object)` with accessors
  returning `Option<f64>`, `Option<EnumValueId>` or `TypedRef<T>`. Ergonomics of typed
  structs, none of the cost.
- **Compile-time identifiers spelled as the standard spells them.** `UnitMultiplier.M`
  (mega) and `UnitMultiplier.m` (milli) differ only by case; transforming names to
  `SCREAMING_SNAKE_CASE` collides them. Constants therefore mirror the CIM identifier
  exactly, which is also how a reader of IEC 61970-301 finds them.
- **Associations as identifiers, not pointers.** IEC 61970-501 serializes exactly one
  side of each association, so the other side is derived. `TypedRef<T>` carries the target
  class in the type; `InverseIndex` makes reverse traversal a hash lookup.
- **CIM datatypes as documented units.** `Resistance` and friends serialize as their
  primitive; the schema-fixed unit and multiplier are recorded in `DatatypeDef` and
  surfaced in the generated rustdoc for each accessor.

### 4.3 Per-value profile provenance — the correctness insight

CGMES declares `IdentifiedObject.mRID` in **ten of eleven** profiles, and `name` in eight.
Deciding what a profile file should contain from declarations alone therefore repeats
almost every object in almost every file. Measured on `RealGrid`, that inflated a 112 MiB
model to **399 MiB** on export.

Nor can it be fixed by filtering on class membership: the SSH vocabulary declares
`Equipment.inService` without declaring `BusbarSection`, yet real SSH files set
`inService` on busbar sections through inheritance. Filtering by declared class membership
silently dropped that data — a bug the round-trip tests caught.

`Slot` therefore records **which profiles' files a value came from**, at no memory cost
(the mask fits in existing padding). Export writes a value to profile *P* when its
provenance includes *P*, falling back to the attribute's declared profiles only for values
set programmatically. Export then reproduces the original file set: 112 MiB in, 110 MiB
out, every value preserved.

### 4.4 Dataset layer

- Object storage with an mRID index and per-class index.
- Multi-file assembly that merges by mRID, promotes to the most specific class seen, and
  widens provenance when two files carry the same value. Load order does not matter.
- `md:Model.DependentOn` tracking, with unsatisfied dependencies reported.
- Difference models: retract every reverse statement, then assert every forward one.
- `LinkPolicy::Lenient` (default) versus `Strict` for unresolved references — real
  exchanges are routinely partial, since an SSH file references equipment defined
  elsewhere.

### 4.5 I/O

- **Reader**: a streaming pull parser on `quick-xml`, specialized to the 61970-552 RDF/XML
  subset. Element names are resolved through a cache keyed on raw QName bytes, since
  instance documents repeat a handful of names many thousands of times. The document is
  never buffered. Measured at **248 MiB/s**.
- **Writer**: deterministic — objects in mRID order, attributes in schema order — so an
  unchanged model re-writes byte-identically and version diffs stay readable. Correct
  per-namespace prefixes, `rdf:ID` versus `rdf:about` by profile convention, full headers.
- **Zip** archives behind the `zip` feature, for CGMES model sets as distributed.

### 4.6 Validation & diagnostics

Every finding is a structured `Diagnostic` with a stable rule code (`CIM0001`–`CIM0015`),
severity, and the object, class, attribute and source file it concerns — so a pipeline can
filter and fail on specific classes of problem without matching message text.

Two checks required care to be *useful* rather than merely correct:

- A required attribute is not reported missing when it is an association side the schema
  never serializes (`AssociationUsed = No`), since that side is derived by inversion.
- "Attribute not in profile" reports only data that would genuinely be **lost on export** —
  a value no loaded profile would write. Judging by declaration alone produced 9,995
  warnings on `RealGrid`, all of them true and none of them actionable.

## 5. Single-Crate Strategy & Features

One published crate keeps integration trivial, but ~1,100 generated types × 2 schema
vintages is a compile-time hazard. Mitigations:

- **Feature-gated vintages/profiles** (additive):
  - `cgmes3` *(default)* — CGMES 3.0 profile set
  - `cgmes2` — CGMES 2.4.15
  - `cim17` — full canonical schema (superset, heavyweight, for tooling authors)
  - `zip`, `serde`, `rdf` — optional integrations
- Generated code is **macro-free and generic-light** (plain structs, plain impls) — the
  cheapest thing rustc compiles.
- Dev-loop builds use `--no-default-features` + one profile.

Mandatory dependencies stay minimal: `quick-xml`, `memchr`-class utilities, a UUID type.
Everything else is optional.

## 6. Quality Bar ("best-in-class" made concrete)

| Dimension | Commitment |
|---|---|
| Correctness | Round-trip property tests (read→write→read, semantic equality) on the public **CGMES conformity test models**: MicroGrid / MiniGrid / SmallGrid / RealGrid / FullGrid (CAS v2.0, CGMES 2.4.15) and the CAS v3.0.3 configurations (CGMES 3.0), plus the QoCDC v3.2.1 model set |
| Interop | Golden-file tests against outputs/inputs of PowSyBl and cimgen-generated models |
| Robustness | `cargo-fuzz` targets on the XML reader; lenient-mode guarantees: never panic on malformed input, always return diagnostics |
| Performance | Criterion benchmarks in CI on real-scale models; documented throughput numbers in the README |
| API stability | `cargo-semver-checks` in CI; pre-1.0 minor-bump discipline; generated-code diffs reviewed like source |
| Docs | Standard-derived rustdoc on every generated item; a `docs/` book: quickstart, CGMES primer, dataset cookbook, migration notes per schema vintage |
| MSRV | Stated, tested in CI, bumped only on minor releases |
| Supply chain | `cargo-deny` (licenses/advisories); reproducible codegen; provenance note linking every generated file to its RDFS input + version |

### Spec & test corpus (`specs/`, gitignored)

All standards inputs live in `specs/`, fetched — never committed — by
`scripts/fetch-specs.sh` (idempotent, resumable, pinned refs, SHA-256 `MANIFEST.txt`):

| Path | Content | License |
|---|---|---|
| `application-profiles-library/` | ENTSO-E RDFS + SHACL + PROF for CGMES 3.0 (incl. Header AP, beta 501-Ed2 RDFS), CGMES 2.4 archive, NC profiles — pinned tag | Apache-2.0 |
| `cgmes-2.4.15/` | CGMES 2.4.15 RDFS (2016) + 2020 component refresh | Apache-2.0 (UCAIug/ENTSO-E) |
| `test-models/cas-2.0/` | MicroGrid, MiniGrid, SmallGrid, RealGrid, FullGrid conformity test configurations | CC BY-SA 4.0 |
| `test-models/cas-3.0.3/` | CGMES 3.0 conformity assessment test configurations | CC BY-SA 4.0 |
| `test-models/qocdc-3.2.1/` | ENTSO-E QoCDC quality-gate test models (large, realistic) | CC BY-SA 4.0 |
| `docs/` | Public ENTSO-E PDFs: CGMES 2.4.15 & 2.5/600 technical specs, RDF syntax user guide, profiles read-me, implementation Q&A | public ENTSO-E docs |

Licensing consequence: CC BY-SA 4.0 test models are a *local/CI test corpus only* and
are never redistributed inside the published crate; Apache-2.0 RDFS inputs feed codegen,
and the resulting Rust source (committed, published) carries the UCAIug attribution
headers. IEC standard *documents* are paywalled and deliberately absent — nothing in the
build or test pipeline may depend on them.

## 7. Ergonomics Preview

```rust
use cim::cgmes3::{Dataset, ReadOptions, eq::ACLineSegment};

// Assemble a multi-profile model (EQ + SSH + TP + SV + boundary)
let ds = Dataset::read_files(
    ["model_EQ.xml", "model_SSH.xml", "model_TP.xml", "model_SV.xml", "boundary_EQBD.xml"],
    ReadOptions::lenient(),
)?;

for line in ds.iter::<ACLineSegment>() {
    let bv = line.base_voltage(&ds)?;            // typed association traversal
    println!("{}: {} kV, r={:?}", line.name().unwrap_or("?"), bv.nominal_voltage(), line.r);
}

let report = ds.validate();                       // structured diagnostics
ds.write_zip("out.zip", Default::default())?;     // deterministic, header-complete
```

## 8. Roadmap

1. **M0 — Foundation** ✅ codegen pipeline (RDFS → IR → Rust), CGMES 3.0 schema tables,
   streaming reader, mRID index, dataset merge. *Exit met: the published conformity
   corpus parses and navigates.*
2. **M1 — Round-trip** ✅ writer, headers, boundary handling, all eleven profiles,
   difference models, round-trip suite over the conformity models. *Exit met: 31 model
   sets, 250,698 objects, no value lost.*
3. **M2 — Validation** ✅ structural checks with stable rule codes, profile coverage
   reporting, zip archives, `codegen --check` in CI. *Exit met: the complete merged
   conformity models validate with zero findings.*
4. **M3 — Hardening & 0.x release** — `cargo-fuzz` targets on the reader, criterion
   benchmarks in CI, `cargo-semver-checks`, a documentation book, crates.io release,
   and a feedback loop with the LF Energy / SOGNO community (possible cimgen upstreaming).
5. **M4 — CGMES 2.4.15** — the second vintage, exercising the "new vintage is a
   regeneration" claim against a schema that is genuinely different (CIM16 namespace,
   TPBD profile). The design cost of this was paid up front; the work is codegen input
   selection plus a feature gate.
6. **Post-1.0 candidates** — CIM18 on publication, IEC 61970-501 Ed.2 RDFS input,
   ENTSO-E NC/RCP extension profiles, CIM JSON-LD syntax, IEC 62325 market profiles, a
   native SHACL subset, Python and WASM bindings (separate crates), topology-processing
   helpers.

### What is verified today

| | |
|---|---|
| Conformity corpus read | 31 model sets, 361 files, 250,698 objects, **0 diagnostics** |
| Round-trip | MicroGrid, MiniGrid, SmallGrid, FullGrid, merged variants — no value lost |
| Difference models | published EQ diff applied: 346 retractions, 612 assertions, 0 errors |
| Validation | complete merged models: **0 findings** |
| Throughput | 248 MiB/s read, 304 MiB/s write (RealGrid, 112 MiB) |
| Test suite | 57 tests; corpus-backed tests skip cleanly on a fresh clone |

## 9. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Compile time / binary size of generated code | *Resolved by design*: sparse storage means one view type per class instead of a wide struct; the crate compiles in seconds. Feature-gated vintages remain available if a second vintage changes this. |
| Real-world files violate 552 (namespaces, UUIDs, dangling refs) | Lenient mode is a first-class design input and the default; non-conforming identifiers are preserved verbatim, malformed values are reported without losing the object, and every deviation becomes a structured diagnostic. Covered by tests that need no corpus. |
| Schema evolution (CIM18) | Everything model-specific is regenerated; the hand-written core is written against `schema::Schema` and names no class. `codegen --check` in CI makes drift a build failure. |
| Single-crate constraint vs. codegen tooling | Generator lives as unpublished `xtask` in the repo — published surface stays one crate |
| Prior-art overlap (cimoxide) | Different positioning: single ergonomic crate, streaming perf, dataset semantics; monitor and cooperate where sensible |
| IP concerns | Only Apache-2.0 UCAIug/ENTSO-E machine-readable artifacts are consumed/redistributed; IEC PDF content is never copied |

## 10. Sources

- [IEC 61970-301:2020 (IEC webstore)](https://webstore.iec.ch/en/publication/62698) · [DIN EN IEC 61970-301:2025 = 61970-301:2020+AMD1:2022](https://webstore.ansi.org/standards/din/dineniec619703012025)
- [CIM18 / Ed.7 development status — WG13 release notes](https://utf13-reports.ucaiug.io/18v03-18v04/CIM18v04_ReleaseNotes.pdf)
- [IEC TS 61970-600-1:2021 (CGMES 3.0)](https://webstore.iec.ch/en/publication/63866) · [ENTSO-E CGMES library](https://www.entsoe.eu/data/cim/cim-for-grid-models-exchange/)
- [ENTSO-E application-profiles-library (RDFS + SHACL, CGMES + NC)](https://github.com/entsoe/application-profiles-library) · [ENTSO-E RDF syntax user guide](https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/RDF-SyntaxUserGuide_v1-0.pdf)
- [PowSyBl CIM-CGMES importer/exporter docs](https://powsybl.readthedocs.io/projects/powsybl-core/en/stable/grid_exchange_formats/cgmes/)
- [sogno-platform/cimgen (Apache-2.0; C++, Go, Java, Python backends)](https://github.com/sogno-platform/cimgen) · [pycgmes (cimgen-generated Python)](https://pypi.org/project/pycgmes/) · [cimoxide (early Rust CIM tooling)](https://github.com/m-mirz/cimoxide)
- [ENTSO-E CIM conformity & interoperability (test configurations, CC BY-SA 4.0)](https://www.entsoe.eu/data/cim/cim-conformity-and-interoperability/)
- [CIM Modeling Guide — UCAIug CIM licensing (Apache 2.0)](https://cim-mg.ucaiug.io/latest/section1-introduction/) · [UCAIug CIM JSON-LD syntax work](https://github.com/cimug-org/CIM_JSON-LD_Syntax)
- [oxrdfxml (maintained RDF/XML parser; rio_xml deprecated)](https://crates.io/crates/oxrdfxml)
