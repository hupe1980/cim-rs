+++
title = "Validation"
description = "The structural checks cim-rs derives from the CGMES RDFS vocabulary, the full CIM0001–CIM0020 rule catalogue, and where SHACL takes over."
weight = 40
+++

`cim-rs` performs exactly the checks the RDFS vocabulary can justify: cardinality,
datatypes, class membership, reference targets, profile membership, identifier conformance,
header structure. Semantic rules that ENTSO-E publishes as **SHACL shapes** are deliberately
out of scope — export with [`rdf`](@/docs/rdf.md) and run them with a SHACL engine.

```rust
use cim_rs::prelude::*;
use cim_rs::ValidateOptions;

fn check(grid: &Dataset) -> bool {
    let _defaults = grid.validate();
    let _reduced = grid.validate_with(&ValidateOptions::essential()); // only broken data
    let report = grid.validate_with(&ValidateOptions::thorough());    // + deprecation

    for (rule, count) in report.summary() {
        println!("{rule}: {count}");
    }
    !report.has_errors()
}
```

## Rule catalogue

Codes are stable, so a pipeline can filter and fail on a class of problem without matching
message text. `cim_rs::Rule::ALL` enumerates them.

**Raised by** says where a finding comes from, because they arrive in different reports:
*read* is the `LoadReport` a load returns, *check* is what `validate()` produces. A rule
marked only *read* will never appear in a validation report — `CIM0021` is about which
schema you loaded with, which is settled before validation can begin. Applying a difference
model raises the *read* rules too.

| Code | Rule | Raised by | Meaning |
|---|---|---|---|
| `CIM0001` | `UnknownClass` | read | An element names a class the schema does not define |
| `CIM0002` | `UnknownAttribute` | read · check | A property the schema does not define, or one the object's class does not have |
| `CIM0003` | `InvalidValue` | read | A value could not be parsed as its declared type |
| `CIM0004` | `NonConformingMrid` | read · check | An identifier is not a UUID, or not written as 61970-552 requires |
| `CIM0005` | `DuplicateMrid` | read | One identifier used by two unrelated classes |
| `CIM0006` | `DanglingReference` | check | A reference points outside the dataset |
| `CIM0007` | `MissingRequired` | check | A mandatory attribute is absent |
| `CIM0008` | `CardinalityExceeded` | check | An attribute occurs more often than its multiplicity permits |
| `CIM0009` | `WrongReferenceTarget` | check | A reference points at an object of an unrelated class |
| `CIM0010` | `AttributeNotInProfile` | check | A file carries data belonging to a profile its header does not declare |
| `CIM0011` | `AbstractInstantiated` | read · check | An abstract class has an instance |
| `CIM0012` | `Deprecated` | read · check | A deprecated class or attribute is in use |
| `CIM0013` | `MalformedHeader` | check | The `md:FullModel` header is missing or incomplete |
| `CIM0014` | `UnsatisfiedDependency` | check | A header declares a `Model.DependentOn` that was not loaded |
| `CIM0015` | `Structure` | read · check | A structural problem that did not prevent reading |
| `CIM0016` | `DatatypeMismatch` | check | A stored value does not have the shape its attribute declares |
| `CIM0017` | `FixedValueMismatch` | check | An attribute the schema pins with `cims:isFixed` holds something else |
| `CIM0018` | `ConflictingValue` | check | Two loaded files gave one single-valued attribute two different values |
| `CIM0019` | `IllegalXmlCharacter` | read · check | A value holds a character XML 1.0 cannot represent in any form |
| `CIM0020` | `UnserializableIdentifier` | check | An identifier has no valid `rdf:ID` or `rdf:about` form |
| `CIM0021` | `WrongVintage` | read | The document is written against a different CIM vintage than the schema reading it |

## Checks worth explaining

Several are only *useful* because of a judgement built into them.

### The datatype check is not redundant with parsing

CIM/XML exchanges **no type information at all**. A parser without the profile reads every
value as a string; only the RDFS says `ACLineSegment.r` is a float, or that
`Terminal.phases` must be a `PhaseCode`. ENTSO-E's own guidance tells importers to enrich
the data graph from the RDFS before validating, which is what this check does.

