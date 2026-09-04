//! Writer for CIM/XML instance documents (IEC 61970-552).
//!
//! Output is deterministic: objects are emitted in ascending mRID order and attributes
//! in schema order, so re-writing an unchanged model yields byte-identical output and
//! diffs between model versions stay readable.

use std::io::Write;

use crate::dataset::Dataset;
use crate::error::{Error, Result};
use crate::header::{
    DM_NS, DifferenceModel, HeaderValue, MD_NS, ModelHeader, ModelKind, Statement, StatementValue,
};
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::{AttrKind, ClassDef, ClassId, NsId, Primitive, ProfileId, ProfileMask, Schema};
use crate::value::Value;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// How to identify objects in the output.
///
/// IEC 61970-552 distinguishes the file that *introduces* an object from a file that adds
/// to one defined elsewhere, and the RDFS records which is which per class per profile:
/// a class a profile only refers to carries `cims:stereotype Description`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum IdStyle {
    /// Decide per object from the schema — the correct behaviour, and the default.
    ///
    /// An object is written with `rdf:ID` when some profile in the write set *defines*
    /// its class ([`ClassDef::defined_in`](crate::schema::ClassDef::defined_in)), and with
    /// `rdf:about` otherwise. This is not a per-file choice: a Topology file identifies
    /// `TopologicalNode` with `rdf:ID` because Topology introduces it, and `Terminal` with
    /// `rdf:about` because Equipment did. A single profile keyword cannot express that,
    /// and assuming one silently rewrote 49,255 `rdf:ID`s as `rdf:about` in the published
    /// `RealGrid` State Variables file.
    #[default]
    Auto,
    /// Force `rdf:ID="_<uuid>"` for every object.
    RdfId,
    /// Force `rdf:about="#_<uuid>"` for every object.
    RdfAbout,
}

/// Options controlling serialization.
/// Where a document's `md:FullModel` header comes from.
///
/// IEC 61970-552 gives every instance file a header, and a consumer needs it: it is what
/// says which profiles the file serves and which models it depends on. Deriving one is
/// therefore the default, so that the obvious call produces a document another
/// implementation will accept — writing none has to be asked for.
///
/// The derivation belongs here rather than in the callers: two entry points that each
/// decide for themselves produce two behaviours, and only one of them ends up documented.
#[derive(Clone, Debug, Default)]
pub enum HeaderSource {
    /// Derive one from the dataset: identified by its content, declaring the profiles
    /// actually written.
    ///
    /// Deterministic and clock-free — the identifier is an RFC 4122 v5 UUID over the
    /// model's content, so the same model exports to the same header twice.
    #[default]
    Derive,
    /// Write this header, which is how a model is written back as the file set it came
    /// from. An identifier is filled in from the model's content if it has none.
    ///
    /// Boxed because a `ModelHeader` is large next to the other two answers, and this
    /// enum is copied into every `WriteOptions`.
    Given(Box<ModelHeader>),
    /// Write no header. Not a conforming instance file.
    Omit,
}

#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// Write only attributes belonging to these profiles; zero means "everything".
    ///
    /// This is a *set*, not a single profile, because one instance file routinely serves
    /// several: CGMES normally exchanges Equipment, Operation and ShortCircuit together,
    /// and a file that declared only one of them would not reproduce its input.
    pub profiles: ProfileMask,
    pub id_style: IdStyle,
    /// Indent nested elements. Off produces smaller files.
    pub pretty: bool,
    /// Where the document's `md:FullModel` comes from.
    pub header: HeaderSource,
    /// Emit the XML declaration.
    pub xml_declaration: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            profiles: 0,
            id_style: IdStyle::Auto,
            pretty: true,
            header: HeaderSource::default(),
            xml_declaration: true,
        }
    }
}

impl WriteOptions {
    /// Write one profile's attributes, as a conforming instance file.
    pub fn profile(profile: ProfileId) -> WriteOptions {
        WriteOptions::profiles(profile.mask())
    }

    /// Write the attributes of every profile in `profiles`.
    pub fn profiles(profiles: ProfileMask) -> WriteOptions {
        WriteOptions {
            profiles,
            ..Default::default()
        }
    }

    /// Write `header` rather than a derived one.
    pub fn with_header(mut self, header: ModelHeader) -> Self {
        self.header = HeaderSource::Given(Box::new(header));
        self
    }

    /// Write no header at all.
    ///
    /// The result is not a conforming instance file. Use it for a fragment, or where the
    /// caller assembles the document itself.
    pub fn headerless(mut self) -> Self {
        self.header = HeaderSource::Omit;
        self
    }
    pub fn with_id_style(mut self, style: IdStyle) -> Self {
        self.id_style = style;
        self
    }
    pub fn compact(mut self) -> Self {
        self.pretty = false;
        self
    }
}

