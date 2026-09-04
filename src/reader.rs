//! Streaming reader for CIM/XML instance documents (IEC 61970-552).
//!
//! IEC 61970-552 constrains RDF/XML to a small, regular subset: a flat `rdf:RDF` root
//! containing a header and a list of objects, where each object is a class element whose
//! property children are either element text or a single `rdf:resource`. A purpose-built
//! pull parser over that subset is both much faster and much simpler than a general RDF
//! toolchain, and it never holds the document in memory.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::dataset::Dataset;
use crate::error::{Diagnostic, Error, Report, Result, Rule};
use crate::header::{
    DM_NS, DifferenceModel, HeaderProperty, MD_NS, ModelHeader, ModelKind, QualifiedName,
    Statement, StatementValue,
};
use crate::mrid::Mrid;
use crate::object::{Compound, Object};
use crate::schema::{AttrId, AttrKind, ClassId, Primitive, ProfileId, ProfileMask, Schema};
use crate::value::Value;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Stand-in identifier for a diagnostic raised where no object is under construction.
static UNKNOWN_MRID: Mrid = Mrid::nil();

/// How strictly to treat schema deviations while reading.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Strictness {
    /// Unknown classes and attributes are recorded as diagnostics and skipped.
    ///
    /// The default: published models regularly carry vendor extensions, and refusing to
    /// read them would make the library useless for real data.
    #[default]
    Lenient,
    /// Unknown classes or attributes make the read fail.
    Strict,
}

/// Options controlling a read.
#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub strictness: Strictness,
    /// Profile to attribute the data to when the header does not declare one.
    pub assume_profile: Option<ProfileId>,
    /// Record `Info` diagnostics for deprecated classes and attributes.
    pub report_deprecated: bool,
    /// Record a diagnostic for every identifier that is not a UUID.
    pub report_non_conforming_mrids: bool,
    /// Stop recording after this many diagnostics.
    ///
    /// Lenient reading answers a malformed document with a diagnostic rather than an
    /// error, which means the *report* is proportional to how broken the input is: a file
    /// whose every element is unknown produces one finding per element, each carrying
    /// several owned strings. On a hostile or merely corrupt hundred-megabyte document
    /// that is memory the caller never asked for. Past the cap the reader keeps reading
    /// and stops recording, closing the report with a note saying so, so nothing is
    /// silently truncated. Zero means no limit.
    pub max_diagnostics: usize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            strictness: Strictness::Lenient,
            assume_profile: None,
            report_deprecated: false,
            report_non_conforming_mrids: false,
            max_diagnostics: ReadOptions::DEFAULT_MAX_DIAGNOSTICS,
        }
    }
}

impl ReadOptions {
    /// How many diagnostics one read records before it stops recording.
    pub const DEFAULT_MAX_DIAGNOSTICS: usize = 10_000;

    /// Tolerant defaults, suitable for real-world exchange files.
    pub fn lenient() -> ReadOptions {
        ReadOptions::default()
    }

    /// Fail on anything the schema does not describe, and report every deviation.
    pub fn strict() -> ReadOptions {
        ReadOptions {
            strictness: Strictness::Strict,
            report_deprecated: true,
            report_non_conforming_mrids: true,
            ..ReadOptions::default()
        }
    }

    pub fn with_profile(mut self, profile: ProfileId) -> Self {
        self.assume_profile = Some(profile);
        self
    }

    /// Cap the number of diagnostics one read records. Zero means no limit.
    pub fn with_max_diagnostics(mut self, max: usize) -> Self {
        self.max_diagnostics = max;
        self
    }
}

/// Result of reading one document.
#[derive(Debug)]
pub struct ReadOutcome {
    pub header: Option<ModelHeader>,
    /// Present when the document was a `dm:DifferenceModel`.
    pub difference: Option<DifferenceModel>,
    pub report: Report,
    /// Number of object elements read from this document.
    pub objects_read: usize,
}

/// Read a CIM/XML document into `dataset`, merging with what is already loaded.
pub fn read_into<R: BufRead>(
    dataset: &mut Dataset,
    input: R,
    source: Option<&str>,
    options: &ReadOptions,
) -> Result<ReadOutcome> {
    let schema = dataset.schema();
    let mut parser = Parser::new(schema, source, options, false);
    parser.run(input, Some(dataset))
}

/// Read only the header of a document, without storing objects.
///
/// Useful for planning a multi-file load: headers declare profiles and dependencies.
pub fn read_header<R: BufRead>(
    schema: &'static Schema,
    input: R,
    source: Option<&str>,
) -> Result<Option<ModelHeader>> {
    let options = ReadOptions::default();
    let mut parser = Parser::new(schema, source, &options, true);
    Ok(parser.run(input, None)?.header)
}

/// Identify which schema vintage a document is written against, reading only its root.
///
/// CIM/XML names its vocabulary in the document, so the vintage is a fact to be read rather
/// than a parameter to be guessed. Returns `None` when no compiled-in vintage recognises the
/// namespaces — a document this build has no feature for, or not a CIM/XML document at all.
///
/// ```no_run
/// # fn main() -> cim_rs::Result<()> {
/// let file = std::io::BufReader::new(std::fs::File::open("EQ.xml")?);
/// let schema = cim_rs::reader::sniff(file)?.expect("a known CIM vintage");
/// let mut ds = cim_rs::Dataset::new(schema);
/// # Ok(()) }
/// ```
pub fn sniff<R: BufRead>(input: R) -> Result<Option<&'static Schema>> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().check_end_names = false;
    let mut buf = Vec::new();
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::Xml(e.to_string()))?;
        let e = match ev {
            Event::Eof => return Ok(None),
            Event::Start(e) | Event::Empty(e) => e,
            _ => {
                buf.clear();
                continue;
            }
        };
        // Producers occasionally wrap `rdf:RDF`, so keep looking rather than giving up on
        // the first element — but only until something declares a namespace we know.
        let mut iris: Vec<String> = Vec::new();
        for attr in e.attributes().with_checks(false) {
            let attr = attr?;
            let key = attr.key.as_ref();
            if key == b"xmlns" || key.starts_with(b"xmlns:") {
                iris.push(String::from_utf8_lossy(&attr.value).into_owned());
            }
        }
        if let Some(schema) = Schema::detect(iris.iter().map(String::as_str)) {
            return Ok(Some(schema));
        }
        buf.clear();
    }
}

