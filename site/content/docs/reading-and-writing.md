+++
title = "Reading and writing"
description = "Reading CIM/XML with cim-rs: lenient and strict modes, structured diagnostics, zip archives, and writing a model back as the file set it came from."
weight = 30
+++

## Reading

The reader is a streaming pull parser over the IEC 61970-552 subset of RDF/XML. It never
buffers the document.

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let mut grid = Dataset::new(SCHEMA);
    let load = grid.load_files(["EQ.xml", "SSH.xml"], &ReadOptions::lenient())?;

    // Nothing is ever silently dropped: deviations become structured diagnostics.
    for d in load.report.iter() {
        // warning[CIM0002] EQ.xml@8213: unknown property <cim:Vendor.x>, skipped
        println!("{d}");
    }
    Ok(())
}
```

### Lenient is the default, and deliberately so

Published models carry vendor extensions and identifier deviations. Refusing them would
make the library useless for real data, so `ReadOptions::lenient()` reports and continues.
`ReadOptions::strict()` turns anything the schema does not define into a hard error.

Reading is forgiving in specific, documented ways — and never silently:

| Deviation | What happens |
|---|---|
| A UUID written without hyphens | Keeps its identity *and* its spelling; flagged `CIM0004` |
| A non-UUID identifier | Kept verbatim; flagged `CIM0004` |
| A reference written as an absolute IRI | Resolves to the UUID in its fragment |
| An enumeration literal under the wrong namespace | Recovered by qualified name; flagged `CIM0003` |
| An unparseable value | Dropped, object kept; flagged `CIM0003` |
| A comma that could be a decimal mark *or* a thousands separator | Refused rather than guessed at |
| An unknown class or property | Skipped; flagged `CIM0001` / `CIM0002` |
| Markup nested inside a value | Skipped with its content; the value stays its own element's text |

### Tolerance costs a report

Everything the reader tolerates it also has to *say*, so the report grows with how broken
the input is — the one place a reader's cost is not proportional to what its input contains.
A file whose every element is unknown produces one finding per element.

```rust
use cim_rs::prelude::*;

fn tighter() -> ReadOptions {
    ReadOptions::lenient().with_max_diagnostics(100)
}
```

The default cap is 10,000. Past it the reader keeps reading and stops recording, closing the
report with a line that says so — a silently truncated report would be worse than a long one.

### Diagnostics

```rust
use cim_rs::prelude::*;

fn describe(report: &cim_rs::Report) {
    for d in report.iter() {
        // `object` is an `Mrid` and `attribute` the schema's own name, not rendered text.
        println!("{} {:?} {:?} {:?}", d.rule, d.severity, d.object, d.attribute);
        if let (Some(source), Some(at)) = (&d.source, d.position) {
            println!("  {source} at byte {at}");
        }
    }
}
```

`object` is an `Mrid` and `attribute` the schema's own name, not rendered text — so you can
group findings by object or look one up without parsing a message back into data.

`position` is a **byte offset**, not a line number: the reader is streaming and never holds
the document, so a line number would cost either a second pass or an index proportional to
the file. `cim_rs::line_and_column(&text, offset)` converts one where a human has to read it.

### Zip archives

CGMES model sets are routinely distributed as archives. With the `zip` feature, an archive
is just another input:

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let grid = Dataset::load(SCHEMA, ["MicroGrid_BE_v2.zip"])?;
    println!("{} objects", grid.len());
    Ok(())
}
```

`cim_rs::instance_files(path)` walks a directory (or returns a single file), keeping the
inputs that look like CIM and sorting them so a load is reproducible.

### Headers

Every instance file starts with an `md:FullModel` header declaring what it contains, which
profiles it conforms to and which models it depends on. They are first-class, because
assembling a multi-file model correctly requires them:

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let grid = Dataset::load_dir(SCHEMA, "MicroGrid-BE")?;
    for header in grid.headers() {
        println!("{:?} {:?} {:?}", header.source, header.profiles, header.scenario_time);
    }

    // Or read only the headers, to decide what to load.
    let plan = Dataset::peek_headers(SCHEMA, ["EQ.xml", "SSH.xml"])?;
    println!("{} file(s) inspected", plan.len());
    Ok(())
}
```

Header properties outside the `md:` vocabulary — DCAT terms, vendor extensions — are
preserved with the prefix, namespace and value form they were read with, so a header
round-trips unchanged rather than being flattened into `md:` element text.

## Writing

Output is deterministic: objects in mRID order, attributes in schema order. Re-writing an
unchanged model is byte-identical, so version diffs stay readable.

### Writing a model back

```rust,no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;

fn main() -> cim_rs::Result<()> {
    let grid = Dataset::load_dir(SCHEMA, "MicroGrid-BE")?;
    let dir = std::path::Path::new("out");

    // As the file set it was read from, headers included.
    let saved = grid.save_as_loaded(dir)?;
    println!("{} file(s) written", saved.written.len());

    // Or one file per profile, for a model built programmatically.
    let written = grid.save_all_profiles(dir, "MyModel")?;
    println!("{} profile file(s)", written.len());
    Ok(())
}
```

`save_as_loaded` is the faithful export. It writes one file per loaded header, carrying that
header, the profiles it declared and the objects that came from it — because selecting by
profile alone would put every authority's equipment in every authority's file in a merged
common grid model.

### The form of the document

Two rules govern what the writer emits, and both are read from the vocabulary rather than
assumed.

**Which class names the element.** An instance file may only name classes its own profiles
declare, and only one whose mandatory attributes the data supplies. A Steady State
Hypothesis file therefore writes `<cim:Equipment rdf:about="…">` for an `ACLineSegment`.
Emitting the most specific class instead produces a document that fails the profile's own
SHACL shapes.

**`rdf:ID` versus `rdf:about`**, per class per profile — see
[Concepts](@/docs/concepts.md).

You can ask what the writer would do without rendering anything:

```rust
use cim_rs::cgmes3::{SCHEMA, classes};

fn how_is_a_terminal_identified_in_ssh() -> cim_rs::writer::IdStyle {
    let ssh = SCHEMA.profile_by_keyword("SSH").expect("the SSH profile");
    cim_rs::writer::id_style_for(SCHEMA, classes::Terminal, ssh.mask())
}
```

### Models built in memory

A model built programmatically has no `md:FullModel` identifier, and IEC 61970-552 requires
one. A random UUID would make every export of an unchanged model a different document, so
`cim-rs` derives one from `Dataset::content_id()` — a version-5 UUID over what the model
actually contains. Export the same model twice and the documents are identical; change one
value and the identifier changes. No clock, no random source, nothing to configure.

The `build_model` example shows the whole write path with no input files at all.

## Output has to satisfy someone else's parser

A tolerant reader accepting its own output proves nothing. Every document the writer
produces is checked against the XML and XML Namespaces recommendations — balanced elements,
no duplicate attribute, every prefix bound before use — over the whole published corpus.

That check is not theoretical. It is what found that every file the writer emitted carried
`xmlns:md` twice: a duplicate attribute is a well-formedness error, and no CIM-tolerant
reader notices while every conforming XML parser refuses the document at byte 2.