/// Write every object of `dataset` as one CIM/XML document.
pub fn write<W: Write>(dataset: &Dataset, out: W, options: &WriteOptions) -> Result<()> {
    let ids: Vec<_> = dataset.iter().map(|(id, _)| id).collect();
    write_objects(dataset, ids.into_iter(), out, options)
}

/// Write a chosen subset of objects as one CIM/XML document.
pub fn write_objects<W: Write, I>(
    dataset: &Dataset,
    objects: I,
    out: W,
    options: &WriteOptions,
) -> Result<()>
where
    I: Iterator<Item = crate::dataset::ObjectId>,
{
    let schema = dataset.schema();
    let mut w = Writer::new(out, schema, options.pretty);

    // Deterministic order: sort by mRID so output does not depend on load order.
    let mut list: Vec<&Object> = objects.filter_map(|id| dataset.get(id)).collect();
    list.sort_by(|a, b| a.mrid().cmp(b.mrid()));

    w.start_document(options)?;
    // IEC 61970-552 requires `md:FullModel rdf:about`, and a header a caller built by hand
    // may not have one. The model's content identifier stands in: deterministic, distinct
    // per model, and derived rather than invented — where the nil UUID would be a document
    // this crate's own validator rejects.
    match &options.header {
        HeaderSource::Omit => {}
        HeaderSource::Given(h) if h.id.is_some() => w.header(h)?,
        HeaderSource::Given(h) => {
            let mut h = h.clone();
            h.id = Some(dataset.content_id());
            w.header(&h)?;
        }
        HeaderSource::Derive => w.header(&derive_header(dataset, options.profiles))?,
    }
    for obj in list {
        w.object(obj, options)?;
    }
    w.end_document()
}

/// Write one profile's slice of a dataset.
pub fn write_profile<W: Write>(
    dataset: &Dataset,
    profile: ProfileId,
    out: W,
    options: &WriteOptions,
) -> Result<()> {
    write_profiles(dataset, profile.mask(), out, options)
}

/// Write the slice of a dataset belonging to a set of profiles.
///
/// This is how a multi-profile model is exported back to the file set it came from: a
/// CGMES Equipment file that declared Equipment, Operation and ShortCircuit is rebuilt by
/// passing all three, rather than being split into three files.
pub fn write_profiles<W: Write>(
    dataset: &Dataset,
    profiles: ProfileMask,
    out: W,
    options: &WriteOptions,
) -> Result<()> {
    let schema = dataset.schema();
    let mut opts = options.clone();
    opts.profiles = profiles;

    let ids: Vec<_> = dataset
        .iter()
        .filter(|(_, o)| object_has_content_in(schema, o, profiles))
        .map(|(id, _)| id)
        .collect();
    write_objects(dataset, ids.into_iter(), out, &opts)
}

/// Write a difference model (IEC 61970-552 `dm:DifferenceModel`).
///
/// The counterpart of [`read_difference`](crate::reader::read_difference): a change file
/// read in can be written back out. Statements are grouped by subject, reverse before
/// forward, matching the published conformity change sets.
///
/// The difference's own header is used unless `options` supplies one.
pub fn write_difference<W: Write>(
    schema: &'static Schema,
    diff: &DifferenceModel,
    out: W,
    options: &WriteOptions,
) -> Result<()> {
    let mut header = match &options.header {
        HeaderSource::Given(h) => (**h).clone(),
        // A change set is *about* a model, so a derived header cannot be built from data
        // this function does not have. The difference's own header is the answer to both
        // `Derive` and `Omit`: a `dm:DifferenceModel` element is the document's root
        // content, not decoration that can be left out.
        _ => diff.header.clone(),
    };
    header.kind = ModelKind::Difference;

    let mut w = Writer::new(out, schema, options.pretty);
    // Statement predicates may name namespaces outside the schema's own set — a difference
    // can carry a vendor extension — so they are bound before the root element is written.
    for s in diff.reverse.iter().chain(&diff.forward) {
        w.bind_statement_namespaces(s);
    }
    let mut opts = options.clone();
    opts.header = HeaderSource::Given(Box::new(header));
    w.start_document(&opts)?;

    let HeaderSource::Given(h) = &opts.header else {
        unreachable!("set to Given above")
    };
    w.header_open(h)?;
    w.header_body(h)?;
    // 61970-552 lists what is being replaced before what replaces it.
    w.statements("reverseDifferences", &diff.reverse)?;
    w.statements("forwardDifferences", &diff.forward)?;
    w.header_close(h)?;
    w.end_document()
}

/// Whether a stored value belongs in an instance file serving `profiles`.
///
/// See [`Schema::effective_profiles`] for the rule. A zero mask means "no filtering".
#[inline]
fn slot_belongs_to(schema: &Schema, slot: &crate::object::Slot, profiles: ProfileMask) -> bool {
    profiles == 0 || schema.effective_profiles(slot.attr, slot.profiles) & profiles != 0
}

