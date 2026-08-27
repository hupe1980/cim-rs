//! Standard RDF output: the profile-enriched graph, in N-Triples or Turtle.
//!
//! # Why this exists
//!
//! CIM/XML looks like RDF/XML and is not. IEC 61970-552 predates the W3C RDF/XML
//! recommendation, and the two disagree in ways that matter: `rdf:parseType="Statements"`
//! is not RDF syntax at all, and `rdf:ID="_<uuid>"` denotes `urn:uuid:<uuid>` rather than
//! a fragment of the document. ENTSO-E's own interoperability reporting says so — the
//! int:net IOP report calls CIMXML's differences from "RDFXML *defined by W3C*" gaps, and
//! the CGMES documentation's claim that CIMXML "can be read by available RDF
//! de-serialization software" does not hold.
//!
//! The second gap is larger. **CIM/XML exchanges no datatype information at all.** Every
//! value is element text, so a reader without the profile cannot tell `1` (an integer)
//! from `1` (a float) from `1` (a string) — and ENTSO-E's SHACL shapes constrain 3,137
//! properties by `sh:datatype`. The 2024 int:net report puts it plainly: *"There are no
//! open libraries to natively enhance the data based on the profile definitions"*, and
//! lists providing one as one of three ways out.
//!
//! This module is that step. It writes the model as ordinary RDF with every literal
//! carrying the XSD type the profile assigns it, so the result can be loaded by any RDF
//! toolchain and validated directly against the SHACL shapes ENTSO-E publishes alongside
//! the RDFS this crate is generated from.
//!
// An example has to name a vintage, and a vintage is a feature. See `Dataset::view`.
#![cfg_attr(
    feature = "cgmes3",
    doc = r#"
```no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;
use cim_rs::rdf::{RdfOptions, Syntax};

# fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
let ds = Dataset::load(SCHEMA, ["model_EQ.xml", "model_SSH.xml"])?;
let out = std::fs::File::create("model.ttl")?;
cim_rs::rdf::write(&ds, out, &RdfOptions::new(Syntax::Turtle))?;
# Ok(())
# }
```
"#
)]
//!
//! # The mapping
//!
//! | CIM | RDF |
//! |---|---|
//! | object with a UUID identifier | IRI `urn:uuid:<uuid>` (IEC 61970-552 §identification) |
//! | object with a non-conforming identifier | blank node, or an IRI under [`RdfOptions::base`] |
//! | class | `rdf:type <namespace><ClassName>` |
//! | attribute | predicate `<namespace><Class.attr>` |
//! | primitive value | literal typed by [`Primitive::xsd_datatype`] |
//! | CIM datatype (`Resistance`, …) | literal of the primitive it serializes as |
//! | enumeration | IRI `<namespace><Enum.literal>` |
//! | association | IRI of the target object |
//! | compound (`rdf:parseType="Resource"`) | blank node, typed, with its fields |
//! | `md:FullModel` header | a resource with its `md:Model.*` properties |
//!
//! # What this is not
//!
//! Not an RDF store, and not a SHACL engine. It writes a graph; validating it against
//! shapes is a job for a tool that already does that well. The structural checks this
//! crate performs itself are in [`validate`](mod@crate::validate) and cover what the RDFS
//! justifies without shapes.

use std::io::Write;

use crate::dataset::Dataset;
use crate::error::{Error, Result};
use crate::header::{HeaderValue, MD_NS, ModelHeader};
use crate::mrid::Mrid;
use crate::object::{Compound, Object};
use crate::schema::{AttrId, AttrKind, ClassId, Primitive, ProfileMask, Schema};
use crate::value::Value;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";
/// `rdf:type`, spelled out once. Written on every object and every compound, so it is a
/// constant rather than a `format!` of `RDF_NS` — building it per triple allocated over a
/// million times on a nation-scale model, twice each: once to write it and once to
/// recognise it as the one predicate Turtle abbreviates to `a`.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Which RDF syntax to write.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Syntax {
    /// One triple per line, no prefixes. The line-based interchange syntax every RDF
    /// tool reads, and the one to pipe into a validator.
    #[default]
    NTriples,
    /// Prefixed and grouped by subject. The same graph, for a person to read.
    Turtle,
}

