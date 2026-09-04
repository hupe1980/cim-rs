+++
title = "Standard RDF"
description = "Export a CGMES model as N-Triples or Turtle with every literal typed from the profile, so it loads in any RDF toolchain and passes ENTSO-E's SHACL shapes."
weight = 50
+++

## The gap this closes

CGMES is built on W3C standards — RDFS for the schema, SHACL for the constraints, RDF for
the data — and then exchanges the data in a syntax that is **not RDF**.

IEC 61970-552 was published in 2003; RDF/XML 1.0 in 2004. `rdf:parseType="Statements"` is
not RDF syntax at all, and `rdf:ID="_<uuid>"` denotes `urn:uuid:<uuid>` rather than a
fragment of the document. A general RDF parser either refuses a CGMES file or reads the
wrong subjects out of it.

The second gap is larger: **CIM/XML carries no datatype information.** Every value is
element text, so `1` could be an integer, a float or a string — and only the profile knows.
ENTSO-E's CGMES 3.0 SHACL shapes constrain **3,137 properties by `sh:datatype`**, of which
2,871 name `xsd:float`. A graph loaded from CIM/XML fails all of them.

ENTSO-E's own 2024 interoperability reporting names the missing piece exactly: *"There are
no open libraries to natively enhance the data based on the profile definitions."*

`cim-rs` already holds the only thing needed to close that — the profile, parsed.

## Exporting

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;
use cim_rs::rdf::{RdfOptions, Syntax};

fn main() -> cim_rs::Result<()> {
    let grid = Dataset::load_dir(SCHEMA, "MicroGrid-BE")?;

    // One profile's graph, as a SHACL engine wants it.
    let eq = SCHEMA.profile_by_keyword("EQ").expect("the EQ profile");
    let turtle = cim_rs::rdf::to_string(
        &grid,
        &RdfOptions::new(Syntax::Turtle).profiles(eq.mask()),
    )?;
    print!("{turtle}");
    Ok(())
}
```

```turtle
<urn:uuid:17086487-56ba-4979-b8de-064025a6b4da>
    a cim:ACLineSegment ;
    cim:ACLineSegment.r "2.2"^^xsd:float ;
    cim:ConductingEquipment.BaseVoltage <urn:uuid:a7f1d8de-d658-428a-821b-3a5ae5965fd1> ;
    cim:Equipment.aggregate "false"^^xsd:boolean ;
    cim:IdentifiedObject.name "BE-Line_1" ;
    eu:IdentifiedObject.shortName "BE-L_1" .
```

Or from a shell:

```bash
cim rdf MicroGrid-BE/ --out graphs/              # a Turtle graph per profile with data
cim rdf MicroGrid-BE/ --out graphs/ --ntriples   # for piping into a validator
cim rdf MicroGrid-BE/ --out graphs/ --merged     # the whole model in one graph
```

## The mapping

| CIM | RDF |
|---|---|
| Object with a UUID identifier | IRI `urn:uuid:<uuid>` — 61970-552's own identification rule |
| Object with a non-conforming identifier | Blank node, or an IRI under `RdfOptions::base` |
| Class | `rdf:type <namespace><ClassName>` |
| Attribute | Predicate `<namespace><Class.attr>` |
| Primitive value | Literal typed by `Primitive::xsd_datatype` |
| CIM datatype (`Resistance`, …) | Literal of the primitive it serializes as |
| Enumeration | IRI `<namespace><Enum.literal>` |
| Association | IRI of the target object |
| Compound (`rdf:parseType="Resource"`) | Typed blank node with its fields |
| `md:FullModel` header | A resource with its `md:Model.*` properties |

The datatype mapping is taken from the published shapes rather than guessed. That is why
CIM `Float` becomes **`xsd:float`** and not the `xsd:double` a 64-bit representation would
suggest, and why `Money` becomes `xsd:decimal` and is written without an exponent —
`xsd:decimal`'s lexical space has none.

The header is typed from the generated **Header AP** vocabulary, not hand-written as
strings: `md:Model.created` is an `xsd:dateTime`, `md:Model.version` an `xsd:integer`,
`md:Model.profile` an `xsd:anyURI`. Emitting them all as plain strings fails ENTSO-E's
header shapes on every file.

## Export is per profile, not per model

This is the part that is easy to get wrong. A CGMES profile constrains a reference's target
to the classes **it** declares: the State Variables shapes allow
`SvStatus.ConductingEquipment` to point only at a `CsConverter` or a `VsConverter`, because
those are the only ones SV declares. Validating a *merged* graph against them reports
hundreds of violations no instance file could have had.

A profile mask therefore selects the same slice the CIM/XML writer would, types each object
with the same element-class rule — an SSH graph types an `ACLineSegment` as `cim:Equipment`
— and omits objects the profile says nothing about, exactly as its instance file would.

**Including the headers.** A profile's graph carries the `md:FullModel` of the files that
*serve* it and no others, because that is what its instance file carries. Unscoped headers
make the Steady State Hypothesis graph assert that the model declares the Equipment
profile — valid RDF, correctly typed, accepted by the header shapes, and false.

**A profile the model says nothing about gets no graph.** `cim rdf` writes a file only where
there is something to describe, and names the profiles it skipped on stderr. A graph of
nothing but headers looks like an export and validates like one.

Use `--merged` when the whole model in one graph is what you want, as in a triple store.

### A file that serves several profiles is one graph

"Per profile" is shorthand for "per instance file", and the two differ in CGMES 2.4.15,
where Equipment, Operation and ShortCircuit travel together and ENTSO-E publishes a single
`EquipmentProfile` shape for the lot. Asking for them together writes the graph that file
would contain:

```bash
cim rdf model/ --profile EQ --profile OP --profile SC --out graphs/   # graphs/EQ_OP_SC.ttl
```

From the library it is a mask, which is the same thing `write_profiles` takes:

```rust,no_run
# use cim_rs::prelude::*;
# use cim_rs::cgmes3::SCHEMA;
# use cim_rs::rdf::{RdfOptions, Syntax};
# fn main() -> cim_rs::Result<()> {
# let grid = Dataset::load(SCHEMA, ["EQ.xml"])?;
let mask = ["EQ", "OP", "SC"]
    .iter()
    .filter_map(|k| SCHEMA.profile_by_keyword(k))
    .fold(0, |acc, p| acc | p.mask());
let turtle = cim_rs::rdf::to_string(&grid, &RdfOptions::new(Syntax::Turtle).profiles(mask))?;
# Ok(()) }
```

## Checked by an engine, not by assertion

`cargo xtask shacl` exports each profile through `cim rdf` and hands it to
[`pyshacl`](https://github.com/RDFLib/pySHACL) with ENTSO-E's published shapes, in CI.

A profile the model carries no data for is reported as `no data` rather than as a pass, and
a run in which nothing was validated fails.

**Every profile that carries data, in every published CGMES 3.0 conformity model, conforms**
— `RealGrid` included. The only findings that remain belong to the models: every Steady
State Hypothesis file in the corpus writes `<cim:Equipment rdf:about="…">` carrying nothing
but `inService`, and the SSH shapes require `IdentifiedObject.mRID`. Reproducing that
faithfully is correct; hiding it would not be.

Running SHACL is deliberately **not** this crate's job — there is no mature SHACL engine in
Rust and writing one would be a second project. Emitting data an engine can actually consume
is the job, and it is done.

## Reading RDF back

Deliberately absent. CIM/XML is the exchange format and the crate reads it natively; a
second input syntax would be surface area without a use case until CIM JSON-LD is real.