/// Whether an object would serialize any attribute in `profiles`.
pub fn object_has_content_in(schema: &Schema, obj: &Object, profiles: ProfileMask) -> bool {
    obj.slots()
        .iter()
        .any(|s| slot_belongs_to(schema, s, profiles))
}

/// The prefix bindings of one output document.
///
/// A document's `xmlns:` declarations have to satisfy two constraints that writing them
/// from several independent places violates easily, and both produce output that is not
/// well-formed XML rather than output that is merely ugly:
///
/// * **Each prefix may be declared at most once on an element.** The schema's namespace
///   table already contains `md` and `dm`, so emitting them again for the header is a
///   duplicate attribute — which no CIM-tolerant reader notices and every XML parser
///   rejects outright.
/// * **A prefix must exist before it is used.** A header property read from a document
///   that bound the *default* namespace has no prefix at all, and `<:Model.x>` is not a
///   name.
///
/// Bindings are therefore resolved once, up front: every namespace the document can
/// mention is registered here, a prefix is minted where the preferred one is taken by a
/// different IRI, and the writer asks this type for the prefix of everything it emits.
#[derive(Debug, Default)]
struct Namespaces {
    /// `(prefix, IRI)` in declaration order.
    decls: Vec<(String, String)>,
    /// Prefix chosen for each [`NsId`] of the schema.
    by_ns: Vec<usize>,
}

impl Namespaces {
    /// Bind `iri`, preferring `want` as its prefix, and return the binding's index.
    ///
    /// An IRI already bound keeps its existing prefix — that is what makes the schema's
    /// `md` entry and the header's `md` requirement the same declaration instead of two.
    fn bind(&mut self, want: &str, iri: &str) -> usize {
        if let Some(i) = self.decls.iter().position(|(_, n)| n == iri) {
            return i;
        }
        // `xml` and `xmlns` are reserved by the XML Namespaces recommendation, and an
        // empty prefix cannot name an element in a document that uses no default
        // namespace, so both fall back to a generated name.
        let base = match want {
            "" | "xml" | "xmlns" => "ns",
            other => other,
        };
        let mut prefix = base.to_owned();
        let mut n = 1;
        while self.decls.iter().any(|(p, _)| *p == prefix) {
            n += 1;
            prefix = format!("{base}{n}");
        }
        self.decls.push((prefix, iri.to_owned()));
        self.decls.len() - 1
    }

    /// Bindings for a document written against `schema`.
    ///
    /// The schema's full namespace set is declared whether or not a given document uses
    /// all of it, so that output is stable across models and matches the published files.
    fn for_schema(schema: &Schema) -> Namespaces {
        let mut ns = Namespaces::default();
        // `rdf` first and by that name: it is the one prefix IEC 61970-552 fixes, and the
        // vocabularies themselves sometimes bind it under a generated name.
        ns.bind("rdf", RDF_NS);
        ns.by_ns = schema
            .namespaces
            .iter()
            .map(|n| ns.bind(n.prefix, n.iri))
            .collect();
        ns
    }

    fn prefix(&self, ns: NsId) -> &str {
        self.by_ns
            .get(ns.index())
            .map_or("ns", |&i| self.decls[i].0.as_str())
    }

    fn prefix_at(&self, index: usize) -> &str {
        &self.decls[index].0
    }

    /// The prefix bound to `iri`, if the document declares it.
    fn prefix_of_iri(&self, iri: &str) -> Option<&str> {
        self.decls
            .iter()
            .find(|(_, n)| n == iri)
            .map(|(p, _)| p.as_str())
    }
}

struct Writer<W: Write> {
    out: W,
    schema: &'static Schema,
    pretty: bool,
    ns: Namespaces,
    /// Prefix index for each entry of the header's `extra` list, in order.
    extra_prefixes: Vec<usize>,
}

impl<W: Write> Writer<W> {
    fn new(out: W, schema: &'static Schema, pretty: bool) -> Writer<W> {
        Writer {
            out,
            schema,
            pretty,
            ns: Namespaces::for_schema(schema),
            extra_prefixes: Vec::new(),
        }
    }

    /// Register the namespaces a difference statement mentions.
    fn bind_statement_namespaces(&mut self, s: &Statement) {
        if let Some(q) = &s.class {
            self.ns.bind(suggest_prefix(&q.ns), &q.ns);
        }
        self.ns
            .bind(suggest_prefix(&s.predicate_ns), &s.predicate_ns);
    }

    /// The prefix under which the model-description vocabulary is written here.
    fn md(&self) -> String {
        self.ns.prefix_of_iri(MD_NS).unwrap_or("md").to_owned()
    }
    /// The prefix under which the difference-model vocabulary is written here.
    fn dm(&self) -> String {
        self.ns.prefix_of_iri(DM_NS).unwrap_or("dm").to_owned()
    }