/// Read a difference model document.
pub fn read_difference<R: BufRead>(
    schema: &'static Schema,
    input: R,
    source: Option<&str>,
) -> Result<Option<DifferenceModel>> {
    let options = ReadOptions::default();
    let mut parser = Parser::new(schema, source, &options, false);
    Ok(parser.run(input, None)?.difference)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// What an element name refers to, once resolved against the schema.
#[derive(Clone, Copy)]
enum Resolved {
    Class(ClassId),
    Attr(AttrId),
    /// A name the schema does not define — typically a vendor extension.
    Unknown,
}

/// Where the parser currently is in the document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    /// Outside `rdf:RDF`.
    Outside,
    /// Directly inside `rdf:RDF`.
    Body,
    /// Inside `md:FullModel` or `dm:DifferenceModel`.
    Header,
    /// Inside an object element.
    Object,
    /// Inside a compound value — an `rdf:parseType="Resource"` element.
    Compound,
    /// Inside `dm:forwardDifferences` / `dm:reverseDifferences`.
    DiffSet,
    /// Inside an `rdf:Description` within a difference set.
    DiffSubject,
    /// Inside an element whose text content is being accumulated.
    ///
    /// Distinct from [`Ctx::Skip`] so that a value is exactly the text of *its own*
    /// element. Without the distinction, markup nested inside a property element — which
    /// IEC 61970-552 does not allow, and which malformed documents nonetheless contain —
    /// donated its text to the property and ended it at the nested element's close tag,
    /// so `<name><x>ignored</x></name>` read back as `ignored` rather than as empty.
    Text,
    /// Inside an element that is being skipped wholesale.
    Skip,
}

/// What a resolved property element asks the parser to do.
enum Prop {
    /// Store this value immediately: it came from `rdf:resource` or an empty element.
    Value(AttrId, Value),
    /// Descend into a compound value of the named type.
    Enter(AttrId, ClassId),
    /// Accumulate the element's text for this attribute.
    Text(AttrId),
    /// Nothing to store.
    Skip,
}

/// Everything being assembled while the document is walked.
///
/// Kept in one place rather than threaded through the element handlers as a dozen
/// `&mut` parameters.
#[derive(Default)]
struct State {
    saw_root: bool,
    header: Option<ModelHeader>,
    difference: Option<DifferenceModel>,
    objects_read: usize,
    /// Object under construction.
    object: Option<Object>,
    /// Compounds under construction, innermost last. Compounds nest: a `StreetAddress`
    /// holds a `StreetDetail`.
    compounds: Vec<(AttrId, Compound)>,
    /// Property whose text is being accumulated.
    pending: Option<PendingText>,
    /// Profiles to attribute values to, taken from the header once it closes.
    profile_mask: ProfileMask,
    /// Subject of the statement group being read: identifier, class and whether the
    /// group introduced it with `rdf:ID`.
    diff_subject: Option<(Mrid, Option<QualifiedName>, bool)>,
    diff_forward: bool,
    /// This file's slot among the dataset's headers, so an export can put each object
    /// back in the file it came from.
    source: Option<usize>,
    /// Whether the header has been registered with the dataset yet.
    header_registered: bool,
    /// Slot claimed for a document that had not named itself yet, to be filled in if it
    /// turns out to have an `md:FullModel` after all.
    placeholder_slot: Option<usize>,
}

struct Parser<'a> {
    schema: &'static Schema,
    options: &'a ReadOptions,
    source: Option<String>,
    report: Report,
    header_only: bool,
    /// Whether the report has already been closed off at `max_diagnostics`.
    truncated: bool,
    /// Prefix bytes -> namespace IRI, as declared in the document.
    ns: HashMap<Vec<u8>, String>,
    /// Raw element QName -> resolution. Instance documents repeat a small set of element
    /// names many thousands of times, so this cache removes nearly all lookup cost.
    cache: HashMap<Vec<u8>, Resolved>,
}

impl<'a> Parser<'a> {
    fn new(
        schema: &'static Schema,
        source: Option<&str>,
        options: &'a ReadOptions,
        header_only: bool,
    ) -> Parser<'a> {
        Parser {
            schema,
            options,
            source: source.map(str::to_owned),
            report: Report::default(),
            header_only,
            truncated: false,
            ns: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    fn diag(&mut self, d: Diagnostic, pos: u64) {
        let max = self.options.max_diagnostics;
        if max != 0 && self.report.len() >= max {
            if !self.truncated {
                self.truncated = true;
                self.report.push(Diagnostic::warning(
                    Rule::Structure,
                    format!(
                        "stopped recording after {max} diagnostics; reading continued \
                         (raise ReadOptions::max_diagnostics to see the rest)"
                    ),
                ));
            }
            return;
        }
        let mut d = d.with_position(pos);
        if let Some(s) = &self.source {
            d = d.with_source(s.clone());
        }
        self.report.push(d);
    }

    /// Expand an element QName to `(namespace IRI, local name)`, borrowing both.
    fn expand<'q>(&self, qname: &'q [u8]) -> Option<(&str, &'q str)> {
        let (prefix, local) = match qname.iter().position(|&b| b == b':') {
            Some(i) => (&qname[..i], &qname[i + 1..]),
            None => (&b""[..], qname),
        };
        let ns = self.ns.get(prefix)?;
        let local = std::str::from_utf8(local).ok()?;
        Some((ns.as_str(), local))
    }

    fn resolve(&mut self, qname: &[u8]) -> Resolved {
        if let Some(r) = self.cache.get(qname) {
            return *r;
        }
        let resolved = match self.expand(qname) {
            // A dot separates class from property: `cim:ACLineSegment.r`.
            Some((ns, local)) if local.contains('.') => match self.schema.find_attr(ns, local) {
                Some(a) => Resolved::Attr(a),
                None => Resolved::Unknown,
            },
            Some((ns, local)) => match self.schema.find_class(ns, local) {
                Some(c) => Resolved::Class(c),
                None => Resolved::Unknown,
            },
            None => Resolved::Unknown,
        };
        self.cache.insert(qname.to_vec(), resolved);
        resolved
    }

