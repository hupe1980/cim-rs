//! A Rust implementation of the IEC Common Information Model for power systems.
//!
//! `cim-rs` reads, navigates, validates and writes CIM grid models in both profile sets
//! European transmission system operators use: **CGMES 3.0**
//! (IEC TS 61970-600-1/-2:2021) and **CGMES 2.4.15**.
//!
//! Each vintage is a module behind a feature of the same name — [`cgmes3`] (default) and
//! [`cgmes2`] — holding its schema tables, typed views and named constants. Identifiers
//! are per-vintage and cannot be mixed by accident.
//!
//! # What this crate models
//!
//! A CGMES grid model is not one file. It is a *set* of profile files — Equipment (EQ),
//! Steady State Hypothesis (SSH), Topology (TP), State Variables (SV) and others — that
//! describe the **same objects** from different angles. A `SynchronousMachine` gets its
//! ratings from EQ and its dispatch from SSH, under one identifier.
//!
//! [`Dataset`] is built around that fact: loading several files merges each object's
//! attributes rather than producing duplicates, and writing filters attributes back down
//! to a single profile. Which file contributed a value is remembered, so a model set
//! written back out is the file set it came from.
//!
//! Merging is order-independent for everything except a genuine contradiction — two files
//! giving one single-valued attribute two different values. Resolving that means
//! discarding one of them, so it is recorded rather than swallowed:
//! [`Dataset::merge_conflicts`], reported by [`validate()`] as `CIM0018`.
//!
//! # Getting started
//!
//! [`Dataset::open`] is the shortest path in: it reads the vintage out of the document's own
//! namespace declarations, so a directory, a zip archive or a single file loads without the
//! caller knowing whether it is CGMES 3.0 or 2.4.15. Name the schema explicitly — as below —
//! when a program only ever handles one vintage.
//!
// An example has to name a vintage, and a vintage is a feature — so it is written only
// into a build that has the one it names. See `Dataset::view`.
#![cfg_attr(
    feature = "cgmes3",
    doc = r#"
```no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::views::ACLineSegment;

# fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
// Assemble a model from its profile files. A model set is a directory in practice, so
// [`Dataset::load_dir`] is usually the call; [`Dataset::merge`] combines datasets built
// separately, for assembling one in parallel.
let ds = Dataset::load(
    cim_rs::cgmes3::SCHEMA,
    ["model_EQ.xml", "model_SSH.xml", "model_TP.xml", "model_SV.xml"],
)?;

// Navigate it with typed views.
for line in ds.view::<ACLineSegment>() {
    println!("{:?}: r={:?} x={:?}", line.name(), line.r(), line.x());
}

// Check it, structurally, against the schema.
for d in ds.validate().iter() {
    println!("{d}");
}
# Ok(())
# }
```
"#
)]
//!
//! # Design
//!
//! Objects are stored **sparsely** — only attributes actually present are kept — because a
//! `Terminal` has around thirty possible attributes and a Steady State Hypothesis file sets
//! exactly one. Typed access comes from generated zero-cost [views](view::TypedView) rather
//! than a struct per class, which keeps profile merging natural and compile times low.
//!
//! Everything model-specific is generated from the official RDFS vocabularies; everything
//! else is written against the [`schema`] interface, so a new CIM vintage is a regeneration
//! rather than a rewrite. Three facts about CGMES shape the I/O layer — attributes are
//! declared far more widely than they are used, one file serves several profiles at once,
//! and the form of a document is a property of the class within the profile. The
//! [guide](https://hupe1980.github.io/cim-rs/docs/concepts/) explains each.
//!
//! # Changing a model, not resending it
//!
//! IEC 61970-552 defines `dm:DifferenceModel` as the incremental form of exchange: the
//! statements to retract, then the statements to assert. [`diff`] computes one from a
//! before and an after — [`Dataset::difference_to`] — and [`Dataset::apply_difference`]
//! applies it. Applying a computed difference to its base reproduces the target exactly;
//! that is a test against the published conformity models, not a claim.
//!
//! # Leaving for the wider RDF world
//!
//! CIM/XML looks like RDF/XML and is not, and it carries **no datatype information at
//! all** — every value is element text, so `1` could be an integer, a float or a string.
//! That is why a CGMES file cannot simply be handed to a triple store or to the SHACL
//! shapes ENTSO-E publishes beside the RDFS.
//!
//! [`rdf`] closes that: it writes the model as ordinary N-Triples or Turtle with
//! `urn:uuid:` subjects and every literal typed from the profile. The result loads in any
//! RDF toolchain and validates against ENTSO-E's shapes as it stands.
//!
//! # Conformance
//!
//! * Serialization follows IEC 61970-552: `rdf:ID`/`rdf:about` identity, `md:FullModel`
//!   headers, `rdf:parseType="Resource"` compounds, `dm:` difference models. Both reading
//!   and writing are covered, and a model set re-exports as the files it came from.
//! * Every document written is well-formed XML, checked over the whole published corpus
//!   with namespace resolution, duplicate-attribute detection and XML 1.0's character
//!   range — a tolerant reader accepting its own output proves nothing about anyone else's
//!   parser. Two constraints of the output syntax that the object model cannot express are
//!   enforced at the writer and reported by [`validate()`]: a value must hold only
//!   characters XML can represent (`CIM0019`), and an identifier must have an `rdf:ID` or
//!   `rdf:about` form (`CIM0020`). See [`xml`].
//! * [`validate()`] performs the structural checks the RDFS vocabulary justifies:
//!   cardinality, value datatypes, fixed values, reference targets, profile membership,
//!   identifier form. The datatype check is not redundant with parsing — CIM/XML exchanges
//!   no type information at all, so a value's type is only decidable against the profile.
//!   Semantic rules ENTSO-E publishes as SHACL shapes are out of scope: export with
//!   [`rdf`] and run them with a SHACL engine.
//! * Reading is tolerant by default ([`ReadOptions::lenient`]), because published models
//!   carry vendor extensions and identifier deviations. Nothing is silently dropped: every
//!   deviation becomes a [`Diagnostic`] with a stable rule code (`CIM0001`–`CIM0021`) and,
//!   from parsing, a byte offset that [`line_and_column`] turns into a line. The report
//!   grows with how broken the input is, so [`ReadOptions::max_diagnostics`] bounds it and
//!   the reader says when it stopped.
//! * Identifiers are compared by the UUID they denote, not by how a producer spelled it.
//!   Published CGMES 2.4.15 files write `rdf:ID="_1fa19c281c8f4e1eaad9e1cab70f923e"`, and
//!   references as absolute IRIs; read as opaque text, such an object loses its
//!   `urn:uuid:` name in RDF and splits in two against a file that spells the same UUID
//!   conventionally. The spelling is remembered so re-export reproduces the document; the
//!   identity does not depend on it. See [`MridForm`].
//!
//! # A command line
//!
//! The `cli` feature builds `cim`, a tool over this same API: `cim info`, `validate`,
//! `export`, `rdf`, `diff` and `schema` over a model set given as files, archives or
//! directories. `cargo install cim-rs --features cli`.
//!
//! # Further reading
//!
//! This page is orientation and the rest of these docs are the reference. The **guide** —
//! CGMES background, one page per task, and the full rule catalogue — is at
//! <https://hupe1980.github.io/cim-rs/>.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod dataset;
pub mod diff;
pub mod error;
pub mod header;
pub mod load;
pub mod mrid;
pub mod object;
pub mod rdf;
pub mod reader;
pub mod schema;
pub mod validate;
pub mod value;
pub mod view;
pub mod writer;
pub mod xml;

