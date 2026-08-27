+++
title = "Documentation"
description = "Guide to cim-rs: reading and writing CGMES model sets, validating them against the profile, exporting typed RDF, and computing difference models."
sort_by = "weight"
template = "section.html"
page_template = "page.html"
+++

`cim-rs` is a Rust library and command line for **IEC CIM** grid models, covering the two
profile sets European transmission system operators exchange: **CGMES 3.0**
(IEC TS 61970-600-1/-2:2021) and **CGMES 2.4.15**.

These pages are the guide. The per-item API reference — every generated class, every
attribute, with the documentation the standard itself carries — lives on
[docs.rs](https://docs.rs/cim-rs).

## Where to start

| If you want to | Read |
|---|---|
| Load a model and look at it | [Getting started](@/docs/getting-started.md) |
| Understand what a "profile set" even is | [Concepts](@/docs/concepts.md) |
| Read files, write them back, handle archives | [Reading and writing](@/docs/reading-and-writing.md) |
| Check a model against the profile | [Validation](@/docs/validation.md) |
| Hand a model to a triple store or SHACL engine | [Standard RDF](@/docs/rdf.md) |
| Send only what changed | [Difference models](@/docs/difference-models.md) |
| Do all of that from a shell | [Command line](@/docs/cli.md) |
| Know what is actually verified | [Conformance](@/docs/conformance.md) · [Performance](@/docs/performance.md) |

## At a glance

| | |
|---|---|
| Vintages | CGMES 3.0 (`cgmes3`, default), CGMES 2.4.15 (`cgmes2`) |
| Profiles | EQ, OP, SC, EQBD, SSH, TP, SV, DL, GL, DY, header — plus TPBD in 2.4.15 |
| Formats | CIM/XML (IEC 61970-552) read and write, `dm:DifferenceModel` read/apply/write/**compute**, N-Triples and Turtle out, zip archives |
| Dependencies | one mandatory: `quick-xml`. `zip` is optional |
| MSRV | 1.88 |
| License | MIT OR Apache-2.0 |