    fn start_document(&mut self, options: &WriteOptions) -> Result<()> {
        // Every prefix the document can use is resolved before anything is written, so a
        // prefix is declared exactly once and always before its first use.
        self.ns.bind("md", MD_NS);
        self.ns.bind("dm", DM_NS);
        // A given header may carry vendor properties in namespaces the schema knows
        // nothing about; a derived one never does.
        self.extra_prefixes = match &options.header {
            HeaderSource::Given(h) => h
                .extra
                .iter()
                .map(|p| self.ns.bind(&p.prefix, &p.ns))
                .collect(),
            _ => Vec::new(),
        };

        if options.xml_declaration {
            self.out
                .write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
        }
        // `rdf` is bound first and to the RDF namespace, so the literal prefix used for
        // `rdf:RDF`, `rdf:ID`, `rdf:resource` and `rdf:parseType` is always correct.
        self.out.write_all(b"<rdf:RDF")?;
        let decls = std::mem::take(&mut self.ns.decls);
        for (prefix, iri) in &decls {
            write!(self.out, " xmlns:{prefix}=\"").map_err(Error::Io)?;
            self.escape_attr(iri)?;
            self.out.write_all(b"\"")?;
        }
        self.ns.decls = decls;
        self.out.write_all(b">\n")?;
        Ok(())
    }

    fn end_document(&mut self) -> Result<()> {
        self.out.write_all(b"</rdf:RDF>\n")?;
        self.out.flush()?;
        Ok(())
    }

    fn header(&mut self, h: &ModelHeader) -> Result<()> {
        self.header_open(h)?;
        self.header_body(h)?;
        self.header_close(h)
    }

    fn header_element(&self, h: &ModelHeader) -> (String, &'static str) {
        match h.kind {
            ModelKind::Full => (self.md(), "FullModel"),
            ModelKind::Difference => (self.dm(), "DifferenceModel"),
        }
    }

    /// Open the header element.
    ///
    /// `write_objects` fills in a missing identifier from the model's content before
    /// reaching here; a difference model derives one of its own. The nil UUID is the last
    /// resort for a caller that reached the writer some other way, and it is deliberately
    /// the *conforming-looking* wrong answer rather than an omitted attribute, because
    /// `validate` reports it and an absent `rdf:about` would make the document unreadable.
    fn header_open(&mut self, h: &ModelHeader) -> Result<()> {
        let (prefix, elem) = self.header_element(h);
        let about =
            h.id.as_ref()
                .map(Mrid::to_urn)
                .unwrap_or_else(|| Mrid::nil().to_urn());
        self.indent(1)?;
        write!(self.out, "<{prefix}:{elem} rdf:about=\"").map_err(Error::Io)?;
        self.escape_attr(&about)?;
        self.out.write_all(b"\">\n")?;
        Ok(())
    }

    fn header_close(&mut self, h: &ModelHeader) -> Result<()> {
        let (prefix, elem) = self.header_element(h);
        self.indent(1)?;
        writeln!(self.out, "</{prefix}:{elem}>").map_err(Error::Io)?;
        Ok(())
    }

    fn header_body(&mut self, h: &ModelHeader) -> Result<()> {
        // Fixed order so headers are diffable across writes.
        self.header_text(h.created.as_deref(), "Model.created")?;
        self.header_text(h.scenario_time.as_deref(), "Model.scenarioTime")?;
        self.header_text(h.description.as_deref(), "Model.description")?;
        self.header_text(h.version.as_deref(), "Model.version")?;
        self.header_text(
            h.modeling_authority_set.as_deref(),
            "Model.modelingAuthoritySet",
        )?;
        for p in &h.profiles {
            self.header_text(Some(p), "Model.profile")?;
        }
        for d in &h.dependent_on {
            self.header_resource(&d.to_urn(), "Model.DependentOn")?;
        }
        for s in &h.supersedes {
            self.header_resource(&s.to_urn(), "Model.Supersedes")?;
        }
        // Prefixes for these were resolved at `start_document`, because a header property
        // may use a prefix the schema does not know, or none at all.
        let prefixes: Vec<String> = self
            .extra_prefixes
            .iter()
            .map(|&i| self.ns.prefix_at(i).to_owned())
            .collect();
        for (p, prefix) in h.extra.iter().zip(&prefixes) {
            match &p.value {
                HeaderValue::Text(v) => self.header_prop_text(prefix, &p.local, v)?,
                HeaderValue::Resource(v) => self.header_prop_resource(prefix, &p.local, v)?,
            }
        }
        Ok(())
    }