mod generated;

/// The README's examples are compiled as doctests so the front page cannot drift from
/// the API.
///
/// Gated on `cgmes3` for the same reason the site pages are: the examples name a vintage,
/// because an example that named none would not be an example of anything. Without the
/// gate `cargo test --no-default-features --features cgmes2` failed on the front page
/// rather than on the code — a failure CI never saw, since the feature matrix runs
/// `clippy --all-targets`, which does not compile doctests.
#[cfg(all(doctest, feature = "cgmes3"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// The documentation site's examples, compiled for the same reason.
///
/// A guide whose code does not build is worse than no guide, and a guide is exactly the
/// thing nobody re-runs. `site/` is repository machinery rather than published crate
/// content, so this is gated on `doctest`: `cfg` stripping happens before macro expansion,
/// which means an ordinary build — including the one `cargo package` runs to verify the
/// tarball — never looks for the files.
#[cfg(all(doctest, feature = "cgmes3"))]
mod site_doctests {
    macro_rules! page {
        ($name:ident, $path:literal) => {
            #[doc = include_str!($path)]
            pub struct $name;
        };
    }
    page!(GettingStarted, "../site/content/docs/getting-started.md");
    page!(Concepts, "../site/content/docs/concepts.md");
    page!(
        ReadingWriting,
        "../site/content/docs/reading-and-writing.md"
    );
    page!(Validation, "../site/content/docs/validation.md");
    page!(Rdf, "../site/content/docs/rdf.md");
    page!(
        DifferenceModels,
        "../site/content/docs/difference-models.md"
    );
    page!(Performance, "../site/content/docs/performance.md");
    page!(Landing, "../site/content/_index.md");
}

// Re-exported so a vintage reads as `cim_rs::cgmes3`. With no vintage feature enabled the
// module is empty and the glob imports nothing, which is the point of the feature.
#[allow(unused_imports)]
pub use generated::*;

pub use dataset::{Dataset, InverseIndex, LinkPolicy, MergeConflict, ObjectId};
pub use diff::{DiffOptions, DiffReport};
pub use error::{Diagnostic, Error, Report, Result, Rule, Severity, line_and_column};
pub use header::{
    DifferenceModel, HeaderProperty, HeaderValue, ModelHeader, ModelKind, Statement, StatementValue,
};
pub use load::{LoadReport, SaveReport, instance_files};
pub use mrid::{Mrid, MridForm};
pub use object::{Compound, Object, Slot};
pub use rdf::{RdfOptions, Syntax};
pub use reader::{ReadOptions, Strictness};
pub use schema::{AttrId, AttrKind, ClassId, Mult, Primitive, ProfileId, ProfileMask, Schema};
pub use validate::{ValidateOptions, validate, validate_with};
pub use value::Value;
pub use view::{TypedRef, TypedView};
pub use writer::{IdStyle, WriteOptions};
pub use xml::IdentifierForm;

/// The imports most programs need.
///
/// `Result` is deliberately absent: importing an alias named `Result` would shadow
/// `std::result::Result` in the caller's module. Use [`cim_rs::Result`](crate::Result)
/// explicitly where you want it.
pub mod prelude {
    pub use crate::dataset::{Dataset, LinkPolicy, ObjectId};
    pub use crate::error::{Diagnostic, Report, Severity};
    pub use crate::mrid::Mrid;
    pub use crate::object::Object;
    pub use crate::reader::ReadOptions;
    pub use crate::schema::{AttrId, ClassId, ProfileId};
    pub use crate::value::Value;
    pub use crate::view::{TypedRef, TypedView};
    pub use crate::writer::{IdStyle, WriteOptions};
}
