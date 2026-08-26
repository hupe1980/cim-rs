//! Streaming reader for CIM/XML instance documents (IEC 61970-552).
//!
//! IEC 61970-552 constrains RDF/XML to a small, regular subset: a flat `rdf:RDF` root
//! containing a header and a list of objects, where each object is a class element whose
//! property children are either element text or a single `rdf:resource`. A purpose-built
//! pull parser over that subset is both much faster and much simpler than a general RDF
//! toolchain, and it never holds the document in memory.

use std::collections::HashMap;
use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::dataset::Dataset;
use crate::error::{Diagnostic, Error, Report, Result, Rule};
use crate::header::{
    DM_NS, DifferenceModel, MD_NS, ModelHeader, ModelKind, QualifiedName, Statement, StatementValue,
};
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::{AttrId, AttrKind, ClassId, Primitive, ProfileId, ProfileMask, Schema};
use crate::value::Value;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

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
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            strictness: Strictness::Lenient,
            assume_profile: None,
            report_deprecated: false,
            report_non_conforming_mrids: false,
        }
    }
}

impl ReadOptions {
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
            assume_profile: None,
        }
    }

    pub fn with_profile(mut self, profile: ProfileId) -> Self {
        self.assume_profile = Some(profile);
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
    /// Inside `dm:forwardDifferences` / `dm:reverseDifferences`.
    DiffSet,
    /// Inside an `rdf:Description` within a difference set.
    DiffSubject,
    /// Inside an element that is being skipped wholesale.
    Skip,
}