### Missing attributes are judged against what is loaded

An attribute mandatory in Equipment is not reported missing when only a Topology file is
loaded. Two conditions gate the check: the object must take part in a profile that declares
the attribute, and some loaded file must actually *define* the object's class. A Topology
file referring to a `ConnectivityNode` that Equipment introduces carries none of its
mandatory attributes, and is not supposed to.

An association side the schema never serializes (`cims:AssociationUsed = No`) is never
reported either — that side is derived by inversion.

### `CIM0010` is about the header, not the data

`RealGrid`'s Equipment file carries ShortCircuit attributes while declaring only
`CoreEquipment`. That is a header under-declaring its own content, so the finding is a
warning and the data is kept. It is aggregated **per attribute** with a count: reporting it
per value produced 10,000 identical findings where 32 say the same thing.

### `CIM0018` cannot be made from the finished model

Assembly keeps one of two contradictory values, so by the time validation runs the other is
gone. The dataset records the conflict as it happens and the check reads it back.

It is a warning rather than an error, because a change file deliberately paired with the
*old* base model produces them by design — that is a legitimate intermediate state, and
applying the change set resolves it.

### `CIM0002` also guards the store

Every other check walks the object's class attribute list, and so does the writer. A value
filed under an attribute that class does **not** have is therefore examined by nothing and
would be dropped without a word on the next export. Three paths can produce one — a
programmatic `set`, a difference statement, a reclassification — and all three are guarded,
but the finished model is checked anyway, because a defence enforced only at the entrances
is one refactor from not being enforced at all.

### `CIM0019` and `CIM0020` are about what the *syntax* can hold

These two are the only checks that ask about serialization rather than about the model, and
they exist because two constraints of CIM/XML cannot be expressed in the object model — so
nothing upstream of the writer enforces them, and nothing downstream questions them.

**`CIM0019`: not every character can appear in an XML document.** XML 1.0's `Char`
production excludes most of the C0 range, and — this is the part that surprises — forbids a
numeric character reference to it as well. There is no escaped form. A value carrying a
`NUL` makes the whole document unparseable at that byte, and `quick-xml` enforces the
production in neither direction, so nothing but this catches one arriving from a mis-encoded
or corrupted source file. The reader strips the character and says so; the writer strips it
too, since `Object::set` is public; and this check names it in a model that already holds
one.

**`CIM0020`: `rdf:ID` is an XML `NCName`.** Every IEC 61970-552 identifier is a UUID and
`_` followed by a UUID is always a name, so this cannot fire on conforming input. It fires
on the input `cim-rs` deliberately keeps verbatim instead of normalizing away — an object a
producer identified with an absolute IRI, say. `rdf:ID="_http://host/EQ.xml#Sub1"` is
*well-formed XML* and *invalid RDF/XML*, which is exactly the combination a well-formedness
check cannot catch. The writer resolves the common case by writing `rdf:about` with the IRI
itself, which is both valid and what the source document said; an identifier that is
neither a name nor an IRI has no valid form at all, and that is what this rule reports.

Neither is something a well-formedness check can be relied on for: the second is invisible
to one by construction, and the first only shows up in a checker that tests XML's character
range *itself* rather than delegating it to a parser that may share the reader's tolerance.

## Profile coverage

A different question from validity: how much of each profile does this model set actually
carry?

```rust
use cim_rs::prelude::*;

fn coverage(grid: &Dataset) {
    for cov in cim_rs::validate::profile_coverage(grid) {
        println!("{:<6} {:>7} objects {:>9} values", cov.keyword, cov.objects, cov.attributes);
    }
}
```

The count comes from the same rule the writer uses, so the report cannot drift from what an
export would contain.

## Choosing checks

```rust
use cim_rs::ValidateOptions;

fn options() -> ValidateOptions {
    ValidateOptions {
        required_attributes: false,   // e.g. for a partial model set
        deprecation: true,
        max_diagnostics: 50_000,
        ..ValidateOptions::default()
    }
}
```

`for_profiles(mask)` restricts the whole run to one profile's expectations, which is what
you want when validating a model that is only claiming to be an Equipment file.
