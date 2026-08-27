+++
title = "Conformance"
description = "Which IEC standards cim-rs implements, which CGMES profiles it supports, and exactly what is verified against the published ENTSO-E conformity models on every build."
weight = 80
+++

## Standards

| Standard | Role |
|---|---|
| IEC 61970-301:2020 + AMD1:2022 | CIM base semantics (CIM17) |
| IEC 61970-501:2006 | RDFS profile representation — the code-generation input |
| IEC 61970-552:2016 | CIM/XML: identity, headers, difference models |
| IEC TS 61970-600-1/-2:2021 | CGMES 3.0 profile set |
| CGMES 2.4.15 | The profile set in production use across Europe since 2021 |

Profiles supported: **EQ, OP, SC, EQBD, SSH, TP, SV, DL, GL, DY** and the header vocabulary
in both vintages, plus **TPBD** in CGMES 2.4.15.

| Vintage | Feature | Namespace | Classes | Attributes | Enums | Profiles |
|---|---|---|---|---|---|---|
| CGMES 3.0 | `cgmes3` *(default)* | `CIM100` + `eu` | 443 | 3,624 | 51 | 11 |
| CGMES 2.4.15 | `cgmes2` | `cim16` + `entsoe` | 401 | 3,539 | 48 | 12 |

## What is verified

Against the published ENTSO-E conformity assessment corpora, on every build with the corpus
present:

| | |
|---|---|
| **Read** | 31 CGMES 3.0 model sets, 361 files, 250,698 objects — **0 diagnostics** |
| **Read** | 52 CGMES 2.4.15 archives, 273,144 objects — 0 errors, 9 recovered namespace mistakes |
| **Re-export** | All 361 files, class for class and `rdf:ID`/`rdf:about` for `rdf:ID`/`rdf:about` |
| **Well-formedness** | Every written document re-parsed with namespace resolution, duplicate-attribute detection and XML 1.0's `Char` production |
| **RDF export** | 1.67M triples over the corpus, whole-model and per profile, against the N-Triples grammar |
| **SHACL** | Every profile *that carries data*, in every published model set, against ENTSO-E's own shapes (`pyshacl`, in CI). A profile a model says nothing about gets no graph and is reported as `no data`; a run in which every profile was skipped fails |
| **Round-trip** | Every conformity model, by profile and as its original file set — no value lost |
| **Difference models** | Read, applied and written back on both vintages, class replacement included |
| **Computed differences** | A change set derived from two model states, applied, and checked to reproduce the target |
| **Validation** | The whole corpus, with the findings pinned so a new one fails the build |
| **Merge conflicts** | Pinned by location — every model set agrees with itself except the two 24-hour time-series directories |
| **Robustness** | Every truncation, 1,631 single-byte corruptions and every region deletion: no panic |
| **Malformed input** | Markup nested in a value stays out of it; objects with no identifier stay distinct; absolute-IRI references resolve; a broken document's report is bounded and says where it stopped |
| **Assembly** | A model merged from datasets built separately has the same content identifier as the same files loaded in sequence |
| **Syntax limits** | A character XML cannot represent is stripped and reported where it enters (`CIM0019`); an identifier that is an absolute IRI is written as `rdf:about` rather than as an `rdf:ID` that is not an `NCName`, and one that is neither is reported (`CIM0020`) |
| **Profile bound** | Every shipped vintage fits `ProfileMask`, with its profiles' bits pinned distinct |
| **Command line & examples** | Run against published models in CI; what they write is checked by `xmllint` |
| **Feature matrix** | Every supported feature combination builds, lints **and compiles its doctests** — the last is what the matrix used to skip, and the README and guide were the part that broke |
| **Documentation links** | Every `#`-anchor in the README and the design document resolves to a heading actually in it; `zola build` fails on a broken link between site pages |
| **Cross-validation** | PowSyBl (Java) builds the identical network from a re-export as from the published files — every equipment count, both bus views, and a SHA-256 over every identifier — on four assembled CGMES 3.0 configurations and the CGMES 2.4.15 MicroGrid archive; rdflib (Python) parses the RDF export as a graph |
| **Identifier text** | Every re-exported file's `rdf:ID`/`rdf:about` values compared letter for letter with its input — what the class-and-style census cannot see |
| **Streams** | `cim diff` run both redirected and with `--out`, and the two compared byte for byte: where a command's result is a document, the document is the whole of stdout |
| **Portability** | The library built for `wasm32-unknown-unknown` — no filesystem, no C dependency — so "usable from WASM" is a build rather than a claim |

208 tests in total. Corpus-backed tests skip cleanly when the standards artifacts are
absent, so a fresh clone is green.

## The check that is not ours

Everything above is written by this repository. The census compares our output against the
input *we* parsed; the well-formedness and N-Triples checkers judge it against grammars *we*
implemented; even the SHACL run, which uses a real engine, is driven by shapes we selected
over a graph we produced.

All of it can be true while `cim-rs` misunderstands CGMES **consistently**. A writer and a
reader that share a misconception agree with each other perfectly, and a round trip is
exactly the test that cannot see one.