    /// Write one `dm:forwardDifferences` / `dm:reverseDifferences` statement set.
    ///
    /// Statements about the same subject share one `rdf:Description`, and a statement group
    /// that named a class keeps it: a difference may reclassify an object, and the class
    /// element is how it says so.
    fn statements(&mut self, local: &str, statements: &[Statement]) -> Result<()> {
        if statements.is_empty() {
            return Ok(());
        }
        let dm = self.dm();
        self.indent(2)?;
        writeln!(self.out, "<{dm}:{local} rdf:parseType=\"Statements\">").map_err(Error::Io)?;

        let mut i = 0;
        while i < statements.len() {
            let subject = &statements[i].subject;
            let class = &statements[i].class;
            let defines = statements[i].defines_subject;
            let mut j = i;
            while j < statements.len()
                && statements[j].subject == *subject
                && statements[j].class == *class
            {
                j += 1;
            }

            // The namespaces were bound before the root element was written, so a
            // statement about a vendor extension keeps its own namespace instead of being
            // relabelled `cim:` — which would have changed what the statement says.
            let (prefix, name) = match class {
                Some(q) => (
                    self.ns.prefix_of_iri(&q.ns).unwrap_or("ns").to_owned(),
                    q.local.clone(),
                ),
                None => ("rdf".to_owned(), "Description".to_owned()),
            };
            self.indent(3)?;
            // The same override as an object element: an identifier that is an absolute
            // IRI has no `rdf:ID` form, so the statement group names it with `rdf:about`.
            if defines && subject.form_in_xml() != crate::xml::IdentifierForm::Iri {
                write!(self.out, "<{prefix}:{name} rdf:ID=\"").map_err(Error::Io)?;
                self.escape_attr(&subject.to_rdf_id())?;
            } else {
                write!(self.out, "<{prefix}:{name} rdf:about=\"").map_err(Error::Io)?;
                self.escape_attr(&subject.to_rdf_reference())?;
            }
            self.out.write_all(b"\">\n")?;

            for s in &statements[i..j] {
                let p = self
                    .ns
                    .prefix_of_iri(&s.predicate_ns)
                    .unwrap_or("ns")
                    .to_owned();
                let predicate = &s.predicate;
                self.indent(4)?;
                match &s.value {
                    StatementValue::Resource(iri) => {
                        write!(self.out, "<{p}:{predicate} rdf:resource=\"").map_err(Error::Io)?;
                        self.escape_attr(iri)?;
                        self.out.write_all(b"\"/>\n")?;
                    }
                    StatementValue::Literal(text) => {
                        write!(self.out, "<{p}:{predicate}>").map_err(Error::Io)?;
                        self.escape_text(text)?;
                        writeln!(self.out, "</{p}:{predicate}>").map_err(Error::Io)?;
                    }
                }
            }

            self.indent(3)?;
            writeln!(self.out, "</{prefix}:{name}>").map_err(Error::Io)?;
            i = j;
        }

        self.indent(2)?;
        writeln!(self.out, "</{dm}:{local}>").map_err(Error::Io)?;
        Ok(())
    }

    fn header_text(&mut self, value: Option<&str>, local: &str) -> Result<()> {
        let Some(v) = value else { return Ok(()) };
        let md = self.md();
        self.header_prop_text(&md, local, v)
    }

    fn header_resource(&mut self, value: &str, local: &str) -> Result<()> {
        let md = self.md();
        self.header_prop_resource(&md, local, value)
    }

    fn header_prop_text(&mut self, prefix: &str, local: &str, value: &str) -> Result<()> {
        self.indent(2)?;
        write!(self.out, "<{prefix}:{local}>").map_err(Error::Io)?;
        self.escape_text(value)?;
        writeln!(self.out, "</{prefix}:{local}>").map_err(Error::Io)?;
        Ok(())
    }

    fn header_prop_resource(&mut self, prefix: &str, local: &str, value: &str) -> Result<()> {
        self.indent(2)?;
        write!(self.out, "<{prefix}:{local} rdf:resource=\"").map_err(Error::Io)?;
        self.escape_attr(value)?;
        self.out.write_all(b"\"/>\n")?;
        Ok(())
    }

