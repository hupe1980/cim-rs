+++
title = "Performance"
description = "Measured throughput of cim-rs on the ENTSO-E RealGrid conformity model — 112 MiB, 188,547 objects — and the design decisions the numbers come from."
weight = 90
+++

Measured on the ENTSO-E `RealGrid` conformity model: **112 MiB, 188,547 objects, 1.1 million
values**. Release build, Apple M-series. Documents are read from memory, so the numbers
measure parsing rather than the disk.

| Operation | Time | Rate |
|---|---|---|
| Read | **0.33 s** | 343 MiB/s · 576k objects/s |
| Write, as the file set it came from | **0.34 s** | 335 MiB/s |
| Write as RDF (N-Triples, 1.3M triples) | **0.55 s** | |
| Validate | **0.04 s** | 4.5M objects/s |
| Diff against another model state | **0.07 s** | 2.6M objects/s |
| Build the inverse index | **0.03 s** | |

```bash
cargo bench -p cim-rs
```

The benchmark also measures a synthetic model, so it runs without the standards corpus.

> Absolute figures move by 20% or more with what else the machine is doing. Treat the
> **ratios** between rows as the stable part.

## Where the speed comes from

**Streaming, never buffering.** The reader is a purpose-built pull parser over the small,
regular subset of RDF/XML that IEC 61970-552 defines. A general RDF toolchain would be both
slower and — as [Standard RDF](@/docs/rdf.md) explains — wrong for this input anyway.

**Element names resolved through a cache.** Instance documents repeat a handful of element
names many thousands of times, so the cache is keyed on the raw QName bytes and removes
nearly all lookup cost. Namespaces, attributes and identifiers are borrowed rather than
copied on the hot path.

**Sparse storage.** Objects carry four values on average against a mean of 21 possible
fields, so memory is proportional to real content rather than to the widest class. See
[Concepts](@/docs/concepts.md).

**Per-class buckets with a recorded slot.** `view::<ACLineSegment>()` walks a handful of
buckets rather than scanning the model, and moving an object between classes is O(1) rather
than a bucket scan — which matters because loading a Steady State Hypothesis file before its
Equipment file promotes every object it carries.

**No allocation on the diagnostic path.** Findings hold an `Mrid` and the schema's own
`&'static str` attribute name rather than rendered text. Removing one `String` per value
checked is what roughly halved validation.

## Assembling in parallel

Reading is the expensive half and a model set is a handful of independent files, so each can
be parsed on its own thread and the parts combined:

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let mut model = Dataset::new(SCHEMA);
    for part in ["EQ.xml", "SSH.xml", "TP.xml", "SV.xml"] {
        model.merge(Dataset::load(SCHEMA, [part])?)?;
    }
    println!("{} objects", model.len());
    Ok(())
}
```

Merging by mRID does not care which dataset an object arrived in. That the result is the
*same model* is a test, not an assertion: a merged assembly and a sequential one produce the
same `Dataset::content_id`.

## Compile time

Around 1,100 generated types per vintage is a real compile-time hazard, mitigated three
ways: generated code is **macro-free and generic-light** (plain structs, plain impls — the
cheapest thing rustc compiles); vintages are **feature-gated**, so a program targeting CGMES
3.0 never compiles the 2.4.15 tables; and generated sources are **committed rather than
built by `build.rs`**, so downstream builds are fast and reproducible and docs.rs works.

## A measurement that did not survive

Swapping SipHash for a fast non-cryptographic hasher on the mRID index is the obvious next
optimization. It was measured, could not be shown to earn its place, and identifiers come
from files this crate is designed to read *without trusting* — so it was dropped rather than
kept on the strength of how it looked.
