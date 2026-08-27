//! Language-agnostic intermediate representation of a CIM schema vintage.
//!
//! One [`Schema`] is built from the RDFS vocabularies of *all* profiles in a vintage
//! (EQ, SSH, SV, TP, ...). Classes and attributes are unioned across profiles and each
//! records the set of profiles it belongs to.
//!
//! The union is essential rather than incidental: in CGMES the *same* object (same mRID)
//! is described by several instance files — e.g. a `SynchronousMachine` carries its
//! parameters in EQ and its dispatch in SSH. A single Rust type per class must therefore
//! hold the union of all profile attributes, while serialization filters by profile.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

use crate::rdfs;

// ---------------------------------------------------------------------------
// Well-known IRIs
// ---------------------------------------------------------------------------

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const CIMS: &str = "http://iec.ch/TC57/1999/rdf-schema-extensions-19990926#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const DCAT: &str = "http://www.w3.org/ns/dcat#";
const DCTERMS: &str = "http://purl.org/dc/terms/";

fn rdf(local: &str) -> String {
    format!("{RDF}{local}")
}
fn rdfs(local: &str) -> String {
    format!("{RDFS}{local}")
}
fn cims(local: &str) -> String {
    format!("{CIMS}{local}")
}

// ---------------------------------------------------------------------------
// IR types
// ---------------------------------------------------------------------------

/// Profile membership as a bitmask over [`Schema::profiles`].
///
/// The same width as the runtime `cim_rs::schema::ProfileMask` it is emitted into, and it
/// has to stay that way: this is shifted by profile index, and Rust masks an over-wide shift
/// in release builds rather than failing, which would alias two profiles onto one bit in the
/// tables everything else is derived from.
pub type ProfileMask = u64;

/// How many profiles one vintage may hold, which is [`ProfileMask`]'s width.
pub const MAX_PROFILES: usize = ProfileMask::BITS as usize;