    /// Write one object element.
    ///
    /// The prefix table is borrowed from `self.ns` while the sink is borrowed mutably, so
    /// the fields are destructured rather than reached through `self`. Doing it the other
    /// way meant copying each prefix into a fresh `String` — one allocation per attribute
    /// occurrence, over a million of them on a nation-scale model.
    fn object(&mut self, obj: &Object, options: &WriteOptions) -> Result<()> {
        let Writer {
            out,
            schema,
            pretty,
            ns,
            ..
        } = self;
        let pretty = *pretty;
        let all_attrs = schema.class(obj.class()).all_attrs;
        let class = schema.class(element_class(schema, obj, obj.class(), options.profiles));
        let prefix = ns.prefix(class.ns);

        indent(out, pretty, 1)?;
        write!(out, "<{}:{}", prefix, class.name).map_err(Error::Io)?;
        // An identifier that is an absolute IRI has no `rdf:ID` form — `rdf:ID` is an XML
        // `NCName` — so the choice the profile would make is overruled by what the syntax
        // permits. `rdf:about` with the IRI itself is both valid and exactly what the
        // document that supplied such an identifier said.
        let mrid = obj.mrid();
        match (
            resolve_id_style(options, class),
            mrid.form_in_xml() == crate::xml::IdentifierForm::Iri,
        ) {
            (_, true) | (IdStyle::RdfAbout, false) => {
                out.write_all(b" rdf:about=\"")?;
                escape_attr(out, &mrid.to_rdf_reference())?;
            }
            // `Auto` is resolved above; anything left is the explicit `rdf:ID` form.
            _ => {
                out.write_all(b" rdf:ID=\"")?;
                escape_attr(out, &mrid.to_rdf_id())?;
            }
        }
        // Asked of the object's own slots rather than of every attribute the class could
        // have: an object carries four values on average against twenty-one possible ones.
        if !object_has_content_in(schema, obj, options.profiles) {
            out.write_all(b"\"/>\n")?;
            return Ok(());
        }
        out.write_all(b"\">\n")?;

        // Attributes are emitted in schema order, and taken from the object's own class
        // rather than the element's: a Steady State Hypothesis file writes
        // `<cim:Equipment>` but the value it carries is still declared where the real
        // class declares it.
        for attr_id in all_attrs {
            let def = schema.attr(*attr_id);
            let attr_prefix = ns.prefix(def.ns);
            let prim = lexical_primitive(schema, def.kind);
            for slot in obj.get_all(*attr_id) {
                if !slot_belongs_to(schema, slot, options.profiles) {
                    continue;
                }
                property(
                    out,
                    schema,
                    ns,
                    pretty,
                    attr_prefix,
                    def.name,
                    &slot.value,
                    prim,
                    2,
                )?;
            }
        }

        indent(out, pretty, 1)?;
        writeln!(out, "</{}:{}>", prefix, class.name).map_err(Error::Io)?;
        Ok(())
    }

    fn indent(&mut self, level: usize) -> Result<()> {
        indent(&mut self.out, self.pretty, level)
    }

    fn escape_text(&mut self, s: &str) -> Result<()> {
        escape_text(&mut self.out, s)
    }

    fn escape_attr(&mut self, s: &str) -> Result<()> {
        escape_attr(&mut self.out, s)
    }
}

/// Write one property element, recursing into compounds.
///
/// `prim` is the primitive the schema gives the attribute, which decides the lexical
/// form: `Decimal` has no exponent notation, `Float` does.
#[allow(clippy::too_many_arguments)]
fn property<W: Write>(
    out: &mut W,
    schema: &'static Schema,
    ns: &Namespaces,
    pretty: bool,
    prefix: &str,
    name: &str,
    value: &Value,
    prim: Primitive,
    depth: usize,
) -> Result<()> {
    indent(out, pretty, depth)?;
    match value {
        Value::Reference(m) => {
            write!(out, "<{prefix}:{name} rdf:resource=\"").map_err(Error::Io)?;
            escape_attr(out, &m.to_rdf_reference())?;
            out.write_all(b"\"/>\n")?;
        }
        Value::Enum(e) => {
            let ev = schema.enum_value(*e);
            write!(out, "<{prefix}:{name} rdf:resource=\"").map_err(Error::Io)?;
            escape_attr(out, schema.namespace(ev.ns).iri)?;
            escape_attr(out, ev.name)?;
            out.write_all(b"\"/>\n")?;
        }
        // A compound has no identity, so it is written inline rather than referenced.
        // `rdf:parseType="Resource"` is what makes the nested element a blank node.
        Value::Compound(c) => {
            if c.is_empty() {
                writeln!(out, "<{prefix}:{name} rdf:parseType=\"Resource\"/>")
                    .map_err(Error::Io)?;
                return Ok(());
            }
            writeln!(out, "<{prefix}:{name} rdf:parseType=\"Resource\">").map_err(Error::Io)?;
            for (attr, v) in c.values() {
                let def = schema.attr(*attr);
                property(
                    out,
                    schema,
                    ns,
                    pretty,
                    ns.prefix(def.ns),
                    def.name,
                    v,
                    lexical_primitive(schema, def.kind),
                    depth + 1,
                )?;
            }
            indent(out, pretty, depth)?;
            writeln!(out, "</{prefix}:{name}>").map_err(Error::Io)?;
        }
        other => {
            write!(out, "<{prefix}:{name}>").map_err(Error::Io)?;
            if let Some(text) = other.to_lexical_as(prim) {
                escape_text(out, &text)?;
            }
            writeln!(out, "</{prefix}:{name}>").map_err(Error::Io)?;
        }
    }
    Ok(())
}

fn indent<W: Write>(out: &mut W, pretty: bool, level: usize) -> Result<()> {
    if pretty {
        for _ in 0..level {
            out.write_all(b"  ")?;
        }
    }
    Ok(())
}