    fn run<R: BufRead>(
        &mut self,
        input: R,
        mut dataset: Option<&mut Dataset>,
    ) -> Result<ReadOutcome> {
        let mut reader = Reader::from_reader(input);
        reader.config_mut().trim_text(false);
        reader.config_mut().expand_empty_elements = false;
        reader.config_mut().check_end_names = false;

        let mut buf = Vec::new();
        let mut ctx = Ctx::Outside;
        let mut stack: Vec<Ctx> = Vec::with_capacity(8);

        let mut st = State {
            profile_mask: self.options.assume_profile.map_or(0, |p| p.mask()),
            ..Default::default()
        };

        loop {
            let ev = reader
                .read_event_into(&mut buf)
                .map_err(|e| Error::Xml(e.to_string()))?;
            let pos = reader.buffer_position();

            match ev {
                Event::Eof => break,

                Event::Start(e) => {
                    let next = self.on_open(&e, false, ctx, &mut st, pos)?;
                    stack.push(ctx);
                    ctx = next;
                }

                Event::Empty(e) => {
                    // Self-closing element: open and close in one step, so the context
                    // stack is untouched.
                    let saved = ctx;
                    let entered = self.on_open(&e, true, ctx, &mut st, pos)?;
                    match entered {
                        // An object with no properties is written `<cim:Class rdf:ID="…"/>`.
                        // It closes immediately, so commit it here — otherwise it would be
                        // lost and would overwrite whichever object came next.
                        Ctx::Object => {
                            if let Some(o) = st.object.take() {
                                self.ensure_source(&mut st, &mut dataset);
                                self.commit(o, &st, &mut dataset, pos);
                                st.objects_read += 1;
                            }
                        }
                        // An empty compound: `<cim:X.y rdf:parseType="Resource"/>`.
                        Ctx::Compound => self.close_compound(&mut st),
                        Ctx::DiffSubject => st.diff_subject = None,
                        // A header with no properties: `<md:FullModel rdf:about="…"/>`.
                        // Legal, and degenerate, but it still identifies the file — and
                        // registering it is what gives this file's objects a source slot,
                        // so an export can put them back in the file they came from.
                        Ctx::Header => self.register_header(&mut st, &mut dataset),
                        _ => {}
                    }
                    ctx = saved;
                }

                // Text belongs to the element that is accumulating it and to no other, so
                // it is taken only in `Ctx::Text`. Anything nested inside a property
                // element is skipped along with its content.
                Event::Text(t) if ctx == Ctx::Text => {
                    if let Some(p) = st.pending.as_mut() {
                        // XML 1.0 content: decoded, and with CR and CRLF normalized to LF
                        // as the specification requires of every conforming parser. A
                        // value that must keep a literal CR is written as `&#13;`, which
                        // arrives as the reference event below rather than as text.
                        let text = t.xml10_content().map_err(|e| Error::Xml(e.to_string()))?;
                        p.text.push_str(&text);
                    }
                }

                // `&amp;`, `&lt;`, `&#13;`: a reference is its own event, delivered between
                // the text runs around it. Ignoring it does not fail — it silently deletes
                // the character, so `A &amp; B` reads back as `A  B`. The round-trip test
                // over XML special characters is what says otherwise.
                Event::GeneralRef(r) if ctx == Ctx::Text => {
                    if let Some(p) = st.pending.as_mut() {
                        match r.resolve_char_ref() {
                            // A numeric character reference: `&#13;` or `&#x0D;`.
                            Ok(Some(c)) => p.text.push(c),
                            // A named entity. Only the five XML predefines exist here:
                            // IEC 61970-552 documents declare no DTD, so anything else is
                            // undefined and keeping its literal form loses least.
                            Ok(None) | Err(_) => {
                                let name = r.decode().map_err(|e| Error::Xml(e.to_string()))?;
                                match quick_xml::escape::resolve_predefined_entity(&name) {
                                    Some(text) => p.text.push_str(text),
                                    None => {
                                        p.text.push('&');
                                        p.text.push_str(&name);
                                        p.text.push(';');
                                    }
                                }
                            }
                        }
                    }
                }

                Event::CData(t) if ctx == Ctx::Text => {
                    if let Some(p) = st.pending.as_mut() {
                        p.text.push_str(&String::from_utf8_lossy(&t));
                    }
                }

                Event::End(_) => {
                    // The header precedes every object, so once it closes the profiles
                    // the file declares are known, the header can be registered with the
                    // dataset, and objects can be attributed to both.
                    if ctx == Ctx::Header {
                        self.register_header(&mut st, &mut dataset);
                    }
                    match ctx {
                        // A value ends with its own element, never with a nested one.
                        Ctx::Text => {
                            if let Some(p) = st.pending.take() {
                                self.finish_text(p, &mut st, pos);
                            }
                        }
                        Ctx::Object => {
                            if let Some(o) = st.object.take() {
                                self.ensure_source(&mut st, &mut dataset);
                                self.commit(o, &st, &mut dataset, pos);
                                st.objects_read += 1;
                            }
                        }
                        Ctx::Compound => self.close_compound(&mut st),
                        Ctx::DiffSubject => st.diff_subject = None,
                        _ => {}
                    }
                    ctx = stack.pop().unwrap_or(Ctx::Outside);
                }

                _ => {}
            }
            buf.clear();

            // Reading only the header: stop as soon as it is complete.
            if self.header_only && st.header.is_some() && ctx == Ctx::Body {
                break;
            }
        }

        if !st.saw_root {
            return Err(Error::NotCimXml("no rdf:RDF root element found".to_owned()));
        }

        // A file without a header still occupies a slot, so that the slot a later file
        // gets matches its position in `Dataset::headers`. `ensure_source` has normally
        // claimed it already; this covers a document that declared no objects either.
        if !st.header_registered
            && let Some(ds) = dataset.as_mut()
        {
            ds.push_header(self.placeholder_header());
        }

        let mut header = st.header;
        let mut difference = st.difference;
        if let Some(d) = difference.as_mut()
            && let Some(h) = header.clone()
        {
            d.header = h;
        }
        if let Some(h) = header.as_mut()
            && h.source.is_none()
        {
            h.source = self.source.clone();
        }
        // A difference is not applied here — `Dataset::apply_difference` does that — but
        // keeping it lets a model set containing change files be written back whole.
        if let (Some(d), Some(ds)) = (difference.as_ref(), dataset.as_mut()) {
            ds.push_difference(d.clone());
        }

        Ok(ReadOutcome {
            header,
            difference,
            report: std::mem::take(&mut self.report),
            objects_read: st.objects_read,
        })
    }

