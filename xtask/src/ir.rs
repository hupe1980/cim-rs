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
use std::path::Path;

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
pub type ProfileMask = u32;

#[derive(Debug, Clone)]
pub struct Profile {
    /// Short keyword from `dcat:keyword`, e.g. `EQ`, `SSH`.
    pub keyword: String,
    /// `owl:versionIRI`, the value written as `md:Model.profile`, e.g.
    /// `http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0`.
    pub version_iri: String,
    pub title: String,
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
    pub deprecated: bool,
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

/// A profile's RDFS file paired with the keyword to use if the file omits `dcat:keyword`.
pub struct ProfileSource {
    pub path: std::path::PathBuf,
    pub fallback_keyword: String,
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
    let mut docs = Vec::new();
    for src in sources {
        let doc = rdfs::parse_file(&src.path)?;
        let profile = extract_profile(&doc, src)?;
        schema.profiles.push(profile);
        docs.push(doc);
    }

    // Pass 2: classify every description. Enumerations and datatypes must be known
    // before attributes can be typed, so classification happens across all documents
    // before attributes are attached.
    let mut enum_locals: BTreeSet<String> = BTreeSet::new();
    for doc in &docs {
        for d in &doc.descriptions {
            if stereotypes(d).any(|s| s == "enumeration") {
                if let Ok(n) = Name::from_iri(&d.iri) {
                    enum_locals.insert(n.local.clone());
                }
            }
        }
    }

    for (pi, doc) in docs.iter().enumerate() {
        let bit: ProfileMask = 1 << pi;
        collect_types(&mut schema, doc, bit, &enum_locals)?;
    }
    for (pi, doc) in docs.iter().enumerate() {
        let bit: ProfileMask = 1 << pi;
        collect_attributes(&mut schema, doc, bit)?;
    }

    finish(&mut schema);
    Ok(schema)
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

    let Some(o) = ontology else {
        return Ok(Profile {
            keyword: src.fallback_keyword.clone(),
            version_iri: String::new(),
            title: src.fallback_keyword.clone(),
            source_file: file,
        });
    };

    Ok(Profile {
        keyword: o
            .literal(&format!("{DCAT}keyword"))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| src.fallback_keyword.clone()),
        version_iri: o
            .resource(&format!("{OWL}versionIRI"))
            .unwrap_or_default()
            .to_owned(),
        title: o
            .literal(&format!("{DCTERMS}title"))
            .unwrap_or(&src.fallback_keyword)
            .trim()
            .to_owned(),
        source_file: file,
    })
}

/// Register classes, enumerations and CIM datatypes found in one profile document.
fn collect_types(
    schema: &mut Schema,
    doc: &rdfs::Document,
    bit: ProfileMask,
    enum_locals: &BTreeSet<String>,
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
                deprecated,
            });
            c.profiles |= bit;
            c.concrete |= stereos.contains(&"concrete");
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
    for d in &doc.descriptions {
        let Ok(name) = Name::from_iri(&d.iri) else {
            continue;
        };
        let Some(owner) = name.owner() else { continue };
        if !stereotypes(d).any(|s| s == "enum") {
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
fn collect_attributes(schema: &mut Schema, doc: &rdfs::Document, bit: ProfileMask) -> Result<()> {
    for d in &doc.descriptions {
        let is_prop = d.resources(&rdf("type")).any(|t| t == rdf("Property"));
        if !is_prop {
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

    // Plain attributes declare `cims:dataType`, either a primitive or a CIMDatatype.
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
    // Unknown datatype: fall back to a string-typed value rather than dropping data.
    Some(AttrKind::Primitive(Primitive::String))
}

fn comment_of(d: &rdfs::Description) -> Option<String> {
    d.literal(&rdfs("comment"))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Sort for determinism, assign namespace prefixes, and drop dangling parents.
fn finish(schema: &mut Schema) {
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
            _ => return None,
        }
        .to_owned(),
    )
}

pub fn discover_profiles(dir: &Path, pattern_strip: &[&str]) -> Result<Vec<ProfileSource>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("listing RDFS directory {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rdf") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let mut keyword = stem.clone();
        for p in pattern_strip {
            keyword = keyword.replace(p, "");
        }
        out.push(ProfileSource {
            path,
            fallback_keyword: keyword,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
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