/// Escape a value for element content.
///
/// Only `&` and `<` have to be escaped in character data; `>` is escaped as well because a
/// bare `]]>` is forbidden and escaping every `>` is the cheap way to be sure. Quotes are
/// left alone — a name containing an apostrophe is written as the producer wrote it rather
/// than as `&apos;`.
///
/// Carriage return is escaped numerically. It has to be: an XML parser normalizes literal
/// CR and CRLF in content to LF, so a value carrying one would come back different from
/// the value that was written.
fn escape_text<W: Write>(out: &mut W, s: &str) -> Result<()> {
    escape(out, s, |b| match b {
        b'&' => Some(b"&amp;".as_slice()),
        b'<' => Some(b"&lt;"),
        b'>' => Some(b"&gt;"),
        b'\r' => Some(b"&#13;"),
        _ => None,
    })
}

/// Escape a value for a double-quoted attribute.
///
/// Beyond the markup characters, every whitespace control character is escaped
/// numerically, because attribute-value normalization replaces a literal tab, newline or
/// carriage return with a space before an application ever sees it — silently changing the
/// value.
fn escape_attr<W: Write>(out: &mut W, s: &str) -> Result<()> {
    escape(out, s, |b| match b {
        b'&' => Some(b"&amp;".as_slice()),
        b'<' => Some(b"&lt;"),
        b'>' => Some(b"&gt;"),
        b'"' => Some(b"&quot;"),
        b'\t' => Some(b"&#9;"),
        b'\n' => Some(b"&#10;"),
        b'\r' => Some(b"&#13;"),
        _ => None,
    })
}

/// Copy `s`, replacing the bytes `rep` names, and dropping what XML cannot represent.
///
/// Multi-byte characters are untouched by the replacement table because every replaced
/// byte is ASCII and so never part of one.
///
/// The removal is the part that is not decoration. XML 1.0's `Char` production excludes
/// most of the C0 range *and* forbids a numeric character reference to it, so a value
/// carrying a NUL has no representation at all and a document containing one is refused by
/// every conforming parser at that byte, and `quick-xml` enforces neither end of this.
/// Dropping the character is the only alternative to writing an unreadable document; it is
/// not silent, because [`Rule::IllegalXmlCharacter`] fires where the value enters and again
/// for any value still holding one in the store.
///
/// [`Rule::IllegalXmlCharacter`]: crate::Rule::IllegalXmlCharacter
fn escape<W: Write>(out: &mut W, s: &str, rep: fn(u8) -> Option<&'static [u8]>) -> Result<()> {
    // One byte test per byte on the common path, and no allocation unless it fires.
    let cleaned = crate::xml::strip_illegal(s);
    let bytes = cleaned.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let Some(r) = rep(b) else { continue };
        out.write_all(&bytes[start..i])?;
        out.write_all(r)?;
        start = i + 1;
    }
    out.write_all(&bytes[start..])?;
    Ok(())
}

/// The primitive an attribute's values are written as.
///
/// A CIM datatype such as `Money` serializes as the primitive it wraps; anything with a
/// serialization of its own — an enumeration, a reference, a compound — never reaches the
/// text path, so `String` is a harmless stand-in.
fn lexical_primitive(schema: &Schema, kind: AttrKind) -> Primitive {
    match kind {
        AttrKind::Primitive(p) => p,
        AttrKind::Datatype(d) => schema.datatype(d).value,
        _ => Primitive::String,
    }
}

/// A prefix to suggest for a namespace the schema does not know, derived from its IRI.
///
/// `http://example.com/vendor#` yields `vendor`; anything unusable yields `ns`, which
/// [`Namespaces::bind`] then makes unique.
fn suggest_prefix(iri: &str) -> &str {
    let trimmed = iri.trim_end_matches(['#', '/']);
    let last = trimmed.rsplit(['#', '/', ':']).next().unwrap_or_default();
    let usable = !last.is_empty()
        && last.starts_with(|c: char| c.is_ascii_alphabetic())
        && last
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if usable { last } else { "ns" }
}