    /// Report, once, that this document belongs to a different schema vintage.
    ///
    /// The namespaces are in the document, so a mismatch is knowable at the root — where it
    /// is one error, rather than one "unknown class" warning per element and a model with
    /// nothing in it.
    fn check_vintage(&mut self, pos: u64) {
        let declared: Vec<&str> = self.ns.values().map(String::as_str).collect();
        // Comparative, not absolute: the header vocabulary is shared, so every vintage
        // scores something on every CIM/XML document. What matters is whether another one
        // scores higher.
        let Some(actual) = Schema::detect(declared.iter().copied()) else {
            return;
        };
        if std::ptr::eq(actual, self.schema) {
            return;
        }
        self.diag(
            Diagnostic::error(
                Rule::WrongVintage,
                format!(
                    "this document is {} and is being read as {}; nothing in it will \
                     resolve. Use `{}::SCHEMA`, or `reader::sniff` to detect it",
                    actual.vintage, self.schema.vintage, actual.vintage
                ),
            ),
            pos,
        );
    }

    /// The header slot a document with no `md:FullModel` still gets.
    ///
    /// Named after the document it came from, and declaring the profile the caller assumed
    /// where there is one: a file the caller has identified *is* a file of that profile,
    /// and every export path asks the header which profiles it serves. Without this the
    /// answer was "none", which made a headerless model export as an empty directory.
    fn placeholder_header(&self) -> ModelHeader {
        let mut h = ModelHeader::new(ModelKind::Full);
        h.source = self.source.clone();
        if let Some(p) = self.options.assume_profile {
            h.profiles
                .push(self.schema.profile(p).version_iri.to_owned());
        }
        h
    }

    /// Make sure this document owns a header slot before its first object is stored.
    ///
    /// Objects record the file they came from as they are committed, so the slot has to
    /// exist by then: a document with no `md:FullModel` would otherwise leave its objects
    /// belonging to no file, and nothing to export them back into.
    fn ensure_source(&mut self, st: &mut State, dataset: &mut Option<&mut Dataset>) {
        if st.header_registered || st.source.is_some() {
            return;
        }
        st.header_registered = true;
        if let Some(ds) = dataset.as_mut() {
            let slot = ds.push_header(self.placeholder_header());
            st.placeholder_slot = Some(slot);
            st.source = Some(slot);
        }
    }

    /// Register the just-parsed header with the dataset and adopt what it declares.
    fn register_header(&mut self, st: &mut State, dataset: &mut Option<&mut Dataset>) {
        let Some(h) = st.header.as_ref() else { return };
        let declared = h
            .profiles
            .iter()
            .filter_map(|iri| self.schema.profile_by_iri(iri))
            .fold(0, |acc, p| acc | p.mask());
        if declared != 0 {
            st.profile_mask = declared;
        }
        // A document that names itself after its first object already has a slot; fill it
        // in rather than describing the same file twice.
        if let Some(slot) = st.placeholder_slot.take()
            && let Some(ds) = dataset.as_mut()
        {
            let mut h = h.clone();
            if h.source.is_none() {
                h.source = self.source.clone();
            }
            ds.set_header(slot, h);
            return;
        }
        if st.header_registered {
            return;
        }
        st.header_registered = true;
        if let Some(ds) = dataset.as_mut() {
            let mut h = h.clone();
            if h.source.is_none() {
                h.source = self.source.clone();
            }
            // A caller who says which profile an undeclared file holds has said what the
            // file *is*, so the header says so too. Without this the values are attributed
            // and the file is still not reproducible as itself: every export path asks the
            // header which profiles it serves, and this one would answer "none".
            if h.profiles.is_empty()
                && let Some(p) = self.options.assume_profile
            {
                h.profiles
                    .push(self.schema.profile(p).version_iri.to_owned());
            }
            st.source = Some(ds.push_header(h));
        }
    }

    /// Finish the innermost compound and attach it to whatever encloses it.
    fn close_compound(&mut self, st: &mut State) {
        let Some((attr, compound)) = st.compounds.pop() else {
            return;
        };
        let value = Value::Compound(Box::new(compound));
        let many = self.schema.attr(attr).mult.is_many();
        match st.compounds.last_mut() {
            Some((_, parent)) => {
                if many {
                    parent.push(attr, value)
                } else {
                    parent.set(attr, value)
                }
            }
            None => {
                if let Some(obj) = st.object.as_mut() {
                    if many {
                        obj.push_in(st.profile_mask, attr, value);
                    } else {
                        obj.set_in(st.profile_mask, attr, value);
                    }
                }
            }
        }
    }

    /// Store a finished object, reporting an identifier collision that is not a merge.
    ///
    /// Two files describing the same object is normal and is how profiles compose. Two
    /// files giving the same identifier to *unrelated* classes is a defect, because one
    /// of them will be silently absorbed into the other.
    fn commit(
        &mut self,
        mut object: Object,
        st: &State,
        dataset: &mut Option<&mut Dataset>,
        pos: u64,
    ) {
        object.mark_profile(st.profile_mask);
        object.from_file = true;
        object.shrink_for_load();
        let Some(ds) = dataset.as_mut() else { return };

        if let Some(existing) = ds.by_mrid(object.mrid()) {
            let incoming = object.class();
            let held = existing.class();
            if !self.schema.is_a(incoming, held) && !self.schema.is_a(held, incoming) {
                let d = Diagnostic::error(
                    Rule::DuplicateMrid,
                    format!(
                        "identifier {} is used by both {} and {}, which are unrelated classes",
                        object.mrid().canonical(),
                        self.schema.class(held).name,
                        self.schema.class(incoming).name
                    ),
                )
                .with_class(self.schema.class(incoming).name)
                .with_object(object.mrid().clone());
                self.diag(d, pos);
            }
        }
        // Re-borrow: `by_mrid` above held an immutable borrow.
        if let Some(ds) = dataset.as_mut() {
            let id = ds.insert(object);
            if let Some(source) = st.source {
                ds.record_source(source, id);
            }
        }
    }