`cargo xtask crossvalidate` runs implementations that do not share our assumptions, in
pinned containers:

- **PowSyBl** (Java, LF Energy) imports both the published files and a `cim-rs` re-export of
  them, and the two networks must be identical — the count of every identifiable type, both
  bus views, and a SHA-256 digest over every identifier. PowSyBl reads CGMES through an
  RDF4J triple store and SPARQL; `cim-rs` uses a purpose-built streaming parser. Two
  implementations sharing a misconception is what a self-check cannot rule out; two that do
  not share an *approach* is as independent as this gets.
- **rdflib** (Python) parses the RDF export into a graph. The datatype histogram is the
  point: that literals carry the XSD type the profile assigns is the claim
  [the RDF page](@/docs/rdf.md) makes, and nothing else here would notice it silently
  reverting to plain strings.

It demonstrably fails when it should. Removing one `cim:Breaker` from a published equipment
file changes the digest completely, the switch count from 28 to 27, and the bus count from
14 to **15** — the switch had been holding two buses together. That a count moved *upward*
is why the digest is there.

### What it found

CGMES 2.4.15 disagreed the first time it was run. The boundary set writes UUIDs in *mixed*
case — `_24C12434-E42B-497f-928F-119C6AE92079` — and `Mrid` remembered whether an identifier
had been hyphenated but not how it was cased, so all 26 identifiers in that file were
rewritten on export. Nothing here could see it: the census compares classes and identity
styles, and the round-trip compares the model, where identity is the sixteen bytes. PowSyBl
saw it at once, because an IIDM identifier is the mRID string. `Mrid` now carries case as a
bitmask over the 32 digit positions — at no extra size — and the census compares identifiers
letter for letter.

It also produced two false positives, both worth knowing about because they are the same
mistake. PowSyBl names a tie line by joining its two halves' mRIDs in *encounter* order, so
this crate's deterministic mRID ordering flips it while the network stays identical; the
harness compares that pair unordered. And an assertion that no class appears both named and
blank fires on the published 2.4.15 MicroGrid, where 43 `CurrentLimit`s are identified by
`_88240efd-b544-4131-bc99-e6b77d4bac881` — a UUID with a digit appended, which is not a UUID,
has no `urn:uuid:` form, and is correctly left blank rather than given an invented name. Both
were withdrawn. A cross-validation's first job is to be believable.

## How the layers differ

Each of these catches a class of defect the others structurally cannot, which is why there
are several:

* **Round-trip on the model** proves no *value* was lost. It cannot see the document.
* **A per-file element census** — how many objects of each class, identified which way —
  proves the *document* was reproduced. It cannot see whether the document is valid.
* **A conforming XML parser** proves the output is a document at all. This is what found
  that every file the writer emitted carried `xmlns:md` twice.
* **A real SHACL engine** proves the RDF export is something the wider ecosystem accepts.
* **Deterministic mutation** — truncation at every byte, single-byte corruption, region
  deletion — proves the never-panic guarantee on stable, alongside `cargo-fuzz` targets for
  longer runs on nightly.

## Where cim-rs deviates deliberately

The published vocabularies deviate from the standards they implement — ENTSO-E says so in
the RDF-Syntax User Guide: *"current implementations deviate from these standards due to
various reasons."* Three of those deviations are load-bearing and are handled explicitly:

1. **The `Description` stereotype is applied unevenly.** Steady State Hypothesis marks all
   45 classes it touches except `Equipment`, which it declares concrete as the generic
   carrier for `inService`. Taken literally that would make SSH the file that introduces
   every piece of equipment. The signal is in the hierarchy: a profile that marks `Switch`
   and `ACLineSegment` as descriptions is annotating equipment, not introducing it.
2. **A class instantiated where its own profile calls it abstract** is still treated as
   defined there.
3. **Published files set attributes their own header does not declare.** A CGMES 2.4.15
   boundary file writes `Terminal.ConnectivityNode`, which only Equipment declares. The
   value goes back where it came from — the data is evidence, the vocabulary is a claim
   about it — and the discrepancy is reported as `CIM0010` rather than silently re-filed.

## Not in scope

* Power flow, state estimation, topology processing — this is the data layer they need.
* A general RDF store, SPARQL engine or OWL reasoner.
* **Running SHACL.** Emitting data a SHACL engine can consume is a goal and is met; running
  the engine would be a second project.
* Reading RDF back. CIM/XML is the exchange format.
* IEC 62325 market profiles and 61968 message envelopes — the generator treats a profile as
  data, so these can be added without architectural change.

## Licensing

The crate is **MIT OR Apache-2.0**.

The RDFS vocabularies the generated code derives from are published by ENTSO-E and the UCA
International Users Group under the **Apache License 2.0**; generated files carry that
attribution. The IEC standard *documents* are copyrighted by the IEC, are not redistributed,
and nothing in the build or test pipeline depends on them.

The ENTSO-E conformity test models are licensed **CC BY-SA 4.0** and owned by ENTSO-E. They
are a local and CI test corpus only, never redistributed with the crate.
