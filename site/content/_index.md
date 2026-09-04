+++
title = "cim-rs — IEC CIM / CGMES grid models in Rust"
description = "Read, navigate, validate and write IEC CIM / CGMES power-system models in Rust. CGMES 3.0 and 2.4.15, streaming CIM/XML, profile-aware validation, and RDF export with the datatypes CIM/XML omits."
template = "index.html"
+++

<section class="band">
<div class="wrap">

```rust
use cim_rs::prelude::*;
use cim_rs::cgmes3::{SCHEMA, views::ACLineSegment};

fn main() -> cim_rs::Result<()> {
    // A CGMES model is a *set* of profile files describing the same objects.
    let grid = Dataset::load_dir(SCHEMA, "MicroGrid-BE")?;

    for line in grid.view::<ACLineSegment>() {
        let kv = line.base_voltage_in(&grid).and_then(|bv| bv.nominal_voltage());
        println!("{:?}  {kv:?} kV  r={:?} x={:?}", line.name(), line.r(), line.x());
    }

    // Structural checks, against the profile rather than against a guess.
    for finding in grid.validate().iter() {
        println!("{finding}");
    }
    Ok(())
}
```

</div>
</section>

<section class="band soft">
<div class="wrap">

## Why another CIM library

The IEC Common Information Model is how European transmission system operators exchange
grid models. The mature implementations are Java ([PowSyBl][powsybl]), C++ (libcimpp) and
Python ([pycgmes][pycgmes], CIMpy). Rust had only an early multi-crate experiment
([cimoxide][cimoxide]) — and Rust is exactly where the gap
hurts: high-throughput model servers, edge grid controllers, WebAssembly browser tooling
and safety-critical pipelines all want a fast, memory-safe, dependency-light CIM core.

`cim-rs` is that core. One crate, one mandatory dependency, and the whole typed model
generated from the vocabularies ENTSO-E publishes.

[powsybl]: https://powsybl.readthedocs.io/projects/powsybl-core/en/stable/grid_exchange_formats/cgmes/
[pycgmes]: https://github.com/alliander-opensource/pycgmes
[cimoxide]: https://github.com/m-mirz/cimoxide

</div>
</section>

<section class="band">
<div class="wrap">

## What makes it correct

<p class="sub">CGMES has a handful of properties that a plausible-looking implementation
gets wrong. Each of these is enforced by a test against the published conformity models.</p>

<div class="grid">
<div class="card">
<h3>A model is a set of files</h3>
<p>The same <code>SynchronousMachine</code> gets its ratings from Equipment and its dispatch
from Steady State Hypothesis, under one identifier. Objects merge by mRID, so load order
does not matter and nothing is duplicated.</p>
</div>
<div class="card">
<h3>An identifier is a UUID</h3>
<p>Not the way somebody spelled it. Published files write UUIDs without hyphens, and as
absolute IRIs. Read as opaque text, those objects lose their name in RDF and split in two.</p>
</div>
<div class="card">
<h3>Provenance decides the export</h3>
<p><code>IdentifiedObject.mRID</code> is declared in ten of eleven profiles. Writing from
declarations alone inflated a 112 MiB model to 399 MiB. Each value records the file it
came from.</p>
</div>
<div class="card">
<h3>The element class is derived</h3>
<p>An SSH file writes <code>&lt;cim:Equipment&gt;</code> for an <code>ACLineSegment</code>,
because SSH declares <code>Equipment</code> and not <code>ACLineSegment</code> — and never
a class whose mandatory attributes the data does not supply.</p>
</div>
<div class="card">
<h3>rdf:ID is per class, per profile</h3>
<p>Topology introduces <code>TopologicalNode</code> and only refers to <code>Terminal</code>,
in one document. The RDFS says which is which; guessing from the profile keyword rewrites
tens of thousands of identifiers.</p>
</div>
<div class="card">
<h3>A number's spelling is information</h3>
<p>Published models write <code>2.62637E-05</code> and <code>250.000000</code>. Each value
keeps the form it arrived in, while equality compares numbers — so a re-export is the file
that was read, and a difference never reports a reformatting as a change.</p>
</div>
<div class="card">
<h3>Output has to satisfy someone else</h3>
<p>A tolerant reader accepting its own output proves nothing. Every document is checked
against the XML and XML Namespaces recommendations, and every RDF export against the
N-Triples grammar.</p>
</div>
</div>

The full conformance record is [here](@/docs/conformance.md).

</div>
</section>

<section class="band soft">
<div class="wrap">

## Standard RDF, with the datatypes CIM/XML omits

CIM/XML looks like RDF/XML and is not: it predates the W3C recommendation, and it carries
**no datatype information at all**. ENTSO-E's SHACL shapes constrain 3,137 properties by
`sh:datatype`; a graph loaded straight from CIM/XML fails all of them, because every
literal in it is a string. ENTSO-E's own interoperability reporting names the missing
piece — *"there are no open libraries to natively enhance the data based on the profile
definitions."*

```turtle
<urn:uuid:17086487-56ba-4979-b8de-064025a6b4da>
    a cim:ACLineSegment ;
    cim:ACLineSegment.r "2.2"^^xsd:float ;
    cim:ConductingEquipment.BaseVoltage <urn:uuid:a7f1d8de-d658-428a-821b-3a5ae5965fd1> ;
    cim:Equipment.aggregate "false"^^xsd:boolean ;
    cim:IdentifiedObject.name "BE-Line_1" ;
    eu:IdentifiedObject.shortName "BE-L_1" .
```

Every profile that carries data, in every published CGMES 3.0 conformity model, passes
ENTSO-E's own shapes under `pyshacl`, in CI — see [Standard RDF](@/docs/rdf.md).

</div>
</section>

<section class="band">
<div class="wrap">

## Install

```bash
cargo add cim-rs                  # the library
cargo install cim-rs --features cli   # the `cim` command line
```

```bash
cim info     MicroGrid-BE/                  # what a model set contains
cim validate MicroGrid-BE/ --rule CIM0007   # exits 1 on any error
cim rdf      MicroGrid-BE/ --out graphs/    # one typed RDF graph per profile with data
cim diff     before/ after/ > change.xml    # the change set between two states
cim apply    before/ --change change.xml --out after/   # and applying one
```

<div class="cta-links">

[Read the guide](@/docs/getting-started.md)
[API reference](https://docs.rs/cim-rs)

</div>

</div>
</section>
