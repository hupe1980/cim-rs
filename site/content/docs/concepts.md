+++
title = "Concepts"
description = "What a CGMES profile set is, why a model is many files, how mRIDs identify objects, and why cim-rs stores objects sparsely behind generated typed views."
weight = 20
+++

If you already know CGMES, skip to
[the design section](#how-cim-rs-represents-it). If you do not, this page is the
background the rest of the documentation assumes.

## The standards, briefly

The **Common Information Model** (CIM) is an IEC family of standards describing power
systems as classes, attributes and associations — substations, lines, transformers,
measurements. Three parts matter here:

| Standard | What it gives you |
|---|---|
| IEC 61970-301 | The base semantic model: what an `ACLineSegment` *is* |
| IEC 61970-501 | How that model is published as machine-readable **RDFS** |
| IEC 61970-552 | **CIM/XML**, the file format models are exchanged in |

**CGMES** (IEC 61970-600) is ENTSO-E's *profile set* on top of CIM: a selection of classes
and attributes, split into the pieces a grid exchange actually needs. CGMES 3.0 is the
current interop target; CGMES 2.4.15 is what most of Europe has been exchanging since 2021.

`cim-rs` reads the RDFS vocabularies ENTSO-E publishes and generates the whole typed model
from them, so adopting a new vintage is a regeneration rather than a rewrite.

## A model is a set of files

This is the single most important thing to know. A CGMES model is split across **profiles**,
each shipped as its own instance file:

| Profile | Carries |
|---|---|
| **EQ** Equipment | What exists and how it is connected — ratings, impedances, containment |
| **SSH** Steady State Hypothesis | The operating point — dispatch, switch positions, tap positions |
| **TP** Topology | Which terminals are electrically joined into nodes |
| **SV** State Variables | Power-flow results — voltages, flows |
| **EQBD** Equipment Boundary | The shared border between modelling authorities |
| **DL** / **GL** | Diagram layout / geographical location |
| **DY** Dynamics | Machine and control models |
| **OP** / **SC** | Operational limits / short-circuit data |

The same object appears in several of them. A `SynchronousMachine` gets its ratings from
EQ and its dispatch from SSH — **under one identifier**. Neither file is the whole object.

That is why the unit of work in `cim-rs` is a
[`Dataset`](https://docs.rs/cim-rs/latest/cim_rs/dataset/struct.Dataset.html), not a
document: loading several files merges each object's attributes rather than producing
duplicates, and load order does not matter.

> **The one exception to order-independence** is a genuine contradiction — two files giving
> one single-valued attribute two different values. Resolving that means discarding one of
> them, so it is recorded rather than swallowed and reported as `CIM0018`.

## Identity: the mRID

Every CIM object carries a **master resource identifier**, and IEC 61970-552 requires it to
be a UUID. In a file it appears as `rdf:ID="_<uuid>"` when the file *introduces* the object,
and `rdf:about="#_<uuid>"` when the file only adds to one defined elsewhere.

Published files do not always comply, and the deviations matter:

* **UUIDs without hyphens.** The CGMES 2.4.15 boundary sets write
  `rdf:ID="_1fa19c281c8f4e1eaad9e1cab70f923e"`. That *is* the UUID
  `1fa19c28-1c8f-4e1e-aad9-e1cab70f923e`, so it has a `urn:uuid:` form and belongs in an
  RDF graph as an IRI; only the spelling deviates.
* **Absolute IRIs.** A reference crossing documents is written
  `rdf:resource="http://host/EQ.xml#_<uuid>"`. The fragment is the object.

Reading either as opaque text is the obvious move and is wrong twice over: the object loses
its name in RDF, and it splits in two the moment another file spells the same UUID the
conforming way. `Mrid` compares by the sixteen bytes and remembers the spelling, so identity
holds and re-export writes back what the document had.

## `rdf:ID` versus `rdf:about` is not a per-file choice

A Topology file writes `TopologicalNode` with `rdf:ID` — Topology introduces it — and
`Terminal` with `rdf:about`, because Equipment did, **in the same document**. The RDFS
records which is which by marking the second kind `cims:stereotype Description`.

Deriving it from the profile keyword instead silently rewrote 49,255 identifiers in one
published State Variables file. `cim-rs` reads the stereotype.

## How `cim-rs` represents it

### Sparse storage, not a struct per class

A `Terminal` has around thirty possible attributes; a Steady State Hypothesis file sets
exactly one. Across the conformity corpus, 250,698 objects carry 1.1 million values — four
each on average, against a mean of 21 flattened fields per concrete class. A generated
struct per class would spend most of its memory on `None`, and could not naturally hold the
union of several profiles' attributes.

So an `Object` is a vector of `(attribute, value)` slots sorted by attribute id. Lookup is a
partition point; memory is proportional to real content.

### Typed views over that storage

Ergonomics come from generated **zero-cost views** instead:

```rust
use cim_rs::prelude::*;
use cim_rs::cgmes3::views::ACLineSegment;

fn impedances(grid: &Dataset) {
    for line in grid.view::<ACLineSegment>() {
        let r: Option<f64> = line.r();          // typed from the profile
        let name: Option<&str> = line.name();   // inherited from IdentifiedObject
        println!("{name:?}: r={r:?}");
    }
}
```

`ACLineSegment<'a>` is a wrapper around `&'a Object`. Accessors borrow for the *view's*
lifetime rather than the call's, so resolving an association through the dataset allocates
nothing.

### Associations are identifiers, not pointers

IEC 61970-501 serializes exactly one side of each association; the other is derived by
inversion. Following one forwards is a lookup:

```rust
use cim_rs::prelude::*;
use cim_rs::cgmes3::views::Terminal;

fn owners(grid: &Dataset) {
    for terminal in grid.view::<Terminal>() {
        // Resolved through the dataset, allocating nothing.
        if let Some(equipment) = terminal.conducting_equipment_in(grid) {
            println!("{:?}", equipment.name());
        }
    }
}
```

Following one backwards is a scan unless you index it, so build the index once:

```rust
use cim_rs::prelude::*;
use cim_rs::cgmes3::{attributes, views::Substation};

fn voltage_levels_of_each_substation(grid: &Dataset) {
    let inverse = cim_rs::InverseIndex::build(grid);
    for substation in grid.view::<Substation>() {
        let levels = inverse.referrers(attributes::voltage_level::Substation, substation.mrid());
        println!("{:?}: {} voltage level(s)", substation.name(), levels.len());
    }
}
```

### Compounds are values, not references

A few CIM types have no identity of their own — a `StreetAddress` inside
`Location.mainAddress`. IEC 61970-552 writes them *inline*, as
`rdf:parseType="Resource"`, and they nest. They are stored as values, not objects:

```rust
use cim_rs::prelude::*;
use cim_rs::cgmes3::views::Location;

fn towns(grid: &Dataset) {
    for location in grid.view::<Location>() {
        if let Some(town) = location.main_address().and_then(|a| a.town_detail()) {
            println!("{:?} in {:?}", location.name(), town.name());
        }
    }
}
```

A parser that reads the nested elements as text does not fail — it fabricates a value.

### Named constants spelled as the standard spells them

`UnitMultiplier.M` (mega) and `UnitMultiplier.m` (milli) differ only by case, so
transforming names to `SCREAMING_SNAKE_CASE` would collide them. Generated constants mirror
the CIM identifier exactly, which is also how a reader of IEC 61970-301 finds them.

## Provenance: which file a value came from

`IdentifiedObject.mRID` is declared in **ten of eleven** CGMES 3.0 profiles, and `name` in
eight. Deciding what a profile file should contain from declarations alone therefore repeats
almost every object in almost every file — measured on the largest published model, that
inflated 112 MiB to 399 MiB on export.

So each stored value records **which profiles' files it came from**, and the export rule is
that provenance *intersected* with the profiles that declare the attribute. Each object
additionally records which file contributed it, because in a merged common grid model two
authorities contribute the same profile and each authority's equipment has to go back in its
own file.

That is what makes [`save_as_loaded`](@/docs/reading-and-writing.md#writing-a-model-back)
reproduce the file set a model was read from.
