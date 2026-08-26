//! A Rust implementation of the IEC Common Information Model for power systems.
//!
//! `cim` reads, navigates, validates and writes CIM grid models in both profile sets
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
//! to a single profile.
//!
//! # Getting started
//!
//! ```no_run
//! use cim::prelude::*;
//! use cim::cgmes3::views::ACLineSegment;
//!
//! # fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//! // Assemble a model from its profile files.
//! let ds = Dataset::load(
//!     cim::cgmes3::SCHEMA,
//!     ["model_EQ.xml", "model_SSH.xml", "model_TP.xml", "model_SV.xml"],
//! )?;
//!
//! // Navigate it with typed views.
//! for line in ds.view::<ACLineSegment>() {
//!     println!("{:?}: r={:?} x={:?}", line.name(), line.r(), line.x());
//! }
//!
//! // Check it, structurally, against the schema.
//! let report = cim::validate::validate(&ds);
//! for d in report.iter() {
//!     println!("{d}");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Design
//!
//! Objects are stored **sparsely**: only attributes actually present are kept. This is
//! not a compromise but a match for the data — a `Terminal` has around thirty possible
//! attributes and a Steady State Hypothesis file sets exactly one. Typed access is
//! provided by generated zero-cost [views](view::TypedView) rather than by generating a
//! struct per class, which keeps profile merging natural and compile times low.
//!
//! Everything model-specific — class and attribute tables, typed views, named constants
//! — is generated from the official RDFS vocabularies. Everything else is written
//! against the [`schema`] interface, so a new CIM vintage is a regeneration rather than a
//! rewrite; CGMES 2.4.15 is supported without a line of vintage-specific runtime code.
//!
//! Two facts about CGMES shape the I/O layer. Common attributes such as
//! `IdentifiedObject.mRID` are declared in almost every profile, so what a file should
//! contain cannot be decided from declarations alone — each value records the profile of
//! the file it came from. And one file normally serves several profiles at once, so
//! [`Dataset::save_as_loaded`] writes a model back as the file set it was read from
//! rather than splitting it apart.
//!
//! # Conformance
//!
//! * Serialization follows IEC 61970-552: `rdf:ID`/`rdf:about` identity, `md:FullModel`
//!   headers, `dm:` difference models.
//! * [`validate`] performs the structural checks the RDFS vocabulary justifies:
//!   cardinality, datatypes, reference targets, profile membership, identifier form.
//!   Semantic rules that ENTSO-E publishes as SHACL shapes are out of scope — run those
//!   with a SHACL engine against the same data.
//! * Reading is tolerant by default ([`ReadOptions::lenient`]), because published models
//!   carry vendor extensions and identifier deviations. Nothing is silently dropped:
//!   every deviation becomes a [`Diagnostic`].

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod dataset;
pub mod error;
pub mod header;
pub mod load;
pub mod mrid;
pub mod object;
pub mod reader;
pub mod schema;
pub mod validate;
pub mod value;
pub mod view;
pub mod writer;

mod generated;

/// The README's examples are compiled as doctests so the front page cannot drift from
/// the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use generated::*;

pub use dataset::{Dataset, InverseIndex, LinkPolicy, ObjectId};
pub use error::{Diagnostic, Error, Report, Result, Rule, Severity};
pub use header::{DifferenceModel, ModelHeader, ModelKind};
pub use mrid::Mrid;
pub use object::Object;
pub use reader::{ReadOptions, Strictness};
pub use schema::{AttrId, ClassId, ProfileId, Schema};
pub use value::Value;
pub use view::{TypedRef, TypedView};
pub use writer::{IdStyle, WriteOptions};

/// The imports most programs need.
///
/// `Result` is deliberately absent: importing an alias named `Result` would shadow
/// `std::result::Result` in the caller's module. Use [`cim::Result`](crate::Result)
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