/// Options controlling RDF output.
#[derive(Clone, Debug)]
pub struct RdfOptions {
    pub syntax: Syntax,
    /// Write only values belonging to these profiles; zero means "everything".
    ///
    /// Filtering follows the same rule as the CIM/XML writer
    /// ([`Schema::effective_profiles`]), so validating one profile's graph against that
    /// profile's shapes sees exactly what an export of it would contain.
    pub profiles: ProfileMask,
    /// Emit `rdf:type` for every object and compound.
    ///
    /// On by default: SHACL shapes select their targets with `sh:targetClass`, so a graph
    /// without types is a graph nothing applies to.
    pub types: bool,
    /// Emit the loaded files' `md:FullModel` headers as resources.
    ///
    /// Restricted by [`RdfOptions::profiles`] exactly as the objects are: a graph for one
    /// profile carries the headers of the files that *serve* it and no others, because that
    /// is what the profile's instance file carries.
    pub headers: bool,
    /// Base IRI for identifiers that are not UUIDs.
    ///
    /// IEC 61970-552 requires a UUID and this crate keeps non-conforming identifiers
    /// verbatim, reporting them as
    /// [`Rule::NonConformingMrid`](crate::Rule::NonConformingMrid). Such an identifier has
    /// no `urn:uuid:` form, and inventing one would be a lie about what the document said.
    /// With a base set the identifier becomes `{base}{identifier}`; without one it becomes
    /// a **blank node**, which keeps the graph's shape and its joins but gives the object
    /// no global name.
    pub base: Option<String>,
}

impl Default for RdfOptions {
    fn default() -> Self {
        RdfOptions {
            syntax: Syntax::default(),
            profiles: 0,
            types: true,
            headers: true,
            base: None,
        }
    }
}

impl RdfOptions {
    pub fn new(syntax: Syntax) -> RdfOptions {
        RdfOptions {
            syntax,
            ..Default::default()
        }
    }

    /// Restrict output to a set of profiles.
    pub fn profiles(mut self, profiles: ProfileMask) -> Self {
        self.profiles = profiles;
        self
    }

    /// Name non-conforming identifiers under `base` instead of emitting blank nodes.
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = Some(base.into());
        self
    }
}

/// Write `dataset` as RDF.
pub fn write<W: Write>(dataset: &Dataset, out: W, options: &RdfOptions) -> Result<()> {
    let schema = dataset.schema();
    let mut w = RdfWriter {
        out,
        schema,
        options,
        prefixes: turtle_prefixes(schema),
        blank: 0,
        subject_open: false,
    };
    w.prologue()?;
    if options.headers {
        for h in dataset.headers() {
            if header_serves(schema, h, options.profiles) {
                w.header(h)?;
            }
        }
    }
    // ObjectId order, which is load order: deterministic, and cheaper than sorting a
    // nation-scale model when RDF itself defines no order over a graph.
    for (_, obj) in dataset.iter() {
        w.object(obj)?;
    }
    w.close_subject()?;
    w.out.flush()?;
    Ok(())
}

/// Write `dataset` as RDF into a string.
pub fn to_string(dataset: &Dataset, options: &RdfOptions) -> Result<String> {
    let mut buf = Vec::new();
    write(dataset, &mut buf, options)?;
    String::from_utf8(buf).map_err(|e| Error::NotCimXml(e.to_string()))
}

