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
| **SHACL** | Every CGMES 3.0 profile *that carries data*, in every published model set, against ENTSO-E's own shapes (`pyshacl`, in CI). A profile a model says nothing about gets no graph and is reported as `no data`; a run in which every profile was skipped fails. CGMES 2.4.15 runs too and validates nothing — see below |
| **Round-trip** | Every conformity model, by profile and as its original file set — no value lost |
| **Difference models** | Read, applied and written back on both vintages, class replacement included |
| **Computed differences** | A change set derived from two model states, applied, and checked to reproduce the target |
| **Validation** | The whole corpus, with the findings pinned so a new one fails the build |
| **Merge conflicts** | Pinned by location — every model set agrees with itself except the two 24-hour time-series directories |
| **Quality-check corpus** | ENTSO-E's 100 QoCDC model sets — real TSO exports, conformant and deliberately not — all read, the vintage detected for each, and the three files that are broken on purpose named rather than fatal |
| **Robustness** | Every truncation, 1,631 single-byte corruptions and every region deletion: no panic |
| **Malformed input** | Markup nested in a value stays out of it; objects with no identifier stay distinct; absolute-IRI references resolve; a broken document's report is bounded and says where it stopped |
| **Assembly** | A model merged from datasets built separately has the same content identifier as the same files loaded in sequence |
| **Syntax limits** | A character XML cannot represent is stripped and reported where it enters (`CIM0019`); an identifier that is an absolute IRI is written as `rdf:about` rather than as an `rdf:ID` that is not an `NCName`, and one that is neither is reported (`CIM0020`) |
| **Profile bound** | Every shipped vintage fits `ProfileMask`, with its profiles' bits pinned distinct |
| **Command line & examples** | Run against published models in CI; what they write is checked by `xmllint` |
| **Feature matrix** | Every supported feature combination builds, lints **and compiles its doctests** — `clippy --all-targets` skips those, and an example must name a vintage |
| **Documentation links** | Every `#`-anchor in the README and the design document resolves to a heading actually in it; `zola build` fails on a broken link between site pages |
| **Cross-validation** | PowSyBl (Java) builds the identical network from a re-export as from the published files — every equipment count, both bus views, and a SHA-256 over every identifier — on four assembled CGMES 3.0 configurations and the CGMES 2.4.15 MicroGrid archive; rdflib (Python) parses the RDF export as a graph |
| **Identifier text** | Every re-exported file's `rdf:ID`/`rdf:about` values compared letter for letter with its input — what the class-and-style census cannot see |
| **Value text** | Every value in a re-exported model set compared text for text with its input — 951,340 of them, on both vintages. What the class census, the identifier census and the model round-trip are all structurally blind to: they compare parsed numbers, and `2.62637E-05` and `0.0000262637` are the same `f64` on both sides |
| **Streams** | `cim diff` run both redirected and with `--out`, and the two compared byte for byte: where a command's result is a document, the document is the whole of stdout |
| **Portability** | The library built for `wasm32-unknown-unknown` — no filesystem, no C dependency — so "usable from WASM" is a build rather than a claim |
| **Vintage detection** | Both corpora read with the vintage taken from the documents; a mismatch reported as `CIM0021` at the root element, and neither corpus produces one when correctly paired |

242 tests in total. Corpus-backed tests skip cleanly when the standards artifacts are
absent, so a fresh clone is green.

### CGMES 2.4.15 cannot be SHACL-validated

**Every one of the ten shapes files ENTSO-E publishes for CGMES 2.4.15 is invalid SHACL**:
nine carry property shapes with two `sh:path` values — 46 in `EquipmentProfile.ttl` — and
the tenth has a constraint with two `sh:in`. A conforming engine refuses to load them, so
there is nothing to validate against. The CGMES 3.0 shapes have no such defect.

`cargo xtask shacl --vintage cgmes2` runs in CI anyway and pins the unusable set, so the day
one of those files is fixed the build says so.

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

It demonstrably fails when it should: removing one `cim:Breaker` from a published equipment
file changes the digest, drops the switch count from 28 to 27 and *raises* the bus count from
14 to 15, the switch having held two buses together. A count that moves upward is why the
digest is there and not only the counts.

## How the layers differ

Each of these catches a class of defect the others structurally cannot, which is why there
are several:

* **Round-trip on the model** proves no *value* was lost. It cannot see the document.
* **A per-file element census** — how many objects of each class, identified which way —
  proves the *document* was reproduced. It cannot see the text of an identifier or of a
  value, and it cannot see whether the document is valid.
* **An identifier census and a value census**, comparing both letter for letter against the
  input, close exactly that. Each was added after the layer above it passed while the
  output differed: mixed-case identifiers were being rewritten, and 8,623 numbers in the
  published corpus were being reformatted.