    /// Handle an opening (or self-closing) element. Returns the context to enter.
    fn on_open(
        &mut self,
        e: &BytesStart<'_>,
        self_closing: bool,
        ctx: Ctx,
        st: &mut State,
        pos: u64,
    ) -> Result<Ctx> {
        let qname = e.name();
        let qname = qname.as_ref();

        // The root element carries every namespace binding used in the document — but XML
        // permits them anywhere, so a declaration further down is picked up too rather
        // than leaving everything below it unresolvable. Scanning the element's own bytes
        // for `xmlns` first keeps that off the hot path: instance documents declare
        // nothing after the root.
        if !st.saw_root || declares_namespace(e) {
            self.collect_namespaces(e)?;
        }
        if !st.saw_root {
            if let Some((ns, local)) = self.expand(qname)
                && ns == RDF_NS
                && local == "RDF"
            {
                st.saw_root = true;
                self.check_vintage(pos);
                return Ok(Ctx::Body);
            }
            // Some producers put the namespaces on a wrapper; keep looking.
            return Ok(ctx);
        }

        match ctx {
            Ctx::Body => self.open_in_body(e, qname, st, pos),
            Ctx::Object => {
                let Some(obj) = st.object.as_ref() else {
                    return Ok(Ctx::Skip);
                };
                // Borrowed, not cloned: this runs once per property element, and the
                // identifier is only needed to label a diagnostic.
                let (class, mrid) = (obj.class(), obj.mrid());
                match self.property(e, self_closing, class, mrid, pos)? {
                    Prop::Value(attr, value) => {
                        let obj = st.object.as_mut().expect("object present");
                        if self.schema.attr(attr).mult.is_many() {
                            obj.push_in(st.profile_mask, attr, value);
                        } else {
                            obj.set_in(st.profile_mask, attr, value);
                        }
                        Ok(Ctx::Skip)
                    }
                    Prop::Enter(attr, class) => {
                        st.compounds.push((attr, Compound::new(class)));
                        Ok(Ctx::Compound)
                    }
                    Prop::Text(attr) => {
                        st.pending = Some(PendingText {
                            target: TextTarget::Attr(attr),
                            text: String::new(),
                        });
                        Ok(Ctx::Text)
                    }
                    Prop::Skip => Ok(Ctx::Skip),
                }
            }
            Ctx::Compound => {
                let Some((_, top)) = st.compounds.last() else {
                    return Ok(Ctx::Skip);
                };
                let class = top.class();
                let mrid = st.object.as_ref().map_or(&UNKNOWN_MRID, |o| o.mrid());
                match self.property(e, self_closing, class, mrid, pos)? {
                    Prop::Value(attr, value) => {
                        let (_, top) = st.compounds.last_mut().expect("compound present");
                        if self.schema.attr(attr).mult.is_many() {
                            top.push(attr, value);
                        } else {
                            top.set(attr, value);
                        }
                        Ok(Ctx::Skip)
                    }
                    Prop::Enter(attr, class) => {
                        st.compounds.push((attr, Compound::new(class)));
                        Ok(Ctx::Compound)
                    }
                    Prop::Text(attr) => {
                        st.pending = Some(PendingText {
                            target: TextTarget::Compound(attr),
                            text: String::new(),
                        });
                        Ok(Ctx::Text)
                    }
                    Prop::Skip => Ok(Ctx::Skip),
                }
            }
            Ctx::Header => self.open_in_header(e, qname, self_closing, st),
            Ctx::DiffSet => {
                // Each statement group names its subject, either with a bare
                // `rdf:Description` or with the class element itself. The latter carries
                // real information: a difference may reclassify an object.
                let about = self.attr(e, RDF_NS, "about")?;
                let defines = about.is_none();
                if let Some(raw) = about.or(self.attr(e, RDF_NS, "ID")?) {
                    let class = self.expand(qname).and_then(|(ns, local)| {
                        (!(ns == RDF_NS && local == "Description")).then(|| QualifiedName {
                            ns: ns.to_owned(),
                            local: local.to_owned(),
                        })
                    });
                    st.diff_subject = Some((Mrid::parse(raw.trim()), class, defines));
                    return Ok(Ctx::DiffSubject);
                }
                Ok(Ctx::Skip)
            }
            Ctx::DiffSubject => {
                let Some((ns, local)) = self
                    .expand(qname)
                    .map(|(n, l)| (n.to_owned(), l.to_owned()))
                else {
                    return Ok(Ctx::Skip);
                };
                let Some((subject, class, defines_subject)) = st.diff_subject.clone() else {
                    return Ok(Ctx::Skip);
                };
                if let Some(res) = self.attr(e, RDF_NS, "resource")? {
                    let res = res.trim().to_owned();
                    push_statement(
                        &mut st.difference,
                        st.diff_forward,
                        Statement {
                            subject,
                            class,
                            defines_subject,
                            predicate_ns: ns,
                            predicate: local,
                            value: StatementValue::Resource(res),
                        },
                    );
                    return Ok(Ctx::Skip);
                }
                if self_closing {
                    return Ok(Ctx::Skip);
                }
                st.pending = Some(PendingText {
                    target: TextTarget::Statement {
                        subject,
                        class,
                        defines_subject,
                        ns,
                        local,
                        forward: st.diff_forward,
                    },
                    text: String::new(),
                });
                Ok(Ctx::Text)
            }
            // Markup inside a value. IEC 61970-552 property elements hold text, a single
            // `rdf:resource`, or an inline compound — never mixed content — so this is a
            // defect in the document. The nested element is skipped with its content
            // rather than silently donating its text to the value around it.
            Ctx::Text => {
                let name = String::from_utf8_lossy(qname).into_owned();
                self.diag(
                    Diagnostic::warning(
                        Rule::Structure,
                        format!("unexpected element <{name}> inside a value; skipped"),
                    ),
                    pos,
                );
                Ok(Ctx::Skip)
            }
            Ctx::Skip | Ctx::Outside => Ok(Ctx::Skip),
        }
    }