#[derive(Debug, Clone)]
pub struct Profile {
    /// Short keyword from `dcat:keyword`, e.g. `EQ`, `SSH`.
    pub keyword: String,
    /// `owl:versionIRI`, the value written as `md:Model.profile`, e.g.
    /// `http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0`.
    pub version_iri: String,
    pub title: String,
    /// Further IRIs that also denote this profile when read.
    pub aliases: Vec<String>,
    /// RDFS file this profile was parsed from.
    pub source_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name {
    /// Namespace IRI including the trailing `#`.
    pub ns: String,
    /// Local name (`ACLineSegment`, `ACLineSegment.r`).
    pub local: String,
}

impl Name {
    pub fn from_iri(iri: &str) -> Result<Self> {
        let (ns, local) = iri
            .rsplit_once('#')
            .with_context(|| format!("IRI without fragment: {iri}"))?;
        Ok(Name {
            ns: format!("{ns}#"),
            local: local.to_owned(),
        })
    }
    /// Class part of a `Class.attribute` local name.
    pub fn owner(&self) -> Option<&str> {
        self.local.split_once('.').map(|(c, _)| c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Multiplicity {
    /// `M:0..1`
    Optional,
    /// `M:1..1` or `M:1`
    Required,
    /// `M:0..n`
    Many,
    /// `M:1..n`
    ManyRequired,
    /// Anything else, e.g. `M:0..2`; treated as a bounded list.
    Bounded { min: u32, max: u32 },
}

impl Multiplicity {
    pub fn is_many(self) -> bool {
        matches!(
            self,
            Multiplicity::Many | Multiplicity::ManyRequired | Multiplicity::Bounded { .. }
        )
    }
    fn parse(s: &str) -> Multiplicity {
        match s {
            "0..1" => Multiplicity::Optional,
            "1..1" | "1" => Multiplicity::Required,
            "0..n" => Multiplicity::Many,
            "1..n" => Multiplicity::ManyRequired,
            other => {
                let (lo, hi) = other.split_once("..").unwrap_or((other, other));
                Multiplicity::Bounded {
                    min: lo.parse().unwrap_or(0),
                    max: hi.parse().unwrap_or(u32::MAX),
                }
            }
        }
    }
}

/// The eight CIM primitive types (IEC 61970-301 Annex, `UML#Primitive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    String,
    Integer,
    Float,
    Decimal,
    Boolean,
    Date,
    DateTime,
    MonthDay,
    Time,
    Duration,
    /// `URI`/`IRI`-typed primitives used by header profiles.
    Uri,
}

impl Primitive {
    fn from_local(local: &str) -> Option<Primitive> {
        Some(match local {
            "String" => Primitive::String,
            "Integer" => Primitive::Integer,
            "Float" => Primitive::Float,
            "Decimal" => Primitive::Decimal,
            "Boolean" => Primitive::Boolean,
            "Date" => Primitive::Date,
            "DateTime" => Primitive::DateTime,
            "MonthDay" => Primitive::MonthDay,
            "Time" => Primitive::Time,
            "Duration" => Primitive::Duration,
            "URI" | "IRI" | "AnyURI" => Primitive::Uri,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum AttrKind {
    /// A CIM primitive.
    Primitive(Primitive),
    /// A `CIMDatatype` — a unit-carrying value wrapper (`Resistance`, `Voltage`, ...).
    Datatype(Name),
    /// An enumeration, serialized as `rdf:resource` to `Enum.value`.
    Enumeration(Name),
    /// A `Compound` — a structured value with no identity of its own, serialized inline
    /// as a nested `rdf:parseType="Resource"` element rather than as a reference.
    Compound(Name),
    /// An association to another class, serialized as `rdf:resource` to the target mRID.
    Association {
        target: Name,
        /// Inverse role IRI (`cims:inverseRoleName`), if declared.
        inverse: Option<Name>,
        /// Profiles in which `cims:AssociationUsed` is `Yes`. Exactly one side of an
        /// association is serialized in a given profile; the other side is derived by
        /// inversion and must not be written.
        used_in: ProfileMask,
    },
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Name,
    /// Short name, i.e. the part after the dot.
    pub label: String,
    pub kind: AttrKind,
    pub multiplicity: Multiplicity,
    pub comment: Option<String>,
    pub profiles: ProfileMask,
    pub deprecated: bool,
    /// `cims:isFixed` — a constant value the schema pins for this attribute.
    pub fixed: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Class {
    pub name: Name,
    pub parent: Option<Name>,
    pub concrete: bool,
    pub comment: Option<String>,
    /// Attributes declared directly on this class (not inherited), sorted by local name.
    pub attributes: Vec<Attribute>,
    pub profiles: ProfileMask,
    /// Profiles whose vocabulary *defines* this class rather than merely referring to it.
    ///
    /// A profile that only adds attributes to an object introduced elsewhere marks the
    /// class with `cims:stereotype Description`. That is exactly the distinction
    /// IEC 61970-552 draws between `rdf:ID` (this file introduces the object) and
    /// `rdf:about` (this file updates an object defined elsewhere), so it decides how the
    /// writer identifies each object. The Steady State Hypothesis profile marks every
    /// class it touches; State Variables marks only `CsConverter` and `VsConverter`, which
    /// is why published SV files identify `SvPowerFlow` with `rdf:ID`.
    pub defined_in: ProfileMask,
    /// Profiles that declare this class `concrete`, i.e. may hold instances of it.
    pub concrete_in: ProfileMask,
    pub deprecated: bool,
    /// `cims:stereotype Compound` — a value type without identity, serialized inline.
    pub compound: bool,
}

#[derive(Debug, Clone)]
pub struct EnumType {
    pub name: Name,
    pub comment: Option<String>,
    pub values: Vec<EnumValue>,
    pub profiles: ProfileMask,
}

#[derive(Debug, Clone)]
pub struct EnumValue {
    pub name: Name,
    pub label: String,
    pub comment: Option<String>,
}

/// A `CIMDatatype`: a float/int/string value with a schema-fixed unit and multiplier.
#[derive(Debug, Clone)]
pub struct Datatype {
    pub name: Name,
    pub comment: Option<String>,
    /// Underlying primitive from the `.value` attribute.
    pub value: Primitive,
    /// Fixed `UnitSymbol` from `.unit`'s `cims:isFixed`, e.g. `ohm`.
    pub unit: Option<String>,
    /// Fixed `UnitMultiplier` from `.multiplier`'s `cims:isFixed`, e.g. `k`.
    pub multiplier: Option<String>,
    pub profiles: ProfileMask,
}

#[derive(Debug, Default)]
pub struct Schema {
    /// Vintage key, e.g. `cgmes3`.
    pub vintage: String,
    pub profiles: Vec<Profile>,
    pub classes: BTreeMap<Name, Class>,
    pub enums: BTreeMap<Name, EnumType>,
    pub datatypes: BTreeMap<Name, Datatype>,
    /// Every namespace IRI referenced by a class or attribute, with a stable prefix.
    pub namespaces: BTreeMap<String, String>,
}

impl Schema {
    /// Ancestors of `name`, nearest parent first.
    pub fn ancestors(&self, name: &Name) -> Vec<&Class> {
        let mut out = Vec::new();
        let mut cur = self.classes.get(name).and_then(|c| c.parent.as_ref());
        while let Some(p) = cur {
            match self.classes.get(p) {
                Some(c) => {
                    out.push(c);
                    cur = c.parent.as_ref();
                }
                None => break,
            }
        }
        out
    }

    /// `name` and every class that inherits from it, in schema order.
    ///
    /// Emitted into the tables so that "every `ACLineSegment`, subclasses included" is a
    /// walk of a handful of per-class buckets rather than a scan of the whole dataset.
    pub fn descendants(&self, name: &Name) -> Vec<&Name> {
        self.classes
            .keys()
            .filter(|k| *k == name || self.ancestors(k).iter().any(|a| &a.name == name))
            .collect()
    }

    /// All attributes visible on `name`: own attributes plus every inherited one,
    /// ordered root-first so that base-class fields come first in generated structs.
    pub fn all_attributes(&self, name: &Name) -> Vec<(&Class, &Attribute)> {
        let mut chain: Vec<&Class> = self.ancestors(name);
        chain.reverse();
        if let Some(c) = self.classes.get(name) {
            chain.push(c);
        }
        chain
            .into_iter()
            .flat_map(|c| c.attributes.iter().map(move |a| (c, a)))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Building the IR
// ---------------------------------------------------------------------------

/// A profile's RDFS file together with how to interpret it.
pub struct ProfileSource {
    pub path: std::path::PathBuf,
    pub spec: &'static crate::vintage::ProfileSpec,
}

pub fn build(vintage: &str, sources: &[ProfileSource]) -> Result<Schema> {
    if sources.len() > ProfileMask::BITS as usize {
        bail!(
            "{} profiles exceeds the {}-bit profile mask",
            sources.len(),
            ProfileMask::BITS
        );
    }

    let mut schema = Schema {
        vintage: vintage.to_owned(),
        ..Default::default()
    };

    // Pass 1: parse every profile document and register the profile itself.
    //
    // Several profiles may share one file — CGMES 2.4.15 keeps Equipment, Operation and
    // ShortCircuit in a single vocabulary — so documents are cached by path.
    let mut cache: BTreeMap<std::path::PathBuf, std::rc::Rc<rdfs::Document>> = BTreeMap::new();
    let mut docs: Vec<std::rc::Rc<rdfs::Document>> = Vec::new();
    for src in sources {
        let doc = match cache.get(&src.path) {
            Some(d) => d.clone(),
            None => {
                let d = std::rc::Rc::new(rdfs::parse_file(&src.path)?);
                cache.insert(src.path.clone(), d.clone());
                d
            }
        };
        let profile = extract_profile(&doc, src)?;
        schema.profiles.push(profile);
        docs.push(doc);
    }

    // Stereotypes claimed by any profile drawn from a given file. A profile that claims
    // none takes whatever its siblings leave, which is how the Equipment core is
    // separated from the Operation and ShortCircuit extensions sharing its vocabulary.
    let mut claimed: BTreeMap<&std::path::Path, BTreeSet<&str>> = BTreeMap::new();
    for src in sources {
        let e = claimed.entry(src.path.as_path()).or_default();
        for st in src.spec.stereotypes {
            e.insert(*st);
        }
    }

    // Pass 2: classify every description. Enumerations and datatypes must be known
    // before attributes can be typed, so classification happens across all documents
    // before attributes are attached.
    let mut enum_locals: BTreeSet<String> = BTreeSet::new();
    for doc in &docs {
        for d in &doc.descriptions {
            if stereotypes(d).any(|s| s == "enumeration")
                && let Ok(n) = Name::from_iri(&d.iri)
            {
                enum_locals.insert(n.local.clone());
            }
        }
    }

    let filters: Vec<StereotypeFilter> = sources
        .iter()
        .map(|src| StereotypeFilter {
            claims: src.spec.stereotypes,
            siblings_claim: claimed.get(src.path.as_path()).cloned().unwrap_or_default(),
        })
        .collect();

    // Checked rather than assumed, for the reason `ProfileId::mask` states at runtime: an
    // over-wide shift is masked in release builds, so profile 64 would take profile 0's bit
    // and merge the two everywhere provenance is consulted.
    if docs.len() > MAX_PROFILES {
        bail!(
            "vintage {} has {} profiles but ProfileMask is {} bits wide; widen \
             `ir::ProfileMask` and `cim_rs::schema::ProfileMask` together",
            vintage,
            docs.len(),
            MAX_PROFILES
        );
    }

    for (pi, doc) in docs.iter().enumerate() {
        let bit: ProfileMask = 1 << pi;
        collect_types(&mut schema, doc, bit, &enum_locals, &filters[pi])?;
    }
    for (pi, doc) in docs.iter().enumerate() {
        let bit: ProfileMask = 1 << pi;
        collect_attributes(&mut schema, doc, bit, &filters[pi])?;
    }

    finish(&mut schema);
    Ok(schema)
}

/// Decides whether an attribute belongs to a profile that shares its vocabulary file.
struct StereotypeFilter {
    /// Stereotypes this profile claims. Empty means "whatever siblings do not claim".
    claims: &'static [&'static str],
    /// Every stereotype claimed by any profile drawn from the same file.
    siblings_claim: BTreeSet<&'static str>,
}

impl StereotypeFilter {
    fn accepts(&self, d: &rdfs::Description) -> bool {
        if self.siblings_claim.is_empty() {
            return true;
        }
        let has = |name: &str| stereotypes(d).any(|s| s == name);
        if self.claims.is_empty() {
            // The residual profile: everything no sibling claimed.
            !self.siblings_claim.iter().any(|s| has(s))
        } else {
            self.claims.iter().any(|s| has(s))
        }
    }
}

fn stereotypes(d: &rdfs::Description) -> impl Iterator<Item = &str> {
    let key = cims("stereotype");
    d.props
        .iter()
        .filter(move |p| p.predicate == key)
        .map(|p| match &p.value {
            rdfs::Value::Resource(r) => r.rsplit('#').next().unwrap_or(r),
            rdfs::Value::Literal(l) => l.trim(),
        })
}

fn extract_profile(doc: &rdfs::Document, src: &ProfileSource) -> Result<Profile> {
    let ontology = doc.descriptions.iter().find(|d| {
        d.resources(&rdf("type"))
            .any(|t| t == format!("{OWL}Ontology"))
    });

    let file = src
        .path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();

    // The spec is authoritative; the ontology block fills in what it leaves blank, which
    // is how CGMES 3.0 vocabularies describe themselves.
    let from_ontology = |predicate: &str| {
        ontology
            .and_then(|o| o.resource(predicate))
            .map(str::to_owned)
    };
    let literal = |predicate: &str| {
        ontology
            .and_then(|o| o.literal(predicate))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };

    let version_iri = if src.spec.version_iri.is_empty() {
        from_ontology(&format!("{OWL}versionIRI")).unwrap_or_default()
    } else {
        src.spec.version_iri.to_owned()
    };
    if version_iri.is_empty() {
        bail!(
            "profile {} ({file}) has no version IRI: the vocabulary declares none and the \
             vintage does not state one",
            src.spec.keyword
        );
    }

    let keyword = literal(&format!("{DCAT}keyword"))
        .filter(|_| src.spec.stereotypes.is_empty())
        .unwrap_or_else(|| src.spec.keyword.to_owned());

    Ok(Profile {
        keyword,
        version_iri,
        title: literal(&format!("{DCTERMS}title")).unwrap_or_else(|| src.spec.title.to_owned()),
        aliases: src.spec.aliases.iter().map(|s| (*s).to_owned()).collect(),
        source_file: file,
    })
}

/// Register classes, enumerations and CIM datatypes found in one profile document.
fn collect_types(
    schema: &mut Schema,
    doc: &rdfs::Document,
    bit: ProfileMask,
    enum_locals: &BTreeSet<String>,
    filter: &StereotypeFilter,
) -> Result<()> {
    for d in &doc.descriptions {
        let is_class = d.resources(&rdf("type")).any(|t| t == rdfs("Class"));
        if !is_class {
            continue;
        }
        let Ok(name) = Name::from_iri(&d.iri) else {
            continue;
        };
        // Package/category markers are not model types.
        if name.local.starts_with("Package_") {
            continue;
        }

        let stereos: Vec<&str> = stereotypes(d).collect();
        let comment = comment_of(d);
        let deprecated = stereos.contains(&"deprecated");

        if stereos.contains(&"enumeration") || enum_locals.contains(&name.local) {
            let e = schema
                .enums
                .entry(name.clone())
                .or_insert_with(|| EnumType {
                    name: name.clone(),
                    comment: comment.clone(),
                    values: Vec::new(),
                    profiles: 0,
                });
            e.profiles |= bit;
            if e.comment.is_none() {
                e.comment = comment;
            }
        } else if stereos.contains(&"CIMDatatype") || stereos.contains(&"Primitive") {
            let t = schema
                .datatypes
                .entry(name.clone())
                .or_insert_with(|| Datatype {
                    name: name.clone(),
                    comment: comment.clone(),
                    value: Primitive::Float,
                    unit: None,
                    multiplier: None,
                    profiles: 0,
                });
            t.profiles |= bit;
            if t.comment.is_none() {
                t.comment = comment;
            }
        } else {
            // Only classes are filtered: enumerations and datatypes are shared by every
            // profile drawn from the file, since an enumeration used by a ShortCircuit
            // attribute is just as available there as in the core.
            if !filter.accepts(d) {
                continue;
            }
            let parent = d
                .resource(&rdfs("subClassOf"))
                .and_then(|p| Name::from_iri(p).ok());
            let c = schema.classes.entry(name.clone()).or_insert_with(|| Class {
                name: name.clone(),
                parent: parent.clone(),
                concrete: false,
                comment: comment.clone(),
                attributes: Vec::new(),
                profiles: 0,
                defined_in: 0,
                concrete_in: 0,
                deprecated,
                compound: false,
            });
            let concrete = stereos.contains(&"concrete");
            c.profiles |= bit;
            c.concrete |= concrete;
            c.compound |= stereos.contains(&"Compound");
            if concrete {
                c.concrete_in |= bit;
            }
            // `Description` marks a class this profile only *refers* to. Everything else
            // it declares, it introduces — which is what `rdf:ID` means on export.
            //
            // Deliberately not also requiring `concrete` here: published models do
            // instantiate classes their own profile declares abstract, and SmallGrid's
            // Equipment file writes a bare `<cim:Switch rdf:ID="…">` that the Equipment
            // vocabulary has no concrete declaration for. Refusing to call that a
            // definition would rewrite it as `rdf:about` and change the file.
            if !stereos.contains(&"Description") {
                c.defined_in |= bit;
            }
            c.deprecated |= deprecated;
            if c.parent.is_none() {
                c.parent = parent;
            }
            if c.comment.is_none() {
                c.comment = comment;
            }
        }
    }

    // Enumeration values are typed by their enumeration class: `rdf:type` points at it.
    //
    // The `enum` stereotype is *not* a reliable signal — CGMES 2.4.15 vocabularies omit
    // it entirely and rely on `rdf:type` alone — so the type edge is what is followed.
    for d in &doc.descriptions {
        let Ok(name) = Name::from_iri(&d.iri) else {
            continue;
        };
        let Some(owner) = name.owner() else { continue };

        let typed_as_enum = d.resources(&rdf("type")).any(|t| {
            Name::from_iri(t).is_ok_and(|tn| {
                tn.local == owner
                    && (schema.enums.contains_key(&tn)
                        || schema.enums.keys().any(|k| k.local == tn.local))
            })
        });
        if !typed_as_enum {
            continue;
        }

        let owner_name = Name {
            ns: name.ns.clone(),
            local: owner.to_owned(),
        };
        // Values may live in the base namespace while the enum is declared elsewhere.
        let key = if schema.enums.contains_key(&owner_name) {
            owner_name
        } else {
            match schema.enums.keys().find(|k| k.local == owner) {
                Some(k) => k.clone(),
                None => continue,
            }
        };
        let label = d
            .literal(&rdfs("label"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|| name.local.rsplit('.').next().unwrap_or("").to_owned());
        let e = schema.enums.get_mut(&key).expect("enum present");
        if !e.values.iter().any(|v| v.name == name) {
            e.values.push(EnumValue {
                name,
                label,
                comment: comment_of(d),
            });
        }
    }

    Ok(())
}

/// Attach attributes and associations to their owning class or datatype.
fn collect_attributes(
    schema: &mut Schema,
    doc: &rdfs::Document,
    bit: ProfileMask,
    filter: &StereotypeFilter,
) -> Result<()> {
    for d in &doc.descriptions {
        let is_prop = d.resources(&rdf("type")).any(|t| t == rdf("Property"));
        if !is_prop {
            continue;
        }
        if !filter.accepts(d) {
            continue;
        }
        let Ok(name) = Name::from_iri(&d.iri) else {
            continue;
        };
        let Some(domain_iri) = d.resource(&rdfs("domain")) else {
            continue;
        };
        let Ok(domain) = Name::from_iri(domain_iri) else {
            continue;
        };

        let label = d
            .literal(&rdfs("label"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|| name.local.rsplit('.').next().unwrap_or("").to_owned());
        let multiplicity = d
            .resource(&cims("multiplicity"))
            .and_then(|m| m.rsplit("#M:").next().map(Multiplicity::parse))
            .unwrap_or(Multiplicity::Optional);
        let fixed = d.literal(&cims("isFixed")).map(|s| s.trim().to_owned());
        let comment = comment_of(d);
        let deprecated = stereotypes(d).any(|s| s == "deprecated");

        // A CIMDatatype's own `.value` / `.unit` / `.multiplier` properties describe the
        // datatype rather than adding a field to a class.
        if schema.datatypes.contains_key(&domain) {
            apply_datatype_part(schema, &domain, &label, d, fixed.as_deref());
            continue;
        }

        let kind = match classify_attribute(schema, d, bit) {
            Some(k) => k,
            None => continue,
        };

        let Some(class) = schema.classes.get_mut(&domain) else {
            continue;
        };
        match class.attributes.iter_mut().find(|a| a.name == name) {
            Some(existing) => {
                existing.profiles |= bit;
                // An association counts as "used" if any profile serializes it.
                if let (
                    AttrKind::Association {
                        used_in: existing_used,
                        ..
                    },
                    AttrKind::Association {
                        used_in: new_used, ..
                    },
                ) = (&mut existing.kind, &kind)
                {
                    *existing_used |= *new_used;
                }
                if existing.comment.is_none() {
                    existing.comment = comment;
                }
            }
            None => class.attributes.push(Attribute {
                name,
                label,
                kind,
                multiplicity,
                comment,
                profiles: bit,
                deprecated,
                fixed,
            }),
        }
    }
    Ok(())
}

fn apply_datatype_part(
    schema: &mut Schema,
    domain: &Name,
    label: &str,
    d: &rdfs::Description,
    fixed: Option<&str>,
) {
    let value_prim = d
        .resource(&cims("dataType"))
        .and_then(|t| Name::from_iri(t).ok())
        .and_then(|n| Primitive::from_local(&n.local));
    let Some(dt) = schema.datatypes.get_mut(domain) else {
        return;
    };
    match label {
        "value" => {
            if let Some(p) = value_prim {
                dt.value = p;
            }
        }
        "unit" => dt.unit = fixed.map(str::to_owned).or(dt.unit.take()),
        "multiplier" => dt.multiplier = fixed.map(str::to_owned).or(dt.multiplier.take()),
        _ => {}
    }
}

fn classify_attribute(
    schema: &Schema,
    d: &rdfs::Description,
    bit: ProfileMask,
) -> Option<AttrKind> {
    // Associations declare `rdfs:range` pointing at a class.
    if let Some(range_iri) = d.resource(&rdfs("range")) {
        let range = Name::from_iri(range_iri).ok()?;
        if schema.enums.contains_key(&range) || schema.enums.keys().any(|k| k.local == range.local)
        {
            let key = schema
                .enums
                .keys()
                .find(|k| k.local == range.local)
                .cloned()
                .unwrap_or(range);
            return Some(AttrKind::Enumeration(key));
        }
        if schema.datatypes.contains_key(&range) {
            return Some(AttrKind::Datatype(range));
        }
        // A compound has no identity, so it is never a reference: it is written inline as
        // a nested `rdf:parseType="Resource"` element (IEC 61970-552).
        if schema.classes.get(&range).is_some_and(|c| c.compound) {
            return Some(AttrKind::Compound(range));
        }
        let used = d
            .literal(&cims("AssociationUsed"))
            .map(|v| v.trim().eq_ignore_ascii_case("yes"))
            // Properties without the flag (plain compositions) are serialized.
            .unwrap_or(true);
        return Some(AttrKind::Association {
            target: range,
            inverse: d
                .resource(&cims("inverseRoleName"))
                .and_then(|i| Name::from_iri(i).ok()),
            used_in: if used { bit } else { 0 },
        });
    }

    // Plain attributes declare `cims:dataType`: a primitive, a CIMDatatype, an
    // enumeration, or — since a compound is a value rather than a reference — a compound.
    let dt_iri = d.resource(&cims("dataType"))?;
    let dt = Name::from_iri(dt_iri).ok()?;
    if let Some(p) = Primitive::from_local(&dt.local) {
        return Some(AttrKind::Primitive(p));
    }
    if schema.datatypes.contains_key(&dt) {
        return Some(AttrKind::Datatype(dt));
    }
    if schema.enums.contains_key(&dt) {
        return Some(AttrKind::Enumeration(dt));
    }
    // `Location.mainAddress` names `StreetAddress` through `cims:dataType`, not
    // `rdfs:range`, because a compound is held rather than pointed at.
    if schema.classes.get(&dt).is_some_and(|c| c.compound) {
        return Some(AttrKind::Compound(dt));
    }
    // Unknown datatype: fall back to a string-typed value rather than dropping data.
    Some(AttrKind::Primitive(Primitive::String))
}

fn comment_of(d: &rdfs::Description) -> Option<String> {
    d.literal(&rdfs("comment"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// A profile that only annotates a class hierarchy does not define its base class either.
///
/// The `Description` stereotype marks a class a profile refers to rather than introduces,
/// but the published vocabularies apply it unevenly. Steady State Hypothesis marks all 45
/// of the classes it touches — except `Equipment`, which it declares concrete so that
/// `<cim:Equipment rdf:about="…">` may carry `inService` for equipment whose own class it
/// does not declare. Taken at face value that would make SSH the file that *introduces*
/// every such object, and every published SSH file says otherwise.
///
/// The signal is in the hierarchy: a profile that marks `Switch` and `ACLineSegment` as
/// descriptions is annotating equipment, so it is not introducing `Equipment` either.
fn propagate_description(schema: &mut Schema) {
    let mut annotating: BTreeMap<Name, ProfileMask> = BTreeMap::new();
    for class in schema.classes.values() {
        // Profiles in which this class is a description rather than a definition.
        let described = class.profiles & !class.defined_in;
        if described == 0 {
            continue;
        }
        for ancestor in schema.ancestors(&class.name) {
            *annotating.entry(ancestor.name.clone()).or_default() |= described;
        }
    }
    for (name, mask) in annotating {
        if let Some(c) = schema.classes.get_mut(&name) {
            c.defined_in &= !mask;
        }
    }
}

/// Sort for determinism, assign namespace prefixes, and drop dangling parents.
fn finish(schema: &mut Schema) {
    propagate_description(schema);
    let known: BTreeSet<Name> = schema.classes.keys().cloned().collect();
    for class in schema.classes.values_mut() {
        if let Some(p) = &class.parent
            && !known.contains(p)
        {
            class.parent = None;
        }
        class.attributes.sort_by(|a, b| a.name.cmp(&b.name));
    }
    for e in schema.enums.values_mut() {
        e.values.sort_by(|a, b| a.name.cmp(&b.name));
    }

    let mut namespaces: BTreeSet<String> = BTreeSet::new();
    for c in schema.classes.values() {
        namespaces.insert(c.name.ns.clone());
        for a in &c.attributes {
            namespaces.insert(a.name.ns.clone());
        }
    }
    for e in schema.enums.values() {
        namespaces.insert(e.name.ns.clone());
    }
    for (i, ns) in namespaces.into_iter().enumerate() {
        let prefix = well_known_prefix(&ns).unwrap_or_else(|| format!("ns{i}"));
        schema.namespaces.insert(ns, prefix);
    }
}

fn well_known_prefix(ns: &str) -> Option<String> {
    Some(
        match ns {
            "http://iec.ch/TC57/CIM100#" => "cim",
            "http://iec.ch/TC57/CIM100-European#" => "eu",
            "http://iec.ch/TC57/2013/CIM-schema-cim16#" => "cim",
            "http://entsoe.eu/CIM/SchemaExtension/3/1#" => "entsoe",
            "http://iec.ch/TC57/61970-552/ModelDescription/1#" => "md",
            "http://iec.ch/TC57/61970-552/DifferenceModel/1#" => "dm",
            // The difference-model vocabulary declares `rdf:Statements` and its fields, so
            // the RDF namespace ends up in the schema's own table. It has exactly one
            // conventional prefix, and a generated `ns4` for it would be a name no reader
            // of a CIM document recognises.
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#" => "rdf",
            _ => return None,
        }
        .to_owned(),
    )
}

#[cfg(test)]
mod bound_tests {
    use super::*;

    /// The generator's profile bound, pinned where it can be seen next to the shift.
    ///
    /// `ir::ProfileMask` and `cim_rs::schema::ProfileMask` have to move together: the
    /// generator shifts by profile index into the first and emits the result into the
    /// second. `xtask` deliberately does not depend on the crate it generates, so the two
    /// cannot be compared by the compiler — this states the number both are expected to be.
    #[test]
    fn the_profile_bound_matches_the_runtime_mask() {
        assert_eq!(MAX_PROFILES, 64);
        for v in crate::vintage::VINTAGES {
            assert!(
                v.profiles.len() <= MAX_PROFILES,
                "{} declares {} profiles",
                v.key,
                v.profiles.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplicity_parsing() {
        assert_eq!(Multiplicity::parse("0..1"), Multiplicity::Optional);
        assert_eq!(Multiplicity::parse("1..1"), Multiplicity::Required);
        assert_eq!(Multiplicity::parse("1"), Multiplicity::Required);
        assert_eq!(Multiplicity::parse("0..n"), Multiplicity::Many);
        assert_eq!(Multiplicity::parse("1..n"), Multiplicity::ManyRequired);
        assert!(Multiplicity::parse("0..2").is_many());
    }

    #[test]
    fn name_splits_iri() {
        let n = Name::from_iri("http://iec.ch/TC57/CIM100#ACLineSegment.r").unwrap();
        assert_eq!(n.ns, "http://iec.ch/TC57/CIM100#");
        assert_eq!(n.local, "ACLineSegment.r");
        assert_eq!(n.owner(), Some("ACLineSegment"));
    }
}