/// Whether a file's header belongs in a graph restricted to `profiles`.
///
/// The same question the CIM/XML export asks of a file: does this header declare a profile
/// the export serves. A zero mask is the whole model, where every header belongs.
fn header_serves(schema: &Schema, header: &ModelHeader, profiles: ProfileMask) -> bool {
    profiles == 0
        || header
            .profiles
            .iter()
            .filter_map(|iri| schema.profile_by_iri(iri))
            .any(|p| p.mask() & profiles != 0)
}

/// Whether an export restricted to `profiles` would describe any object at all.
///
/// Lets a caller distinguish "this profile is absent from the model" from "this profile
/// conforms" — which a check on the graph's *size* cannot, since a graph of nothing but
/// headers runs to hundreds of lines.
///
/// Deliberately about objects rather than headers: a file may declare Equipment, Operation
/// and ShortCircuit and carry no Operation attribute at all, and a header on its own is not
/// an export of that profile.
pub fn has_content(dataset: &Dataset, profiles: ProfileMask) -> bool {
    let schema = dataset.schema();
    dataset
        .iter()
        .any(|(_, o)| crate::writer::object_has_content_in(schema, o, profiles))
}

/// One of the three things a triple's object can be.
enum Term<'a> {
    Iri(String),
    Blank(String),
    /// Lexical form and datatype IRI; an empty datatype means a plain string literal.
    Literal(String, &'a str),
}

struct RdfWriter<'a, W: Write> {
    out: W,
    schema: &'static Schema,
    options: &'a RdfOptions,
    /// Prefix to abbreviate each schema namespace with, or `None` where abbreviating it
    /// would be wrong. Resolved once, for the reason [`turtle_prefixes`] explains.
    prefixes: Vec<Option<&'static str>>,
    /// Counter for the blank nodes compounds need.
    blank: u64,
    /// Turtle groups triples by subject, so a subject stays open until the next one.
    subject_open: bool,
}

/// Which of a schema's namespaces may be abbreviated in Turtle, and under what name.
///
/// A `@prefix` declaration is global to the document and the last one wins, so a prefix
/// that names two different namespaces silently changes what every abbreviated name after
/// it denotes — output that parses cleanly and means something else, which is the worst
/// kind. Two ways that can happen, and both are live rather than theoretical once
/// third-party profiles are generated as the design promises:
///
/// * a schema namespace whose conventional prefix is `rdf`, `xsd` or `md`, which this
///   writer has already bound to the vocabularies it needs itself;
/// * two schema namespaces that share a prefix, which the generator's fallback naming
///   makes unlikely but does not prevent.
///
/// Either way the namespace is simply not abbreviated: a full IRI is always correct, and a
/// wrong prefixed name never is.
fn turtle_prefixes(schema: &Schema) -> Vec<Option<&'static str>> {
    const RESERVED: [&str; 3] = ["rdf", "xsd", "md"];
    let mut out: Vec<Option<&'static str>> = Vec::with_capacity(schema.namespaces.len());
    for (i, ns) in schema.namespaces.iter().enumerate() {
        // The vocabularies this writer declares itself keep their canonical prefixes and
        // are not re-declared from the table.
        if matches!(ns.iri, RDF_NS | MD_NS) {
            out.push(None);
            continue;
        }
        let clashes = RESERVED.contains(&ns.prefix)
            || schema.namespaces[..i]
                .iter()
                .any(|earlier| earlier.prefix == ns.prefix && earlier.iri != ns.iri);
        out.push((!clashes).then_some(ns.prefix));
    }
    out
}