* **A conforming XML parser** proves the output is a document at all — namespaces resolved,
  no duplicate attribute, every character inside XML 1.0's `Char` production.
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
* GUI and diagram rendering — DL and GL data are exposed; drawing is a consumer concern.
* IEC 62325 market profiles and 61968 message envelopes — the generator treats a profile as
  data, so these can be added without architectural change.
* **Bindings for other languages.** The library is `wasm32`-clean with one mandatory
  dependency precisely so a binding *can* be built on it; shipping and maintaining a Python
  wheel or an npm package is a separate product with its own incumbents.

## Licensing

The crate is **MIT OR Apache-2.0**.

The RDFS vocabularies the generated code derives from are published by ENTSO-E and the UCA
International Users Group under the **Apache License 2.0**; generated files carry that
attribution. The IEC standard *documents* are copyrighted by the IEC, are not redistributed,
and nothing in the build or test pipeline depends on them.

The ENTSO-E conformity test models are licensed **CC BY-SA 4.0** and owned by ENTSO-E. They
are a local and CI test corpus only, never redistributed with the crate.

## Sources

- [IEC 61970-301:2020 (IEC webstore)](https://webstore.iec.ch/en/publication/62698) — with AMD1:2022, this is CIM17
- [IEC 61970:2026 SER — the series package, still shipping 61970-501:2006](https://webstore.iec.ch/en/publication/61167)
- [CIM18 / Ed.7 development status — WG13 release notes](https://utf13-reports.ucaiug.io/18v03-18v04/CIM18v04_ReleaseNotes.pdf)
- [IEC TS 61970-600-1:2021 (CGMES 3.0)](https://webstore.iec.ch/en/publication/63866) · [ENTSO-E CGMES library](https://www.entsoe.eu/data/cim/cim-for-grid-models-exchange/)
- [ENTSO-E application-profiles-library (RDFS + SHACL, CGMES + NC)](https://github.com/entsoe/application-profiles-library) · [ENTSO-E RDF-Syntax User Guide v1.1.0](https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/RDF-SyntaxUserGuide_v_1-1-0.pdf) — `cargo xtask fetch-specs` downloads this one; §4 is the source for the `rdf:ID`/`rdf:about` rule (which cites rule MVAL5 of IEC 61970-600-1:2021), for datatypes not being exchanged, and for engineering notation carrying precision
- [CIMTool release notes — 61970-501 Ed.2 draft RDFS, CIM18 domain types](https://cimtool.ucaiug.io/release-notes/)
- [PowSyBl CIM-CGMES importer/exporter docs](https://powsybl.readthedocs.io/projects/powsybl-core/en/stable/grid_exchange_formats/cgmes/)
- [sogno-platform/cimgen (Apache-2.0; C++, Go, Java, Python backends)](https://github.com/sogno-platform/cimgen) · [pycgmes (cimgen-generated Python)](https://github.com/alliander-opensource/pycgmes) · [cimoxide (early Rust CIM tooling)](https://github.com/m-mirz/cimoxide)
- [ENTSO-E CIM conformity & interoperability (test configurations, CC BY-SA 4.0)](https://www.entsoe.eu/data/cim/cim-conformity-and-interoperability/)
- [CIM Modeling Guide — UCAIug CIM licensing (Apache 2.0)](https://cim-mg.ucaiug.io/latest/section1-introduction/) · [UCAIug CIM JSON-LD syntax work](https://github.com/cimug-org/CIM_JSON-LD_Syntax)
- [W3C SHACL](https://www.w3.org/TR/shacl/) · [W3C RDF 1.1 N-Triples](https://www.w3.org/TR/n-triples/) · [W3C RDF 1.1 Turtle](https://www.w3.org/TR/turtle/) · [W3C XML Namespaces 1.0](https://www.w3.org/TR/xml-names/)
- [OpenCGMES (SOPTIM, Apache-2.0)](https://opencgmes.soptim.de/cimxml/overview) — its CIMXML overview independently names normalizing UUIDs "with and without underscore prefixes and dashes" as required behaviour, which is [identity: the mRID](@/docs/concepts.md#identity-the-mrid) · [LF Energy Summit 2025: *Breaking down CGMES barriers*, the talk that catalogues them](https://static.sched.com/hosted_files/lfenergysummiteu2025/5f/LF%20Energy%20Summit%202025%20-%20OpenCGMES.pdf)
- [pySHACL, the engine `cargo xtask shacl` drives](https://github.com/RDFLib/pySHACL) · [ModShape — Python/SHACL validation of CGMES datasets](https://github.com/griddigit-ci/ModShape)
- [oxrdfxml (maintained RDF/XML parser; rio_xml deprecated)](https://crates.io/crates/oxrdfxml)