    /// An element directly inside `rdf:RDF`: a header or an object.
    fn open_in_body(
        &mut self,
        e: &BytesStart<'_>,
        qname: &[u8],
        st: &mut State,
        pos: u64,
    ) -> Result<Ctx> {
        if let Some((ns, local)) = self.expand(qname)
            && ((ns == MD_NS && local == "FullModel")
                || (ns == DM_NS && local == "DifferenceModel"))
        {
            let kind = if ns == DM_NS {
                ModelKind::Difference
            } else {
                ModelKind::Full
            };
            let mut h = ModelHeader::new(kind);
            h.id = self
                .attr(e, RDF_NS, "about")?
                .map(|s| Mrid::parse(s.trim()));
            h.source = self.source.clone();
            st.header = Some(h);
            if kind == ModelKind::Difference {
                st.difference = Some(DifferenceModel::default());
            }
            return Ok(Ctx::Header);
        }

        if self.header_only {
            return Ok(Ctx::Skip);
        }

        match self.resolve(qname) {
            Resolved::Class(class) => {
                let def = self.schema.class(class);
                // A compound has no identity, so it cannot stand on its own; it only ever
                // appears inside the property that holds it.
                if def.compound {
                    self.diag(
                        Diagnostic::error(
                            Rule::Structure,
                            format!(
                                "compound type {} has no identity and cannot be a top-level object",
                                def.name
                            ),
                        )
                        .with_class(def.name),
                        pos,
                    );
                    return Ok(Ctx::Skip);
                }
                if !def.concrete {
                    self.diag(
                        Diagnostic::error(
                            Rule::AbstractInstantiated,
                            format!("abstract class {} instantiated", def.name),
                        )
                        .with_class(def.name),
                        pos,
                    );
                }
                if self.options.report_deprecated && def.deprecated {
                    self.diag(
                        Diagnostic::info(
                            Rule::Deprecated,
                            format!("deprecated class {}", def.name),
                        )
                        .with_class(def.name),
                        pos,
                    );
                }
                let mrid = self.object_identity(e, def.name, pos)?;
                let mut o = Object::new(class, mrid);
                o.mark_profile(st.profile_mask);
                st.object = Some(o);
                Ok(Ctx::Object)
            }
            Resolved::Attr(_) | Resolved::Unknown => {
                let name = String::from_utf8_lossy(qname).into_owned();
                if self.options.strictness == Strictness::Strict {
                    return Err(Error::NotCimXml(format!("unknown class element <{name}>")));
                }
                self.diag(
                    Diagnostic::warning(
                        Rule::UnknownClass,
                        format!("unknown class element <{name}>, skipped"),
                    ),
                    pos,
                );
                Ok(Ctx::Skip)
            }
        }
    }

    /// An element inside `md:FullModel` or `dm:DifferenceModel`.
    fn open_in_header(
        &mut self,
        e: &BytesStart<'_>,
        qname: &[u8],
        self_closing: bool,
        st: &mut State,
    ) -> Result<Ctx> {
        let Some((ns, local)) = self
            .expand(qname)
            .map(|(n, l)| (n.to_owned(), l.to_owned()))
        else {
            return Ok(Ctx::Skip);
        };
        // Difference statement sets live inside the header element.
        if ns == DM_NS && (local == "forwardDifferences" || local == "reverseDifferences") {
            st.diff_forward = local == "forwardDifferences";
            if st.difference.is_none() {
                st.difference = Some(DifferenceModel::default());
            }
            return Ok(Ctx::DiffSet);
        }
        let prefix = prefix_of(qname);
        if let Some(res) = self.attr(e, RDF_NS, "resource")? {
            let res = res.trim().to_owned();
            apply_header_resource(&mut st.header, &prefix, &ns, &local, res);
            return Ok(Ctx::Skip);
        }
        if self_closing {
            return Ok(Ctx::Skip);
        }
        st.pending = Some(PendingText {
            target: TextTarget::Header { prefix, ns, local },
            text: String::new(),
        });
        Ok(Ctx::Text)
    }

    /// Resolve a property element of `class` and decide what to do with it.
    fn property(
        &mut self,
        e: &BytesStart<'_>,
        self_closing: bool,
        class: ClassId,
        owner: &Mrid,
        pos: u64,
    ) -> Result<Prop> {
        let qname = e.name();
        let qname = qname.as_ref();
        let Resolved::Attr(attr) = self.resolve(qname) else {
            let name = String::from_utf8_lossy(qname).into_owned();
            if self.options.strictness == Strictness::Strict {
                return Err(Error::NotCimXml(format!(
                    "unknown property element <{name}>"
                )));
            }
            self.diag(
                Diagnostic::warning(
                    Rule::UnknownAttribute,
                    format!("unknown property <{name}>, skipped"),
                )
                .with_object(owner.clone()),
                pos,
            );
            return Ok(Prop::Skip);
        };

        let def = self.schema.attr(attr);
        // Guard against a property being applied to the wrong class.
        if !self.schema.is_a(class, def.owner) {
            self.diag(
                Diagnostic::warning(
                    Rule::UnknownAttribute,
                    format!(
                        "{} does not apply to class {}",
                        def.name,
                        self.schema.class(class).name
                    ),
                )
                .with_attribute(def.name)
                .with_object(owner.clone()),
                pos,
            );
            return Ok(Prop::Skip);
        }
        if self.options.report_deprecated && def.deprecated {
            self.diag(
                Diagnostic::info(
                    Rule::Deprecated,
                    format!("deprecated attribute {}", def.name),
                )
                .with_attribute(def.name),
                pos,
            );
        }

        // A compound is written inline, so it is recognised from the schema rather than
        // from `rdf:parseType`: producers do omit the attribute, and the nested content
        // is unambiguous either way.
        if let AttrKind::Compound(target) = def.kind {
            if self.attr(e, RDF_NS, "resource")?.is_some() {
                self.diag(
                    Diagnostic::error(
                        Rule::InvalidValue,
                        format!(
                            "{} holds a compound, which has no identity, but is written as \
                             a reference",
                            def.name
                        ),
                    )
                    .with_attribute(def.name)
                    .with_object(owner.clone()),
                    pos,
                );
                return Ok(Prop::Skip);
            }
            return Ok(Prop::Enter(attr, target));
        }

        if let Some(res) = self.attr(e, RDF_NS, "resource")? {
            let res = res.trim();
            return Ok(match self.resource_value(attr, res, owner, pos) {
                Some(v) => Prop::Value(attr, v),
                None => Prop::Skip,
            });
        }
        if self_closing {
            // An empty element means an empty string value.
            if matches!(def.kind, AttrKind::Primitive(Primitive::String)) {
                return Ok(Prop::Value(attr, Value::Text("".into())));
            }
            return Ok(Prop::Skip);
        }
        Ok(Prop::Text(attr))
    }