impl<W: Write> RdfWriter<'_, W> {
    fn turtle(&self) -> bool {
        self.options.syntax == Syntax::Turtle
    }

    /// Turtle's prefix declarations. N-Triples has none and needs none.
    fn prologue(&mut self) -> Result<()> {
        if !self.turtle() {
            return Ok(());
        }
        writeln!(self.out, "@prefix rdf: <{RDF_NS}> .").map_err(Error::Io)?;
        writeln!(self.out, "@prefix xsd: <{XSD_NS}> .").map_err(Error::Io)?;
        writeln!(self.out, "@prefix md: <{MD_NS}> .").map_err(Error::Io)?;
        // Exactly the namespaces `write_abbreviated` will use, so the document never
        // declares a prefix it does not need or one that would shadow another.
        for (ns, prefix) in self.schema.namespaces.iter().zip(&self.prefixes) {
            let Some(prefix) = prefix else { continue };
            writeln!(self.out, "@prefix {}: <{}> .", prefix, ns.iri).map_err(Error::Io)?;
        }
        self.out.write_all(b"\n")?;
        Ok(())
    }

    // -- header ------------------------------------------------------------

    fn header(&mut self, h: &ModelHeader) -> Result<()> {
        let Some(id) = h.id.as_ref() else {
            // A header with no identifier has no subject to hang its properties on.
            return Ok(());
        };
        let subject = self.term_for(id);
        self.close_subject()?;
        if self.options.types {
            let kind = match h.kind {
                crate::header::ModelKind::Full => "FullModel",
                crate::header::ModelKind::Difference => "DifferenceModel",
            };
            let ty = Term::Iri(format!("{MD_NS}{kind}"));
            self.triple(&subject, RDF_TYPE, &ty)?;
        }
        for (local, value) in [
            ("Model.created", h.created.as_deref()),
            ("Model.scenarioTime", h.scenario_time.as_deref()),
            ("Model.description", h.description.as_deref()),
            ("Model.version", h.version.as_deref()),
            (
                "Model.modelingAuthoritySet",
                h.modeling_authority_set.as_deref(),
            ),
        ] {
            if let Some(v) = value {
                let term = self.header_literal(local, v);
                self.triple(&subject, &format!("{MD_NS}{local}"), &term)?;
            }
        }
        for p in &h.profiles {
            let term = self.header_literal("Model.profile", p);
            self.triple(&subject, &format!("{MD_NS}Model.profile"), &term)?;
        }
        for (local, ids) in [
            ("Model.DependentOn", &h.dependent_on),
            ("Model.Supersedes", &h.supersedes),
        ] {
            for m in ids {
                let target = self.term_for(m);
                self.triple(&subject, &format!("{MD_NS}{local}"), &target)?;
            }
        }
        for p in &h.extra {
            let term = match &p.value {
                HeaderValue::Text(v) => Term::Literal(v.clone(), ""),
                HeaderValue::Resource(v) => Term::Iri(v.clone()),
            };
            self.triple(&subject, &format!("{}{}", p.ns, p.local), &term)?;
        }
        self.close_subject()
    }

    /// Type a header value from the header profile's own vocabulary.
    ///
    /// The exchange header is a profile like any other — CGMES 3.0 publishes a Header AP
    /// RDFS, and the generator reads it — so `md:Model.created` is a `DateTime`,
    /// `md:Model.version` an `Integer` and `md:Model.profile` a `URI`, and ENTSO-E's
    /// header shapes constrain each with the matching `sh:datatype`. Emitting them all as
    /// plain strings, which is what hand-writing the header amounts to, fails those shapes
    /// on every file. The schema is the single place that knows, so it is asked.
    fn header_literal(&self, local: &str, text: &str) -> Term<'static> {
        let prim = self
            .schema
            .find_attr(MD_NS, local)
            .map(|a| match self.schema.attr(a).kind {
                AttrKind::Primitive(p) => p,
                AttrKind::Datatype(d) => self.schema.datatype(d).value,
                _ => Primitive::String,
            })
            .unwrap_or(Primitive::String);
        Term::Literal(text.to_owned(), prim.xsd_datatype())
    }

    // -- objects -----------------------------------------------------------

    fn object(&mut self, obj: &Object) -> Result<()> {
        let profiles = self.options.profiles;
        let keep = |s: &crate::object::Slot| {
            profiles == 0 || self.schema.effective_profiles(s.attr, s.profiles) & profiles != 0
        };
        // A profile's graph holds the objects that profile describes and no others.
        // Emitting a bare `rdf:type` for an object with nothing to say in this profile
        // would assert something the corresponding instance file never claims — and
        // ENTSO-E's shapes check exactly that, since a profile constrains a reference's
        // target to the classes *it* declares.
        if profiles != 0 && !obj.slots().iter().any(keep) {
            return Ok(());
        }

        let subject = self.term_for(obj.mrid());
        self.close_subject()?;

        if self.options.types {
            // The same rule the CIM/XML writer uses to pick an element's class: a Steady
            // State Hypothesis graph types an `ACLineSegment` as `cim:Equipment`, because
            // that is the class SSH declares and the only one its shapes expect.
            let class = crate::writer::element_class(self.schema, obj, obj.class(), profiles);
            let ty = Term::Iri(self.class_iri(class));
            self.triple(&subject, RDF_TYPE, &ty)?;
        }
        for slot in obj.slots() {
            if !keep(slot) {
                continue;
            }
            self.property(&subject, slot.attr, &slot.value)?;
        }
        self.close_subject()
    }

    /// Emit one attribute occurrence, recursing into compounds.
    fn property(&mut self, subject: &Term<'_>, attr: AttrId, value: &Value) -> Result<()> {
        let def = self.schema.attr(attr);
        let predicate = format!("{}{}", self.schema.namespace(def.ns).iri, def.name);

        // A compound has no identifier, which is exactly what a blank node is for. Its
        // triples are written after the property that points at it, so the subject
        // currently open has to be closed first.
        if let Value::Compound(c) = value {
            self.blank += 1;
            let node = Term::Blank(format!("_:c{}", self.blank));
            self.triple(subject, &predicate, &node)?;
            self.close_subject()?;
            self.compound(&node, c)?;
            self.close_subject()?;
            return Ok(());
        }

        let term = self.term_for_value(def.kind, value);
        self.triple(subject, &predicate, &term)
    }

    fn compound(&mut self, subject: &Term<'_>, c: &Compound) -> Result<()> {
        if self.options.types {
            let ty = Term::Iri(self.class_iri(c.class()));
            self.triple(subject, RDF_TYPE, &ty)?;
        }
        for (attr, v) in c.values() {
            self.property(subject, *attr, v)?;
        }
        Ok(())
    }

    // -- terms -------------------------------------------------------------

    fn class_iri(&self, class: ClassId) -> String {
        let def = self.schema.class(class);
        format!("{}{}", self.schema.namespace(def.ns).iri, def.name)
    }

    /// The subject or object term denoting `mrid`.
    fn term_for(&self, mrid: &Mrid) -> Term<'static> {
        if mrid.is_uuid() {
            return Term::Iri(mrid.to_urn());
        }
        match &self.options.base {
            Some(base) => Term::Iri(format!("{base}{}", mrid.canonical())),
            None => Term::Blank(blank_label(&mrid.canonical())),
        }
    }

    fn term_for_value(&self, kind: AttrKind, value: &Value) -> Term<'static> {
        // The declared kind decides the datatype, and the stored value decides the
        // lexical form. They can disagree — a malformed document is why `validate` has a
        // datatype check — in which case the value is written as what it actually is,
        // rather than being labelled with a type it does not have.
        match (kind, value) {
            (_, Value::Reference(m)) => self.term_for(m),
            (_, Value::Enum(e)) => {
                let ev = self.schema.enum_value(*e);
                Term::Iri(format!("{}{}", self.schema.namespace(ev.ns).iri, ev.name))
            }
            (AttrKind::Primitive(p), _) => self.literal(p, value),
            (AttrKind::Datatype(dt), _) => self.literal(self.schema.datatype(dt).value, value),
            // Compounds are handled before this point; anything else left here is a
            // malformed value kept as text.
            _ => self.literal(Primitive::String, value),
        }
    }

    fn literal(&self, declared: Primitive, value: &Value) -> Term<'static> {
        // Normally the profile decides the type — that is the whole purpose here. But a
        // document can carry something the profile does not declare (which is what
        // `validate`'s datatype check reports), and labelling that with the declared type
        // would assert something false. Such a value is written as what it actually is.
        let effective = if value.fits(declared) {
            declared
        } else {
            match value {
                Value::Boolean(_) => Primitive::Boolean,
                Value::Integer(_) => Primitive::Integer,
                Value::Float(_) => Primitive::Float,
                _ => Primitive::String,
            }
        };
        let lexical = value.to_lexical_as(effective).unwrap_or_default();
        Term::Literal(lexical, effective.xsd_datatype())
    }

    // -- syntax ------------------------------------------------------------

    fn triple(&mut self, subject: &Term<'_>, predicate: &str, object: &Term<'_>) -> Result<()> {
        if self.turtle() {
            if self.subject_open {
                self.out.write_all(b" ;\n    ")?;
            } else {
                self.write_term(subject)?;
                self.out.write_all(b"\n    ")?;
                self.subject_open = true;
            }
            // `a` is Turtle's keyword for rdf:type, and the one abbreviation every reader
            // of RDF expects to see.
            if predicate == RDF_TYPE {
                self.out.write_all(b"a")?;
            } else {
                self.write_abbreviated(predicate)?;
            }
            self.out.write_all(b" ")?;
            self.write_term(object)?;
            return Ok(());
        }
        self.write_term(subject)?;
        self.out.write_all(b" ")?;
        self.write_iri(predicate)?;
        self.out.write_all(b" ")?;
        self.write_term(object)?;
        self.out.write_all(b" .\n")?;
        Ok(())
    }

    fn close_subject(&mut self) -> Result<()> {
        if self.subject_open {
            self.out.write_all(b" .\n")?;
            self.subject_open = false;
        }
        Ok(())
    }

    /// Write an IRI as a prefixed name where the schema knows its namespace.
    fn write_abbreviated(&mut self, iri: &str) -> Result<()> {
        if let Some((ns, local)) = split_iri(iri)
            && is_turtle_local_name(local)
            && let Some(prefix) = self.prefix_for(ns)
        {
            return write!(self.out, "{prefix}:{local}").map_err(Error::Io);
        }
        self.write_iri(iri)
    }

    fn prefix_for(&self, ns: &str) -> Option<&'static str> {
        match ns {
            RDF_NS => Some("rdf"),
            MD_NS => Some("md"),
            XSD_NS => Some("xsd"),
            _ => self
                .schema
                .namespaces
                .iter()
                .position(|n| n.iri == ns)
                .and_then(|i| self.prefixes[i]),
        }
    }

    fn write_term(&mut self, term: &Term<'_>) -> Result<()> {
        match term {
            Term::Iri(iri) if self.turtle() => self.write_abbreviated(iri),
            Term::Iri(iri) => self.write_iri(iri),
            Term::Blank(label) => self.out.write_all(label.as_bytes()).map_err(Error::Io),
            Term::Literal(lexical, datatype) => {
                self.out.write_all(b"\"")?;
                self.write_escaped_literal(lexical)?;
                self.out.write_all(b"\"")?;
                if datatype.is_empty() {
                    // RDF 1.1: a literal with no datatype *is* an `xsd:string`.
                    return Ok(());
                }
                if self.turtle()
                    && let Some(local) = datatype.strip_prefix(XSD_NS)
                {
                    return write!(self.out, "^^xsd:{local}").map_err(Error::Io);
                }
                self.out.write_all(b"^^")?;
                self.write_iri(datatype)
            }
        }
    }

    /// An IRI reference. The characters an IRIREF may not contain are escaped numerically
    /// rather than dropped, so a non-conforming value cannot break the syntax.
    fn write_iri(&mut self, iri: &str) -> Result<()> {
        self.out.write_all(b"<")?;
        for c in iri.chars() {
            match c {
                '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\' => {
                    write!(self.out, "\\u{:04X}", c as u32).map_err(Error::Io)?
                }
                c if (c as u32) <= 0x20 => {
                    write!(self.out, "\\u{:04X}", c as u32).map_err(Error::Io)?
                }
                c => {
                    let mut b = [0u8; 4];
                    self.out.write_all(c.encode_utf8(&mut b).as_bytes())?;
                }
            }
        }
        self.out.write_all(b">")?;
        Ok(())
    }

    fn write_escaped_literal(&mut self, s: &str) -> Result<()> {
        let bytes = s.as_bytes();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let rep: &[u8] = match b {
                b'\\' => b"\\\\",
                b'"' => b"\\\"",
                b'\n' => b"\\n",
                b'\r' => b"\\r",
                b'\t' => b"\\t",
                _ => continue,
            };
            self.out.write_all(&bytes[start..i])?;
            self.out.write_all(rep)?;
            start = i + 1;
        }
        self.out.write_all(&bytes[start..])?;
        Ok(())
    }
}

