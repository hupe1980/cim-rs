//! Writer for CIM/XML instance documents (IEC 61970-552).
//!
//! Output is deterministic: objects are emitted in ascending mRID order and attributes
//! in schema order, so re-writing an unchanged model yields byte-identical output and
//! diffs between model versions stay readable.

use std::io::Write;

use crate::dataset::Dataset;
use crate::error::{Error, Result};
use crate::header::{DM_NS, MD_NS, ModelHeader, ModelKind};
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::{AttrKind, ClassId, ProfileId, ProfileMask, Schema};
use crate::value::Value;

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// How to identify objects in the output.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum IdStyle {
    /// `rdf:ID="_<uuid>"` — the object is *defined* by this file.
    ///
    /// IEC 61970-552 uses this for objects a file introduces.
    #[default]
    RdfId,
    /// `rdf:about="#_<uuid>"` — the object is defined elsewhere and this file adds to it.
    ///
    /// This is what Steady State Hypothesis and State Variables files use when they
    /// describe equipment defined in an Equipment file.
    RdfAbout,
}

/// Options controlling serialization.
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
    /// Header to emit. Without one, no `md:FullModel` element is written.
    pub header: Option<ModelHeader>,
    /// Emit the XML declaration.
    pub xml_declaration: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            profiles: 0,
            id_style: IdStyle::RdfId,
            pretty: true,
            header: None,
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

    pub fn with_header(mut self, header: ModelHeader) -> Self {
        self.header = Some(header);
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
    let mut w = Writer {
        out,
        schema,
        pretty: options.pretty,
    };

    // Deterministic order: sort by mRID so output does not depend on load order.
    let mut list: Vec<&Object> = objects.filter_map(|id| dataset.get(id)).collect();
    list.sort_by(|a, b| a.mrid().cmp(b.mrid()));

    // Only namespaces actually used need to be declared, but declaring the schema's
    // full set keeps output stable across models and matches published files.
    w.start_document(options)?;
    if let Some(h) = &options.header {
        w.header(h)?;
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
    header: Option<ModelHeader>,
    options: &WriteOptions,
) -> Result<()> {
    write_profiles(dataset, profile.mask(), out, header, options)
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
    header: Option<ModelHeader>,
    options: &WriteOptions,
) -> Result<()> {
    let schema = dataset.schema();
    let mut opts = options.clone();
    opts.profiles = profiles;
    opts.header = header;

    let ids: Vec<_> = dataset
        .iter()
        .filter(|(_, o)| object_has_content_in(schema, o, profiles))
        .map(|(id, _)| id)
        .collect();
    write_objects(dataset, ids.into_iter(), out, &opts)
}

/// Whether a stored value belongs in an instance file serving `profiles`.
///
/// See [`Schema::effective_profiles`] for the rule. A zero mask means "no filtering".
#[inline]
fn slot_belongs_to(schema: &Schema, slot: &crate::object::Slot, profiles: ProfileMask) -> bool {
    profiles == 0 || schema.effective_profiles(slot.attr, slot.profiles) & profiles != 0
}

/// Whether an object would serialize any attribute in `profiles`.
fn object_has_content_in(schema: &Schema, obj: &Object, profiles: ProfileMask) -> bool {
    obj.slots()
        .iter()
        .any(|s| slot_belongs_to(schema, s, profiles))
}

struct Writer<W: Write> {
    out: W,
    schema: &'static Schema,
    pretty: bool,
}

impl<W: Write> Writer<W> {
    fn start_document(&mut self, options: &WriteOptions) -> Result<()> {
        if options.xml_declaration {
            self.out
                .write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
        }
        self.out.write_all(b"<rdf:RDF")?;
        write!(self.out, " xmlns:rdf=\"{RDF_NS}\"").map_err(Error::Io)?;
        for ns in self.schema.namespaces {
            write!(self.out, " xmlns:{}=\"{}\"", ns.prefix, ns.iri).map_err(Error::Io)?;
        }
        if options.header.is_some() {
            write!(self.out, " xmlns:md=\"{MD_NS}\"").map_err(Error::Io)?;
            if matches!(
                options.header.as_ref().map(|h| h.kind),
                Some(ModelKind::Difference)
            ) {
                write!(self.out, " xmlns:dm=\"{DM_NS}\"").map_err(Error::Io)?;
            }
        }
        self.out.write_all(b">\n")?;
        Ok(())
    }

    fn end_document(&mut self) -> Result<()> {
        self.out.write_all(b"</rdf:RDF>\n")?;
        self.out.flush()?;
        Ok(())
    }

    fn header(&mut self, h: &ModelHeader) -> Result<()> {
        let (prefix, elem) = match h.kind {
            ModelKind::Full => ("md", "FullModel"),
            ModelKind::Difference => ("dm", "DifferenceModel"),
        };
        let about =
            h.id.as_ref()
                .map(Mrid::to_urn)
                .unwrap_or_else(|| "urn:uuid:00000000-0000-0000-0000-000000000000".to_owned());
        self.indent(1)?;
        write!(self.out, "<{prefix}:{elem} rdf:about=\"").map_err(Error::Io)?;
        self.escaped(&about)?;
        self.out.write_all(b"\">\n")?;

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
        for (k, v) in &h.extra {
            self.header_text(Some(v), k)?;
        }

        self.indent(1)?;
        writeln!(self.out, "</{prefix}:{elem}>").map_err(Error::Io)?;
        Ok(())
    }

    fn header_text(&mut self, value: Option<&str>, local: &str) -> Result<()> {
        let Some(v) = value else { return Ok(()) };
        self.indent(2)?;
        write!(self.out, "<md:{local}>").map_err(Error::Io)?;
        self.escaped(v)?;
        writeln!(self.out, "</md:{local}>").map_err(Error::Io)?;
        Ok(())
    }

    fn header_resource(&mut self, value: &str, local: &str) -> Result<()> {
        self.indent(2)?;
        write!(self.out, "<md:{local} rdf:resource=\"").map_err(Error::Io)?;
        self.escaped(value)?;
        self.out.write_all(b"\"/>\n")?;
        Ok(())
    }

    fn object(&mut self, obj: &Object, options: &WriteOptions) -> Result<()> {
        let class = self.schema.class(obj.class());
        let prefix = self.schema.namespace(class.ns).prefix;

        // Collect the attributes this profile serializes, in schema order.
        let mut written: Vec<(&'static str, &'static str, &Value)> = Vec::new();
        for attr_id in class.all_attrs {
            let def = self.schema.attr(*attr_id);
            let attr_prefix = self.schema.namespace(def.ns).prefix;
            for slot in obj.get_all(*attr_id) {
                if !slot_belongs_to(self.schema, slot, options.profiles) {
                    continue;
                }
                written.push((attr_prefix, def.name, &slot.value));
            }
        }

        self.indent(1)?;
        write!(self.out, "<{}:{}", prefix, class.name).map_err(Error::Io)?;
        match options.id_style {
            IdStyle::RdfId => {
                self.out.write_all(b" rdf:ID=\"")?;
                self.escaped(&obj.mrid().to_rdf_id())?;
            }
            IdStyle::RdfAbout => {
                self.out.write_all(b" rdf:about=\"")?;
                self.escaped(&obj.mrid().to_rdf_resource())?;
            }
        }
        if written.is_empty() {
            self.out.write_all(b"\"/>\n")?;
            return Ok(());
        }
        self.out.write_all(b"\">\n")?;

        for (attr_prefix, name, value) in written {
            self.indent(2)?;
            match value {
                Value::Reference(m) => {
                    write!(self.out, "<{attr_prefix}:{name} rdf:resource=\"").map_err(Error::Io)?;
                    self.escaped(&m.to_rdf_resource())?;
                    self.out.write_all(b"\"/>\n")?;
                }
                Value::Enum(e) => {
                    let ev = self.schema.enum_value(*e);
                    let iri = format!("{}{}", self.schema.namespace(ev.ns).iri, ev.name);
                    write!(self.out, "<{attr_prefix}:{name} rdf:resource=\"").map_err(Error::Io)?;
                    self.escaped(&iri)?;
                    self.out.write_all(b"\"/>\n")?;
                }
                other => {
                    write!(self.out, "<{attr_prefix}:{name}>").map_err(Error::Io)?;
                    if let Some(text) = other.to_lexical() {
                        self.escaped(&text)?;
                    }
                    writeln!(self.out, "</{attr_prefix}:{name}>").map_err(Error::Io)?;
                }
            }
        }

        self.indent(1)?;
        writeln!(self.out, "</{}:{}>", prefix, class.name).map_err(Error::Io)?;
        Ok(())
    }

    fn indent(&mut self, level: usize) -> Result<()> {
        if self.pretty {
            for _ in 0..level {
                self.out.write_all(b"  ")?;
            }
        }
        Ok(())
    }

    /// Escape the five XML metacharacters. Attribute and text content share this path
    /// so that a value can never break out of its context.
    fn escaped(&mut self, s: &str) -> Result<()> {
        let mut start = 0;
        let bytes = s.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            let rep: &[u8] = match b {
                b'&' => b"&amp;",
                b'<' => b"&lt;",
                b'>' => b"&gt;",
                b'"' => b"&quot;",
                b'\'' => b"&apos;",
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

/// Choose the identification style a profile should use.
///
/// Equipment and boundary files define objects (`rdf:ID`); the state profiles describe
/// objects defined elsewhere (`rdf:about`). Callers can override via [`WriteOptions`].
pub fn conventional_id_style(schema: &Schema, profile: ProfileId) -> IdStyle {
    match schema.profile(profile).keyword {
        "SSH" | "SV" | "TP" | "DY" => IdStyle::RdfAbout,
        _ => IdStyle::RdfId,
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
        .filter(move |(_, c)| c.concrete && c.profiles & mask != 0)
        .map(|(i, _)| ClassId(i as u16))
}

/// Whether an attribute would be written for `profile` — exposed for tooling.
pub fn is_written_in(schema: &Schema, attr: crate::schema::AttrId, profile: ProfileId) -> bool {
    let def = schema.attr(attr);
    def.is_serialized_in(profile)
        && !matches!(def.kind, AttrKind::Association { inverse: None, .. } if false)
}