/// The class an object is written as in a file serving `profiles`.
///
/// An instance file may only name classes its own profiles declare, and may only name one
/// whose mandatory attributes it actually carries. A Steady State Hypothesis file
/// therefore writes `<cim:Equipment rdf:about="…">` for an `ACLineSegment` — SSH declares
/// `Equipment` but not `ACLineSegment` — and re-reading it merges the value back onto the
/// object the Equipment file defined. Emitting the most specific class instead produces a
/// document that fails the profile's own SHACL shapes.
///
/// Both conditions are load-bearing, and the published models show each. FullGrid's SSH
/// file writes 61 objects as `cim:Equipment` because SSH declares none of their classes,
/// and writes one `Cut` that way too: SSH declares `Cut`'s parent `Switch`, but `Switch`
/// requires `open`, which that object has no value for, so the generic carrier is the only
/// class the data supports.
///
/// The object's own class is kept when nothing better applies, rather than losing it.
pub fn element_class(
    schema: &Schema,
    obj: &Object,
    class: ClassId,
    profiles: ProfileMask,
) -> ClassId {
    // Writing everything into one document imposes no profile, so the real class stands.
    if profiles == 0 {
        return class;
    }
    let declared = |c: ClassId| {
        let d = schema.class(c);
        d.concrete && !d.compound && d.profiles & profiles != 0
    };
    // Every mandatory attribute this profile expects of the class must be supplied by the
    // *model*, or the document would claim more than the data supports.
    //
    // The model, not this file — `obj.has(a)` rather than "is `a` written here". The
    // stricter reading looks more correct and is not: `IdentifiedObject.mRID` is mandatory
    // in ten of eleven CGMES 3.0 profiles, and the published Steady State Hypothesis files
    // do not carry it (which is the one finding ENTSO-E's own shapes report against their
    // own models). Under the stricter rule no class is ever complete for SSH, every
    // candidate fails, and the fallback writes the *most specific* class — the exact
    // opposite of what this function exists to do, and what a corpus census catches.
    let complete = |c: ClassId| {
        schema.class(c).all_attrs.iter().all(|&a| {
            let def = schema.attr(a);
            !(def.mult.is_required() && def.used_in & profiles != 0) || obj.has(a)
        })
    };
    std::iter::once(class)
        .chain(schema.class(class).ancestors.iter().copied())
        .find(|&c| declared(c) && complete(c))
        .unwrap_or(class)
}

/// The header a document gets when the caller supplies none.
///
/// Public so a caller can derive one, adjust it — a modelling authority set, a scenario
/// time — and pass it back through [`WriteOptions::with_header`].
///
/// Declares exactly the profiles being written — the mask when one is given, otherwise
/// every profile the data actually covers — because `md:Model.profile` is how a consumer
/// decides what a file is, and claiming a profile the file says nothing about is worse
/// than claiming none.
///
/// The identifier is an RFC 4122 v5 UUID over the vintage, those profiles and the model's
/// content identifier: deterministic, distinct per model, and needing no clock and no
/// dependency. `Model.created` is deliberately absent rather than invented — this crate
/// reads no clock, and a timestamp that is not when the model was made is worse than an
/// absent one.
pub fn derive_header(dataset: &Dataset, profiles: ProfileMask) -> ModelHeader {
    derive_header_with(
        dataset.schema(),
        if profiles == 0 {
            dataset.profiles()
        } else {
            profiles
        },
        &dataset.content_id(),
    )
}

/// The same, from an already-computed content identifier.
///
/// Split out because `Dataset::content_id` walks the whole model — 88 ms on the 112 MiB
/// `RealGrid` — and an export that writes eleven profile files would otherwise walk it
/// eleven times to derive eleven headers that all name the same model.
pub(crate) fn derive_header_with(
    schema: &'static Schema,
    covered: ProfileMask,
    content: &Mrid,
) -> ModelHeader {
    let iris: Vec<String> = schema
        .profiles
        .iter()
        .enumerate()
        .filter(|(i, _)| covered & (1u64 << i) != 0)
        .map(|(_, p)| p.version_iri.to_owned())
        .collect();

    let mut name = format!("{}\u{1e}", schema.vintage);
    for iri in &iris {
        name.push_str(iri);
        name.push('\u{1e}');
    }
    name.push_str(&content.to_string());

    ModelHeader {
        kind: ModelKind::Full,
        id: Some(Mrid::new_v5(&Dataset::DERIVED_NS, name.as_bytes())),
        profiles: iris,
        version: Some("1".to_owned()),
        ..Default::default()
    }
}

/// How an object of `class` is identified in a file serving `options.profiles`.
///
/// Exposed so tooling can predict the output without rendering it.
pub fn id_style_for(schema: &Schema, class: ClassId, profiles: ProfileMask) -> IdStyle {
    resolve_id_style(
        &WriteOptions {
            profiles,
            ..Default::default()
        },
        schema.class(class),
    )
}

fn resolve_id_style(options: &WriteOptions, class: &ClassDef) -> IdStyle {
    match options.id_style {
        IdStyle::Auto => {
            // Writing everything into one document makes that document the definition.
            if options.profiles == 0 || class.defined_in & options.profiles != 0 {
                IdStyle::RdfId
            } else {
                IdStyle::RdfAbout
            }
        }
        forced => forced,
    }
}

/// Classes that a profile may contain, for callers building per-profile exports.
pub fn classes_in_profile(
    schema: &Schema,
    profile: ProfileId,
) -> impl Iterator<Item = ClassId> + '_ {
    let mask = profile.mask();
    schema
        .classes
        .iter()
        .enumerate()
        .filter(move |(_, c)| c.concrete && !c.compound && c.profiles & mask != 0)
        .map(|(i, _)| ClassId(i as u16))
}