/// Split an IRI into `(namespace, local name)` at its last `#` or `/`.
fn split_iri(iri: &str) -> Option<(&str, &str)> {
    let at = iri.rfind(['#', '/'])?;
    Some((&iri[..=at], &iri[at + 1..]))
}

/// Whether `local` can be written after a Turtle prefix without escaping.
///
/// Deliberately narrower than Turtle's `PN_LOCAL`: CIM's property names are
/// `Class.attribute`, so an interior dot must be allowed, but a trailing one would end the
/// statement instead.
fn is_turtle_local_name(local: &str) -> bool {
    !local.is_empty()
        && !local.ends_with('.')
        && local.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A stable blank-node label for a non-conforming identifier.
///
/// It has to be stable rather than merely unique: every reference to the object has to
/// produce the same label, or the graph falls apart into unconnected fragments.
fn blank_label(id: &str) -> String {
    let plain = !id.is_empty()
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if plain {
        return format!("_:id{id}");
    }
    let mut out = String::with_capacity(id.len() * 2 + 4);
    out.push_str("_:x");
    for b in id.bytes() {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_labels_are_stable_and_syntactically_valid() {
        // The same identifier must always give the same label, or references stop joining.
        let id = "c1d5c14b8f8011e08e4d00247eb1f55e";
        assert_eq!(blank_label(id), blank_label(id));
        assert_eq!(blank_label(id), format!("_:id{id}"));
        // Anything that is not a plain name is hex-encoded rather than passed through.
        for weird in ["a b", "é", "_lead", "-lead", ""] {
            let label = blank_label(weird);
            assert!(label.starts_with("_:"), "{weird:?} -> {label}");
            assert!(
                label[2..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{weird:?} -> {label}"
            );
        }
    }

    #[test]
    fn cim_property_names_survive_turtle_abbreviation() {
        // `Class.attribute` is a valid Turtle local name because the dot is interior.
        assert!(is_turtle_local_name("ACLineSegment.r"));
        assert!(is_turtle_local_name("IdentifiedObject.mRID"));
        assert!(is_turtle_local_name("PhaseCode.ABC"));
        // A trailing dot would end the statement.
        assert!(!is_turtle_local_name("Trailing."));
        assert!(!is_turtle_local_name("9leading"));
        assert!(!is_turtle_local_name(""));
    }

    #[test]
    fn iris_split_at_the_last_separator() {
        assert_eq!(
            split_iri("http://iec.ch/TC57/CIM100#ACLineSegment.r"),
            Some(("http://iec.ch/TC57/CIM100#", "ACLineSegment.r"))
        );
        assert_eq!(split_iri("urn:uuid:abc"), None);
    }
}