    /// Determine an object's mRID from `rdf:ID` or `rdf:about`.
    fn object_identity(
        &mut self,
        e: &BytesStart<'_>,
        class: &'static str,
        pos: u64,
    ) -> Result<Mrid> {
        for (name, raw) in [
            ("rdf:ID", self.attr(e, RDF_NS, "ID")?),
            ("rdf:about", self.attr(e, RDF_NS, "about")?),
        ] {
            let Some(raw) = raw else { continue };
            let raw = raw.trim();
            let m = Mrid::parse(raw);
            if self.options.report_non_conforming_mrids && !m.is_conforming() {
                let complaint = if m.is_uuid() {
                    // A UUID written without hyphens: the value is recoverable and the
                    // reference joins, so this is about the file's form, not its meaning.
                    format!(
                        "is a UUID written without hyphens; IEC 61970-552 requires {:?}",
                        m.canonical()
                    )
                } else {
                    "is not a UUID as IEC 61970-552 requires".to_owned()
                };
                self.diag(
                    Diagnostic::warning(
                        Rule::NonConformingMrid,
                        format!("{name} {raw:?} {complaint}"),
                    )
                    .with_class(class),
                    pos,
                );
            }
            return Ok(m);
        }
        // An object with no identifier at all still has to be *an* object. Giving them all
        // the same empty identifier merged them into one — the second overwriting the
        // first's values under a class it may not even share — so the document position
        // stands in, which is unique within a file and distinct from any UUID. The value is
        // deliberately opaque rather than a synthesized UUID: inventing a conforming
        // identifier would hide the defect from every later check.
        let placeholder = match &self.source {
            Some(src) => format!("no-identifier-{src}-{pos}"),
            None => format!("no-identifier-{pos}"),
        };
        self.diag(
            Diagnostic::error(
                Rule::Structure,
                format!("<{class}> has neither rdf:ID nor rdf:about; kept as {placeholder:?}"),
            )
            .with_class(class),
            pos,
        );
        Ok(Mrid::parse(&placeholder))
    }

    /// Turn an `rdf:resource` IRI into a value for `attr`.
    fn resource_value(&mut self, attr: AttrId, res: &str, owner: &Mrid, pos: u64) -> Option<Value> {
        let def = self.schema.attr(attr);
        let AttrKind::Enumeration(_) = def.kind else {
            return Some(Value::Reference(Mrid::parse(res)));
        };
        // Enumeration literals are absolute IRIs: `<ns>#Enum.literal`.
        let Some((ns, local)) = res.rsplit_once('#') else {
            self.diag(
                Diagnostic::error(
                    Rule::InvalidValue,
                    format!("enumeration value {res:?} is not an IRI"),
                )
                .with_attribute(def.name),
                pos,
            );
            return None;
        };
        if let Some(v) = self.schema.find_enum_value(&format!("{ns}#"), local) {
            return Some(Value::Enum(v));
        }
        // The qualified name is unambiguous even when the namespace is wrong, so recover
        // the value and report the mismatch rather than drop data.
        match self.schema.find_enum_value_any_ns(local) {
            Some(v) => {
                let correct = self.schema.namespace(self.schema.enum_value(v).ns).iri;
                self.diag(
                    Diagnostic::warning(
                        Rule::InvalidValue,
                        format!(
                            "enumeration literal {local} is written in namespace {ns}# but is \
                             declared in {correct}; value recovered"
                        ),
                    )
                    .with_attribute(def.name)
                    .with_object(owner.clone()),
                    pos,
                );
                Some(Value::Enum(v))
            }
            None => {
                self.diag(
                    Diagnostic::error(
                        Rule::InvalidValue,
                        format!("unknown enumeration literal {res:?}"),
                    )
                    .with_attribute(def.name)
                    .with_object(owner.clone()),
                    pos,
                );
                None
            }
        }
    }

    /// Commit an accumulated text value.
    fn finish_text(&mut self, mut p: PendingText, st: &mut State, pos: u64) {
        // A character XML 1.0 cannot represent has no escaped form, so a value carrying
        // one cannot be written back at all: the document this crate would produce is
        // refused by every conforming parser at that byte. `quick-xml` does not enforce
        // the `Char` production in either direction, so this is where a character arriving
        // from a mis-encoded or corrupted source file is caught. It is dropped — the only
        // alternative to unreadable output — and the loss is reported rather than silent.
        if let Some((offset, c)) = crate::xml::find_illegal(&p.text) {
            self.diag(
                Diagnostic::warning(
                    Rule::IllegalXmlCharacter,
                    format!(
                        "value holds {}, which XML 1.0 cannot represent in any form; \
                         removed (first at byte {offset} of the value)",
                        crate::xml::describe_char(c)
                    ),
                ),
                pos,
            );
            p.text = crate::xml::strip_illegal(&p.text).into_owned();
        }
        match p.target {
            TextTarget::Attr(attr) => {
                let Some(obj) = st.object.as_ref() else {
                    return;
                };
                if let Some(v) = self.parse_text(attr, &p.text, obj.mrid(), pos) {
                    let obj = st.object.as_mut().expect("object present");
                    if self.schema.attr(attr).mult.is_many() {
                        obj.push_in(st.profile_mask, attr, v);
                    } else {
                        obj.set_in(st.profile_mask, attr, v);
                    }
                }
            }
            TextTarget::Compound(attr) => {
                let mrid = st.object.as_ref().map_or(&UNKNOWN_MRID, |o| o.mrid());
                if let Some(v) = self.parse_text(attr, &p.text, mrid, pos)
                    && let Some((_, top)) = st.compounds.last_mut()
                {
                    if self.schema.attr(attr).mult.is_many() {
                        top.push(attr, v);
                    } else {
                        top.set(attr, v);
                    }
                }
            }
            TextTarget::Header { prefix, ns, local } => {
                apply_header_text(&mut st.header, &prefix, &ns, &local, p.text);
            }
            TextTarget::Statement {
                subject,
                class,
                defines_subject,
                ns,
                local,
                forward,
            } => push_statement(
                &mut st.difference,
                forward,
                Statement {
                    subject,
                    class,
                    defines_subject,
                    predicate_ns: ns,
                    predicate: local,
                    value: StatementValue::Literal(p.text),
                },
            ),
        }
    }