struct Parser<'a> {
    schema: &'static Schema,
    options: &'a ReadOptions,
    source: Option<String>,
    report: Report,
    header_only: bool,
    /// Prefix bytes -> namespace IRI, declared on the root element.
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
            ns: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    fn diag(&mut self, d: Diagnostic, line: Option<u64>) {
        let mut d = d;
        if let Some(s) = &self.source {
            d = d.with_source(s.clone());
        }
        if let Some(l) = line {
            d = d.with_line(l);
        }
        self.report.push(d);
    }

    /// Expand an element QName to `(namespace IRI, local name)`.
    fn expand(&self, qname: &[u8]) -> Option<(&str, String)> {
        let (prefix, local) = match qname.iter().position(|&b| b == b':') {
            Some(i) => (&qname[..i], &qname[i + 1..]),
            None => (&b""[..], qname),
        };
        let ns = self.ns.get(prefix)?;
        Some((ns.as_str(), String::from_utf8_lossy(local).into_owned()))
    }

    fn resolve(&mut self, qname: &[u8]) -> Resolved {
        if let Some(r) = self.cache.get(qname) {
            return *r;
        }
        let resolved = match self.expand(qname) {
            Some((ns, local)) => {
                // A dot separates class from property: `cim:ACLineSegment.r`.
                if local.contains('.') {
                    match self.schema.find_attr(ns, &local) {
                        Some(a) => Resolved::Attr(a),
                        None => Resolved::Unknown,
                    }
                } else {
                    match self.schema.find_class(ns, &local) {
                        Some(c) => Resolved::Class(c),
                        None => Resolved::Unknown,
                    }
                }
            }
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

        let mut header: Option<ModelHeader> = None;
        let mut difference: Option<DifferenceModel> = None;
        let mut objects_read = 0usize;
        let mut saw_root = false;

        // Object under construction, plus the profile mask to attribute it to.
        let mut object: Option<Object> = None;
        let mut profile_mask: ProfileMask = self.options.assume_profile.map_or(0, |p| p.mask());

        // Property whose text is being accumulated.
        let mut pending: Option<PendingText> = None;
        // Difference-model assembly state.
        let mut diff_subject: Option<(Mrid, Option<QualifiedName>)> = None;
        let mut diff_into_forward = false;

        loop {
            let ev = reader
                .read_event_into(&mut buf)
                .map_err(|e| Error::Xml(e.to_string()))?;
            let line = None::<u64>;

            match ev {
                Event::Eof => break,

                Event::Start(e) => {
                    let next = self.on_open(
                        &e,
                        false,
                        ctx,
                        &mut saw_root,
                        &mut header,
                        &mut difference,
                        &mut object,
                        &mut pending,
                        &mut diff_subject,
                        &mut diff_into_forward,
                        &mut profile_mask,
                        line,
                    )?;
                    stack.push(ctx);
                    ctx = next;
                }

                Event::Empty(e) => {
                    // Self-closing element: open and close in one step, so the context
                    // stack is untouched.
                    let saved = ctx;
                    let entered = self.on_open(
                        &e,
                        true,
                        ctx,
                        &mut saw_root,
                        &mut header,
                        &mut difference,
                        &mut object,
                        &mut pending,
                        &mut diff_subject,
                        &mut diff_into_forward,
                        &mut profile_mask,
                        line,
                    )?;
                    // An empty property element carries no text; drop any pending slot.
                    pending = None;
                    // An object with no properties is written `<cim:Class rdf:ID="…"/>`.
                    // It closes immediately, so commit it here — otherwise it would be
                    // lost and would overwrite whichever object came next.
                    if entered == Ctx::Object
                        && let Some(o) = object.take()
                    {
                        self.commit(o, profile_mask, &mut dataset, line);
                        objects_read += 1;
                    }
                    if entered == Ctx::DiffSubject {
                        diff_subject = None;
                    }
                    ctx = saved;
                }

                Event::Text(t) => {
                    if let Some(p) = pending.as_mut() {
                        let text = t.unescape().map_err(|e| Error::Xml(e.to_string()))?;
                        p.text.push_str(&text);
                    }
                }

                Event::CData(t) => {
                    if let Some(p) = pending.as_mut() {
                        p.text.push_str(&String::from_utf8_lossy(&t));
                    }
                }

                Event::End(_) => {
                    // The header precedes every object, so once it closes the profiles
                    // the file declares are known and objects can be attributed to them.
                    if ctx == Ctx::Header
                        && let Some(h) = header.as_ref()
                    {
                        let declared = h
                            .profiles
                            .iter()
                            .filter_map(|iri| self.schema.profile_by_iri(iri))
                            .fold(0, |acc, p| acc | p.mask());
                        if declared != 0 {
                            profile_mask = declared;
                        }
                    }
                    // Close the innermost pending property, if any.
                    if let Some(p) = pending.take() {
                        self.finish_text(
                            p,
                            profile_mask,
                            &mut object,
                            &mut header,
                            &mut difference,
                            &diff_subject,
                            diff_into_forward,
                            line,
                        );
                    } else {
                        match ctx {
                            Ctx::Object => {
                                if let Some(o) = object.take() {
                                    self.commit(o, profile_mask, &mut dataset, line);
                                    objects_read += 1;
                                }
                            }
                            Ctx::DiffSubject => diff_subject = None,
                            _ => {}
                        }
                    }
                    ctx = stack.pop().unwrap_or(Ctx::Outside);
                }

                _ => {}
            }
            buf.clear();

            // Reading only the header: stop as soon as it is complete.
            if self.header_only && header.is_some() && ctx == Ctx::Body {
                break;
            }
        }

        if !saw_root {
            return Err(Error::NotCimXml("no rdf:RDF root element found".to_owned()));
        }

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

        Ok(ReadOutcome {
            header,
            difference,
            report: std::mem::take(&mut self.report),
            objects_read,
        })
    }

    /// Store a finished object, reporting an identifier collision that is not a merge.
    ///
    /// Two files describing the same object is normal and is how profiles compose. Two
    /// files giving the same identifier to *unrelated* classes is a defect, because one
    /// of them will be silently absorbed into the other.
    fn commit(
        &mut self,
        mut object: Object,
        profile_mask: ProfileMask,
        dataset: &mut Option<&mut Dataset>,
        line: Option<u64>,
    ) {
        object.mark_profile(profile_mask);
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
                .with_object(object.mrid().canonical());
                self.diag(d, line);
            }
        }
        // Re-borrow: `by_mrid` above held an immutable borrow.
        if let Some(ds) = dataset.as_mut() {
            ds.insert(object);
        }
    }

    /// Handle an opening (or self-closing) element. Returns the context to enter.
    #[allow(clippy::too_many_arguments)]
    fn on_open(
        &mut self,
        e: &BytesStart<'_>,
        self_closing: bool,
        ctx: Ctx,
        saw_root: &mut bool,
        header: &mut Option<ModelHeader>,
        difference: &mut Option<DifferenceModel>,
        object: &mut Option<Object>,
        pending: &mut Option<PendingText>,
        diff_subject: &mut Option<(Mrid, Option<QualifiedName>)>,
        diff_into_forward: &mut bool,
        profile_mask: &mut ProfileMask,
        line: Option<u64>,
    ) -> Result<Ctx> {
        let qname = e.name().as_ref().to_vec();

        // The root element carries every namespace binding used in the document.
        if !*saw_root {
            self.collect_namespaces(e)?;
            if let Some((ns, local)) = self.expand(&qname)
                && ns == RDF_NS
                && local == "RDF"
            {
                *saw_root = true;
                return Ok(Ctx::Body);
            }
            // Some producers put the namespaces on a wrapper; keep looking.
            return Ok(ctx);
        }

        let expanded = self.expand(&qname);

        match ctx {
            Ctx::Body => {
                if let Some((ns, local)) = &expanded {
                    // Header elements.
                    if (*ns == MD_NS && local == "FullModel")
                        || (*ns == DM_NS && local == "DifferenceModel")
                    {
                        let kind = if *ns == DM_NS {
                            ModelKind::Difference
                        } else {
                            ModelKind::Full
                        };
                        let mut h = ModelHeader::new(kind);
                        h.id = self.attr(e, RDF_NS, "about")?.map(|s| Mrid::parse(&s));
                        h.source = self.source.clone();
                        *header = Some(h);
                        if kind == ModelKind::Difference {
                            *difference = Some(DifferenceModel::default());
                        }
                        return Ok(Ctx::Header);
                    }
                }

                if self.header_only {
                    return Ok(Ctx::Skip);
                }

                // Otherwise this should be an object element.
                match self.resolve(&qname) {
                    Resolved::Class(class) => {
                        let def = self.schema.class(class);
                        if !def.concrete {
                            self.diag(
                                Diagnostic::error(
                                    Rule::AbstractInstantiated,
                                    format!("abstract class {} instantiated", def.name),
                                )
                                .with_class(def.name),
                                line,
                            );
                        }
                        if self.options.report_deprecated && def.deprecated {
                            self.diag(
                                Diagnostic::info(
                                    Rule::Deprecated,
                                    format!("deprecated class {}", def.name),
                                )
                                .with_class(def.name),
                                line,
                            );
                        }
                        let mrid = self.object_identity(e, def.name, line)?;
                        let mut o = Object::new(class, mrid);
                        o.mark_profile(*profile_mask);
                        *object = Some(o);
                        Ok(Ctx::Object)
                    }
                    Resolved::Attr(_) | Resolved::Unknown => {
                        let name = String::from_utf8_lossy(&qname).into_owned();
                        if self.options.strictness == Strictness::Strict {
                            return Err(Error::NotCimXml(format!(
                                "unknown class element <{name}>"
                            )));
                        }
                        self.diag(
                            Diagnostic::warning(
                                Rule::UnknownClass,
                                format!("unknown class element <{name}>, skipped"),
                            ),
                            line,
                        );
                        Ok(Ctx::Skip)
                    }
                }
            }

            Ctx::Header => {
                let Some((ns, local)) = expanded else {
                    return Ok(Ctx::Skip);
                };
                // Difference statement sets live inside the header element.
                if ns == DM_NS && (local == "forwardDifferences" || local == "reverseDifferences") {
                    *diff_into_forward = local == "forwardDifferences";
                    if difference.is_none() {
                        *difference = Some(DifferenceModel::default());
                    }
                    return Ok(Ctx::DiffSet);
                }
                if let Some(res) = self.attr(e, RDF_NS, "resource")? {
                    self.apply_header_resource(header, &local, res);
                    return Ok(Ctx::Skip);
                }
                if self_closing {
                    return Ok(Ctx::Skip);
                }
                *pending = Some(PendingText {
                    target: TextTarget::Header(local),
                    text: String::new(),
                });
                Ok(Ctx::Skip)
            }

            Ctx::Object => {
                let Some(obj) = object.as_mut() else {
                    return Ok(Ctx::Skip);
                };
                match self.cached_resolve(&qname) {
                    Resolved::Attr(attr) => {
                        let def = self.schema.attr(attr);
                        // Guard against a property being applied to the wrong class.
                        if !self.schema.is_a(obj.class(), def.owner) {
                            self.diag(
                                Diagnostic::warning(
                                    Rule::UnknownAttribute,
                                    format!(
                                        "{} does not apply to class {}",
                                        def.name,
                                        self.schema.class(obj.class()).name
                                    ),
                                )
                                .with_attribute(def.name)
                                .with_object(obj.mrid().canonical()),
                                line,
                            );
                            return Ok(Ctx::Skip);
                        }
                        if self.options.report_deprecated && def.deprecated {
                            self.diag(
                                Diagnostic::info(
                                    Rule::Deprecated,
                                    format!("deprecated attribute {}", def.name),
                                )
                                .with_attribute(def.name),
                                line,
                            );
                        }
                        if let Some(res) = self.attr(e, RDF_NS, "resource")? {
                            let mask = *profile_mask;
                            self.apply_resource(obj, attr, &res, mask, line);
                            return Ok(Ctx::Skip);
                        }
                        if self_closing {
                            // An empty element means an empty string value.
                            if let AttrKind::Primitive(p) = def.kind
                                && matches!(p, Primitive::String)
                            {
                                obj.push_in(*profile_mask, attr, Value::Text("".into()));
                            }
                            return Ok(Ctx::Skip);
                        }
                        *pending = Some(PendingText {
                            target: TextTarget::Attr(attr),
                            text: String::new(),
                        });
                        Ok(Ctx::Skip)
                    }
                    _ => {
                        let name = String::from_utf8_lossy(&qname).into_owned();
                        if self.options.strictness == Strictness::Strict {
                            return Err(Error::NotCimXml(format!(
                                "unknown property element <{name}>"
                            )));
                        }
                        let mrid = obj.mrid().canonical();
                        self.diag(
                            Diagnostic::warning(
                                Rule::UnknownAttribute,
                                format!("unknown property <{name}>, skipped"),
                            )
                            .with_object(mrid),
                            line,
                        );
                        Ok(Ctx::Skip)
                    }
                }
            }

            Ctx::DiffSet => {
                // Each statement group names its subject, either with a bare
                // `rdf:Description` or with the class element itself. The latter carries
                // real information: a difference may reclassify an object.
                if let Some(about) = self
                    .attr(e, RDF_NS, "about")?
                    .or(self.attr(e, RDF_NS, "ID")?)
                {
                    let class = expanded.and_then(|(ns, local)| {
                        (!(ns == RDF_NS && local == "Description")).then(|| QualifiedName {
                            ns: ns.to_owned(),
                            local,
                        })
                    });
                    *diff_subject = Some((Mrid::parse(&about), class));
                    return Ok(Ctx::DiffSubject);
                }
                Ok(Ctx::Skip)
            }

            Ctx::DiffSubject => {
                let Some((ns, local)) = expanded else {
                    return Ok(Ctx::Skip);
                };
                let Some((subject, class)) = diff_subject.clone() else {
                    return Ok(Ctx::Skip);
                };
                if let Some(res) = self.attr(e, RDF_NS, "resource")? {
                    push_statement(
                        difference,
                        *diff_into_forward,
                        Statement {
                            subject,
                            class,
                            predicate_ns: ns.to_owned(),
                            predicate: local,
                            value: StatementValue::Resource(res),
                        },
                    );
                    return Ok(Ctx::Skip);
                }
                if self_closing {
                    return Ok(Ctx::Skip);
                }
                *pending = Some(PendingText {
                    target: TextTarget::Statement {
                        subject,
                        class,
                        ns: ns.to_owned(),
                        local,
                        forward: *diff_into_forward,
                    },
                    text: String::new(),
                });
                Ok(Ctx::Skip)
            }

            Ctx::Skip | Ctx::Outside => Ok(Ctx::Skip),
        }
    }

    /// Resolve using only the cache-backed path (the element name was already seen or
    /// will be inserted), keeping the borrow checker happy inside `Ctx::Object`.
    fn cached_resolve(&mut self, qname: &[u8]) -> Resolved {
        self.resolve(qname)
    }

    /// Determine an object's mRID from `rdf:ID` or `rdf:about`.
    fn object_identity(
        &mut self,
        e: &BytesStart<'_>,
        class: &'static str,
        line: Option<u64>,
    ) -> Result<Mrid> {
        if let Some(id) = self.attr(e, RDF_NS, "ID")? {
            let m = Mrid::parse(&id);
            if self.options.report_non_conforming_mrids && !m.is_uuid() {
                self.diag(
                    Diagnostic::warning(
                        Rule::NonConformingMrid,
                        format!("rdf:ID {id:?} is not a UUID as IEC 61970-552 requires"),
                    )
                    .with_class(class),
                    line,
                );
            }
            return Ok(m);
        }
        if let Some(about) = self.attr(e, RDF_NS, "about")? {
            let m = Mrid::parse(&about);
            if self.options.report_non_conforming_mrids && !m.is_uuid() {
                self.diag(
                    Diagnostic::warning(
                        Rule::NonConformingMrid,
                        format!("rdf:about {about:?} is not a UUID as IEC 61970-552 requires"),
                    )
                    .with_class(class),
                    line,
                );
            }
            return Ok(m);
        }
        self.diag(
            Diagnostic::error(
                Rule::Structure,
                format!("<{class}> has neither rdf:ID nor rdf:about"),
            )
            .with_class(class),
            line,
        );
        Ok(Mrid::parse(""))
    }

    /// Apply an `rdf:resource` value to an object attribute.
    fn apply_resource(
        &mut self,
        obj: &mut Object,
        attr: AttrId,
        res: &str,
        profiles: ProfileMask,
        line: Option<u64>,
    ) {
        let def = self.schema.attr(attr);
        let value = match def.kind {
            AttrKind::Enumeration(_) => {
                // Enumeration literals are absolute IRIs: `<ns>#Enum.literal`.
                let Some((ns, local)) = res.rsplit_once('#') else {
                    self.diag(
                        Diagnostic::error(
                            Rule::InvalidValue,
                            format!("enumeration value {res:?} is not an IRI"),
                        )
                        .with_attribute(def.name),
                        line,
                    );
                    return;
                };
                match self.schema.find_enum_value(&format!("{ns}#"), local) {
                    Some(v) => Value::Enum(v),
                    // The qualified name is unambiguous even when the namespace is wrong,
                    // so recover the value and report the mismatch rather than drop data.
                    None => match self.schema.find_enum_value_any_ns(local) {
                        Some(v) => {
                            let correct = self.schema.namespace(self.schema.enum_value(v).ns).iri;
                            self.diag(
                                Diagnostic::warning(
                                    Rule::InvalidValue,
                                    format!(
                                        "enumeration literal {local} is written in namespace \
                                         {ns}# but is declared in {correct}; value recovered"
                                    ),
                                )
                                .with_attribute(def.name)
                                .with_object(obj.mrid().canonical()),
                                line,
                            );
                            Value::Enum(v)
                        }
                        None => {
                            self.diag(
                                Diagnostic::error(
                                    Rule::InvalidValue,
                                    format!("unknown enumeration literal {res:?}"),
                                )
                                .with_attribute(def.name)
                                .with_object(obj.mrid().canonical()),
                                line,
                            );
                            return;
                        }
                    },
                }
            }
            _ => Value::Reference(Mrid::parse(res)),
        };
        if def.mult.is_many() {
            obj.push_in(profiles, attr, value);
        } else {
            obj.set_in(profiles, attr, value);
        }
    }

    fn apply_header_resource(
        &mut self,
        header: &mut Option<ModelHeader>,
        local: &str,
        res: String,
    ) {
        let Some(h) = header.as_mut() else { return };
        match local {
            "Model.DependentOn" => h.dependent_on.push(Mrid::parse(&res)),
            "Model.Supersedes" => h.supersedes.push(Mrid::parse(&res)),
            "Model.profile" => h.profiles.push(res),
            other => h.extra.push((other.to_owned(), res)),
        }
    }

    /// Commit an accumulated text value.
    #[allow(clippy::too_many_arguments)]
    fn finish_text(
        &mut self,
        p: PendingText,
        profiles: ProfileMask,
        object: &mut Option<Object>,
        header: &mut Option<ModelHeader>,
        difference: &mut Option<DifferenceModel>,
        _diff_subject: &Option<(Mrid, Option<QualifiedName>)>,
        _forward: bool,
        line: Option<u64>,
    ) {
        match p.target {
            TextTarget::Attr(attr) => {
                let Some(obj) = object.as_mut() else { return };
                let def = self.schema.attr(attr);
                let prim = match def.kind {
                    AttrKind::Primitive(prim) => prim,
                    AttrKind::Datatype(dt) => self.schema.datatype(dt).value,
                    // An enumeration or association written as text is malformed; keep
                    // the text so nothing is silently lost.
                    AttrKind::Enumeration(_) | AttrKind::Association { .. } => Primitive::String,
                };
                match Value::parse_primitive(prim, &p.text) {
                    Ok(v) => {
                        if def.mult.is_many() {
                            obj.push_in(profiles, attr, v);
                        } else {
                            obj.set_in(profiles, attr, v);
                        }
                    }
                    Err(e) => {
                        let mrid = obj.mrid().canonical();
                        self.diag(
                            Diagnostic::error(Rule::InvalidValue, e.to_string())
                                .with_attribute(def.name)
                                .with_object(mrid),
                            line,
                        );
                    }
                }
            }
            TextTarget::Header(local) => {
                let Some(h) = header.as_mut() else { return };
                let text = p.text;
                match local.as_str() {
                    "Model.created" => h.created = Some(text),
                    "Model.scenarioTime" => h.scenario_time = Some(text),
                    "Model.description" => h.description = Some(text),
                    "Model.version" => h.version = Some(text),
                    "Model.modelingAuthoritySet" => h.modeling_authority_set = Some(text),
                    "Model.profile" => h.profiles.push(text),
                    "Model.DependentOn" => h.dependent_on.push(Mrid::parse(&text)),
                    "Model.Supersedes" => h.supersedes.push(Mrid::parse(&text)),
                    other => h.extra.push((other.to_owned(), text)),
                }
            }
            TextTarget::Statement {
                subject,
                class,
                ns,
                local,
                forward,
            } => push_statement(
                difference,
                forward,
                Statement {
                    subject,
                    class,
                    predicate_ns: ns,
                    predicate: local,
                    value: StatementValue::Literal(p.text),
                },
            ),
        }
    }

    fn collect_namespaces(&mut self, e: &BytesStart<'_>) -> Result<()> {
        for attr in e.attributes().with_checks(false) {
            let attr = attr?;
            let key = attr.key.as_ref();
            let value = attr
                .unescape_value()
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
    fn attr(&self, e: &BytesStart<'_>, ns: &str, local: &str) -> Result<Option<String>> {
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
                attr.unescape_value()
                    .map_err(|e| Error::Xml(e.to_string()))?
                    .trim()
                    .to_owned(),
            ));
        }
        Ok(None)
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
    Attr(AttrId),
    /// A `md:Model.*` header property, by local name.
    Header(String),
    /// A difference-model statement.
    Statement {
        subject: Mrid,
        class: Option<QualifiedName>,
        ns: String,
        local: String,
        forward: bool,
    },
}