    fn parse_text(&mut self, attr: AttrId, text: &str, owner: &Mrid, pos: u64) -> Option<Value> {
        let def = self.schema.attr(attr);
        let prim = match def.kind {
            AttrKind::Primitive(prim) => prim,
            AttrKind::Datatype(dt) => self.schema.datatype(dt).value,
            // An enumeration, association or compound written as text is malformed; keep
            // the text so nothing is silently lost.
            AttrKind::Enumeration(_) | AttrKind::Association { .. } | AttrKind::Compound(_) => {
                Primitive::String
            }
        };
        match Value::parse_primitive(prim, text) {
            Ok(v) => Some(v),
            Err(e) => {
                let name = def.name;
                self.diag(
                    Diagnostic::error(Rule::InvalidValue, e.to_string())
                        .with_attribute(name)
                        .with_object(owner.clone()),
                    pos,
                );
                None
            }
        }
    }

    /// Record the namespace bindings declared on an element.
    ///
    /// Bindings are kept in one flat map rather than a scope stack: CIM/XML declares
    /// everything on `rdf:RDF`, and a document that *rebinds* a prefix deeper down — as
    /// opposed to adding one — is beyond what IEC 61970-552 describes. Adding a binding
    /// found deeper is strictly better than leaving every element below it unresolvable.
    fn collect_namespaces(&mut self, e: &BytesStart<'_>) -> Result<()> {
        for attr in e.attributes().with_checks(false) {
            let attr = attr?;
            let key = attr.key.as_ref();
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                .map_err(|e| Error::Xml(e.to_string()))?
                .into_owned();
            if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                self.ns.insert(prefix.to_vec(), value);
            } else if key == b"xmlns" {
                self.ns.insert(Vec::new(), value);
            }
        }
        Ok(())
    }

    /// Read an attribute by namespace and local name, expanding its prefix.
    ///
    /// Borrows from the element where XML escaping allows, so the common case — one
    /// `rdf:ID` or `rdf:resource` per element, repeated hundreds of thousands of times —
    /// does not allocate.
    fn attr<'e>(
        &self,
        e: &'e BytesStart<'_>,
        ns: &str,
        local: &str,
    ) -> Result<Option<Cow<'e, str>>> {
        for attr in e.attributes().with_checks(false) {
            let attr = attr?;
            let key = attr.key.as_ref();
            let Some(i) = key.iter().position(|&b| b == b':') else {
                continue;
            };
            if &key[i + 1..] != local.as_bytes() {
                continue;
            }
            if self.ns.get(&key[..i]).map(String::as_str) != Some(ns) {
                continue;
            }
            return Ok(Some(
                attr.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                    .map_err(|e| Error::Xml(e.to_string()))?,
            ));
        }
        Ok(None)
    }
}

/// Whether an element carries an `xmlns` declaration, without parsing its attributes.
fn declares_namespace(e: &BytesStart<'_>) -> bool {
    e.as_ref().windows(5).any(|w| w == b"xmlns")
}

/// The prefix an element was written with, e.g. `md` for `md:Model.created`.
fn prefix_of(qname: &[u8]) -> String {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => String::from_utf8_lossy(&qname[..i]).into_owned(),
        None => String::new(),
    }
}

/// A header property given as `rdf:resource`.
fn apply_header_resource(
    header: &mut Option<ModelHeader>,
    prefix: &str,
    ns: &str,
    local: &str,
    res: String,
) {
    let Some(h) = header.as_mut() else { return };
    match (ns, local) {
        (MD_NS, "Model.DependentOn") => h.dependent_on.push(Mrid::parse(&res)),
        (MD_NS, "Model.Supersedes") => h.supersedes.push(Mrid::parse(&res)),
        (MD_NS, "Model.profile") => h.profiles.push(res),
        _ => h
            .extra
            .push(HeaderProperty::resource(prefix, ns, local, res)),
    }
}

/// A header property given as element text.
fn apply_header_text(
    header: &mut Option<ModelHeader>,
    prefix: &str,
    ns: &str,
    local: &str,
    text: String,
) {
    let Some(h) = header.as_mut() else { return };
    if ns != MD_NS {
        h.extra.push(HeaderProperty::text(prefix, ns, local, text));
        return;
    }
    match local {
        "Model.created" => h.created = Some(text),
        "Model.scenarioTime" => h.scenario_time = Some(text),
        "Model.description" => h.description = Some(text),
        "Model.version" => h.version = Some(text),
        "Model.modelingAuthoritySet" => h.modeling_authority_set = Some(text),
        "Model.profile" => h.profiles.push(text),
        "Model.DependentOn" => h.dependent_on.push(Mrid::parse(&text)),
        "Model.Supersedes" => h.supersedes.push(Mrid::parse(&text)),
        _ => h.extra.push(HeaderProperty::text(prefix, ns, local, text)),
    }
}

fn push_statement(difference: &mut Option<DifferenceModel>, forward: bool, s: Statement) {
    let d = difference.get_or_insert_with(DifferenceModel::default);
    if forward {
        d.forward.push(s);
    } else {
        d.reverse.push(s);
    }
}

struct PendingText {
    target: TextTarget,
    text: String,
}

enum TextTarget {
    /// An attribute of the object under construction.
    Attr(AttrId),
    /// A field of the innermost compound under construction.
    Compound(AttrId),
    /// A header property, by prefix, namespace and local name.
    Header {
        prefix: String,
        ns: String,
        local: String,
    },
    /// A difference-model statement.
    Statement {
        subject: Mrid,
        class: Option<QualifiedName>,
        defines_subject: bool,
        ns: String,
        local: String,
        forward: bool,
    },
}
