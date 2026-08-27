//! Runtime schema metadata.
//!
//! The generated code under [`crate::cgmes3`] fills in the static tables declared here.
//! Everything else in the crate — reader, writer, validator, typed views — is written
//! against this interface, so a new CIM vintage is a regeneration rather than a rewrite.

use std::fmt;

/// Set of profiles, as a bitmask over [`Schema::profiles`].
///
/// 64 bits rather than 32 because a profile set is not fixed at the eleven CGMES 3.0
/// vocabularies. ENTSO-E publishes eighteen Network Code profiles in exactly the same
/// RDFS form, and the generator treats a profile as data precisely so that those and
/// bespoke ones can be added; twenty-nine published profiles leave no room for the
/// custom profiles the design promises to accept.
pub type ProfileMask = u64;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u16);

        impl $name {
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

id_type!(
    /// Index of a class in [`Schema::classes`].
    ClassId
);
id_type!(
    /// Index of an attribute or association in [`Schema::attributes`].
    AttrId
);
id_type!(
    /// Index of an enumeration in [`Schema::enums`].
    EnumId
);
id_type!(
    /// Index of an enumeration literal in [`Schema::enum_values`].
    EnumValueId
);
id_type!(
    /// Index of a CIM datatype in [`Schema::datatypes`].
    DatatypeId
);
id_type!(
    /// Index of a profile in [`Schema::profiles`].
    ProfileId
);
id_type!(
    /// Index of a namespace in [`Schema::namespaces`].
    NsId
);

impl ProfileId {
    /// How many profiles one schema may hold, which is [`ProfileMask`]'s width.
    pub const MAX_PROFILES: usize = ProfileMask::BITS as usize;

    /// This profile as a single-bit mask.
    ///
    /// # Panics
    ///
    /// If the profile index is at or beyond [`ProfileId::MAX_PROFILES`]. The alternative is
    /// worse and is what a bare shift gives: Rust masks an over-wide shift in release
    /// builds, so profile 64 would silently *alias* profile 0 — every filtering decision in
    /// the writer, the differ and the RDF export would then quietly attribute one profile's
    /// data to another. A schema this large is a generator defect rather than a caller's
    /// mistake, and `schemas_fit_the_profile_mask` in `tests/schema.rs` makes it a test
    /// failure before it can ever be a panic.
    #[inline]
    pub const fn mask(self) -> ProfileMask {
        assert!(
            (self.0 as usize) < ProfileId::MAX_PROFILES,
            "profile index exceeds the width of ProfileMask; widen the type"
        );
        1 << self.0
    }
}

/// A CIM primitive type (IEC 61970-301).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    Uri,
}

impl Primitive {
    /// The XML Schema datatype IRI this primitive's values carry in RDF.
    ///
    /// CIM/XML exchanges no datatype information — every value is element text — so this
    /// mapping is the only thing that can supply one, and it has to agree with the shapes
    /// a consumer will validate against. It is taken from ENTSO-E's own published SHACL:
    /// of the 3,137 `sh:datatype` constraints in the CGMES 3.0 shapes, 2,871 name
    /// `xsd:float`, which is why CIM `Float` maps to `xsd:float` and not to the
    /// `xsd:double` a 64-bit representation would suggest.
    ///
    /// [`Primitive::String`] returns the empty string: RDF 1.1 gives a literal with no
    /// datatype the type `xsd:string` already, so writing one would be redundant.
    pub const fn xsd_datatype(self) -> &'static str {
        match self {
            Primitive::String => "",
            Primitive::Integer => "http://www.w3.org/2001/XMLSchema#integer",
            Primitive::Float => "http://www.w3.org/2001/XMLSchema#float",
            Primitive::Decimal => "http://www.w3.org/2001/XMLSchema#decimal",
            Primitive::Boolean => "http://www.w3.org/2001/XMLSchema#boolean",
            Primitive::Date => "http://www.w3.org/2001/XMLSchema#date",
            Primitive::DateTime => "http://www.w3.org/2001/XMLSchema#dateTime",
            Primitive::MonthDay => "http://www.w3.org/2001/XMLSchema#gMonthDay",
            Primitive::Time => "http://www.w3.org/2001/XMLSchema#time",
            Primitive::Duration => "http://www.w3.org/2001/XMLSchema#duration",
            Primitive::Uri => "http://www.w3.org/2001/XMLSchema#anyURI",
        }
    }
}

/// Attribute cardinality as declared by `cims:multiplicity`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mult {
    /// `0..1`
    Optional,
    /// `1..1`
    Required,
    /// `0..n`
    Many,
    /// `1..n`
    ManyRequired,
    /// A bounded range such as `0..2`.
    Bounded { min: u32, max: u32 },
}

impl Mult {
    #[inline]
    pub const fn is_many(self) -> bool {
        matches!(self, Mult::Many | Mult::ManyRequired | Mult::Bounded { .. })
    }
    #[inline]
    pub const fn is_required(self) -> bool {
        match self {
            Mult::Required | Mult::ManyRequired => true,
            Mult::Bounded { min, .. } => min > 0,
            Mult::Optional | Mult::Many => false,
        }
    }
    #[inline]
    pub const fn min(self) -> u32 {
        match self {
            Mult::Optional | Mult::Many => 0,
            Mult::Required | Mult::ManyRequired => 1,
            Mult::Bounded { min, .. } => min,
        }
    }
    #[inline]
    pub const fn max(self) -> u32 {
        match self {
            Mult::Optional | Mult::Required => 1,
            Mult::Many | Mult::ManyRequired => u32::MAX,
            Mult::Bounded { max, .. } => max,
        }
    }
}

/// What an attribute holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttrKind {
    /// A plain primitive value.
    Primitive(Primitive),
    /// A unit-carrying CIM datatype such as `Resistance`; serialized as its primitive.
    Datatype(DatatypeId),
    /// An enumeration; serialized as `rdf:resource` pointing at `Enum.literal`.
    Enumeration(EnumId),
    /// A structured value with no identity of its own (`cims:stereotype Compound`).
    ///
    /// IEC 61970-552 gives compounds no `rdf:ID`, so they are written *inside* the
    /// property element as `rdf:parseType="Resource"` rather than referenced. The
    /// [`ClassId`] names the compound's type, whose attributes describe its fields.
    Compound(ClassId),
    /// A reference to another object; serialized as `rdf:resource` to its mRID.
    Association {
        target: ClassId,
        /// The opposite role, when the schema declares `cims:inverseRoleName`.
        inverse: Option<AttrId>,
    },
}

#[derive(Debug)]
pub struct NamespaceDef {
    /// Namespace IRI including the trailing `#`.
    pub iri: &'static str,
    /// Conventional prefix used when writing, e.g. `cim`, `eu`.
    pub prefix: &'static str,
}

#[derive(Debug)]
pub struct ProfileDef {
    /// Short keyword, e.g. `EQ`, `SSH`.
    pub keyword: &'static str,
    /// Canonical IRI, written as `md:Model.profile` on export.
    pub version_iri: &'static str,
    /// Further IRIs that also denote this profile when read.
    ///
    /// Published models name profiles by IRIs that changed between revisions, and some
    /// profiles have more than one accepted form.
    pub aliases: &'static [&'static str],
    pub title: &'static str,
}

impl ProfileDef {
    /// Whether `iri` denotes this profile, ignoring the trailing version segment.
    pub fn matches_iri(&self, iri: &str) -> bool {
        if self.version_iri == iri || self.aliases.contains(&iri) {
            return true;
        }
        fn stem(s: &str) -> Option<&str> {
            s.rsplit_once('/').map(|(a, _)| a)
        }
        let Some(want) = stem(iri) else { return false };
        stem(self.version_iri) == Some(want) || self.aliases.iter().any(|a| stem(a) == Some(want))
    }
}

#[derive(Debug)]
pub struct ClassDef {
    /// Local name, e.g. `ACLineSegment`.
    pub name: &'static str,
    pub ns: NsId,
    /// Direct superclass, if any.
    pub parent: Option<ClassId>,
    /// Whether instances of this class may appear in an instance document.
    pub concrete: bool,
    /// Profiles whose vocabulary declares this class.
    ///
    /// This is declaration, not reach: an instance may still carry another profile's
    /// data through an inherited attribute — a Steady State Hypothesis file sets
    /// `Equipment.inService` on a `BusbarSection` that only the Equipment profile
    /// declares. Use an object's stored values to decide what a profile export contains.
    pub profiles: ProfileMask,
    /// Profiles whose vocabulary *defines* this class rather than only referring to it.
    ///
    /// IEC 61970-552 distinguishes the file that introduces an object (`rdf:ID`) from a
    /// file that adds to an object defined elsewhere (`rdf:about`), and the RDFS records
    /// which is which by marking the latter with `cims:stereotype Description`. Steady
    /// State Hypothesis marks every class it touches; State Variables marks only
    /// `CsConverter` and `VsConverter`, which is why published SV files identify
    /// `SvPowerFlow` with `rdf:ID`. [`IdStyle::Auto`](crate::writer::IdStyle::Auto) reads
    /// this field.
    pub defined_in: ProfileMask,
    /// Profiles that declare this class `concrete`, i.e. may hold instances of it.
    ///
    /// Narrower than [`ClassDef::defined_in`], and deliberately so. `defined_in` answers
    /// "did this file introduce the object", which decides `rdf:ID` versus `rdf:about` and
    /// must tolerate published files that instantiate a class their own profile declares
    /// abstract. `concrete_in` answers "may this profile hold such an instance at all",
    /// which is what tells validation whether the file that would carry an object's
    /// mandatory attributes is loaded.
    pub concrete_in: ProfileMask,
    pub deprecated: bool,
    /// Whether this class is a compound value type without identity.
    ///
    /// Compounds are written inline inside the property that holds them and never carry
    /// an `rdf:ID`, so they cannot appear as top-level objects.
    pub compound: bool,
    /// Attributes declared directly on this class.
    pub own_attrs: &'static [AttrId],
    /// Every attribute visible on this class, inherited ones first (root-first order).
    pub all_attrs: &'static [AttrId],
    /// Transitive superclasses, nearest parent first.
    pub ancestors: &'static [ClassId],
    /// This class and every class inheriting from it, in schema order.
    pub descendants: &'static [ClassId],
    pub doc: &'static str,
}

#[derive(Debug)]
pub struct AttrDef {
    /// Qualified local name as serialized, e.g. `ACLineSegment.r`.
    pub name: &'static str,
    /// Short name, e.g. `r`.
    pub label: &'static str,
    pub ns: NsId,
    /// The class on which the attribute is declared.
    pub owner: ClassId,
    pub kind: AttrKind,
    pub mult: Mult,
    /// Profiles in which this attribute exists.
    pub profiles: ProfileMask,
    /// For associations, the profiles in which *this* side is the serialized one
    /// (`cims:AssociationUsed = Yes`). Always equal to `profiles` for plain attributes.
    pub used_in: ProfileMask,
    pub deprecated: bool,
    /// A value the schema pins via `cims:isFixed`.
    pub fixed: Option<&'static str>,
    pub doc: &'static str,
}

impl AttrDef {
    /// Whether this attribute is written when serializing `profile`.
    #[inline]
    pub const fn is_serialized_in(&self, profile: ProfileId) -> bool {
        self.used_in & profile.mask() != 0
    }
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: &'static str,
    pub ns: NsId,
    pub profiles: ProfileMask,
    pub values: &'static [EnumValueId],
    pub doc: &'static str,
}

#[derive(Debug)]
pub struct EnumValueDef {
    /// Qualified name as serialized, e.g. `PhaseCode.ABCN`.
    pub name: &'static str,
    /// Literal name, e.g. `ABCN`.
    pub label: &'static str,
    pub ns: NsId,
    pub owner: EnumId,
    pub doc: &'static str,
}

#[derive(Debug)]
pub struct DatatypeDef {
    pub name: &'static str,
    pub ns: NsId,
    /// The primitive actually serialized for this datatype.
    pub value: Primitive,
    /// Fixed `UnitSymbol`, e.g. `W` for `ActivePower`.
    pub unit: Option<&'static str>,
    /// Fixed `UnitMultiplier`, e.g. `M` (mega) for `ActivePower`.
    pub multiplier: Option<&'static str>,
    pub doc: &'static str,
}

/// A complete CIM schema vintage.
pub struct Schema {
    /// Vintage key, e.g. `cgmes3`.
    pub vintage: &'static str,
    pub namespaces: &'static [NamespaceDef],
    pub profiles: &'static [ProfileDef],
    pub classes: &'static [ClassDef],
    pub attributes: &'static [AttrDef],
    pub enums: &'static [EnumDef],
    pub enum_values: &'static [EnumValueDef],
    pub datatypes: &'static [DatatypeDef],
    /// `(ns index, local name)` sorted, for binary-search lookup of classes.
    pub class_index: &'static [(u16, &'static str, ClassId)],
    /// `(ns index, qualified name)` sorted, for attribute lookup.
    pub attr_index: &'static [(u16, &'static str, AttrId)],
    /// `(ns index, qualified name)` sorted, for enum literal lookup.
    pub enum_value_index: &'static [(u16, &'static str, EnumValueId)],
}

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schema")
            .field("vintage", &self.vintage)
            .field("profiles", &self.profiles.len())
            .field("classes", &self.classes.len())
            .field("attributes", &self.attributes.len())
            .finish()
    }
}

impl Schema {
    #[inline]
    pub fn class(&self, id: ClassId) -> &'static ClassDef {
        &self.classes[id.index()]
    }
    #[inline]
    pub fn attr(&self, id: AttrId) -> &'static AttrDef {
        &self.attributes[id.index()]
    }
    #[inline]
    pub fn enumeration(&self, id: EnumId) -> &'static EnumDef {
        &self.enums[id.index()]
    }
    #[inline]
    pub fn enum_value(&self, id: EnumValueId) -> &'static EnumValueDef {
        &self.enum_values[id.index()]
    }
    #[inline]
    pub fn datatype(&self, id: DatatypeId) -> &'static DatatypeDef {
        &self.datatypes[id.index()]
    }
    #[inline]
    pub fn profile(&self, id: ProfileId) -> &'static ProfileDef {
        &self.profiles[id.index()]
    }
    #[inline]
    pub fn namespace(&self, id: NsId) -> &'static NamespaceDef {
        &self.namespaces[id.index()]
    }

    pub fn ns_id(&self, iri: &str) -> Option<NsId> {
        self.namespaces
            .iter()
            .position(|n| n.iri == iri)
            .map(|i| NsId(i as u16))
    }

    pub fn profile_by_keyword(&self, keyword: &str) -> Option<ProfileId> {
        self.profiles
            .iter()
            .position(|p| p.keyword.eq_ignore_ascii_case(keyword))
            .map(|i| ProfileId(i as u16))
    }

    /// Resolve the `md:Model.profile` IRI written in an instance file header.
    ///
    /// The trailing version segment is ignored so that, for example, a file declaring
    /// `.../CoreEquipment-EU/3.0` still resolves when the schema knows `/3.0`.
    pub fn profile_by_iri(&self, iri: &str) -> Option<ProfileId> {
        // Exact matches first, so an alias never shadows a profile's own IRI.
        self.profiles
            .iter()
            .position(|p| p.version_iri == iri)
            .or_else(|| self.profiles.iter().position(|p| p.aliases.contains(&iri)))
            .or_else(|| self.profiles.iter().position(|p| p.matches_iri(iri)))
            .map(|i| ProfileId(i as u16))
    }

    /// Look up a class by namespace IRI and local name.
    pub fn find_class(&self, ns: &str, local: &str) -> Option<ClassId> {
        let ns = self.ns_id(ns)?.0;
        binary_search(self.class_index, ns, local)
    }

    /// Look up a class by local name in whichever namespace declares it.
    ///
    /// For tooling that must work across vintages without knowing their namespaces:
    /// `Substation` lives in `CIM100` in CGMES 3.0 and in `cim16` in CGMES 2.4.15, and a
    /// program that only wants to count substations should not have to spell either. Where
    /// two namespaces declare the same local name — an extension shadowing a core class —
    /// the first in namespace order wins, so [`Schema::find_class`] is the call to reach
    /// for when the distinction matters.
    pub fn class_by_name(&self, local: &str) -> Option<ClassId> {
        self.class_index
            .iter()
            .find(|(_, name, _)| *name == local)
            .map(|(_, _, id)| *id)
    }

    /// Look up an attribute by namespace IRI and qualified name (`Class.attr`).
    pub fn find_attr(&self, ns: &str, qualified: &str) -> Option<AttrId> {
        let ns = self.ns_id(ns)?.0;
        binary_search(self.attr_index, ns, qualified)
    }

    /// Look up an enumeration literal by namespace IRI and qualified name.
    pub fn find_enum_value(&self, ns: &str, qualified: &str) -> Option<EnumValueId> {
        let ns = self.ns_id(ns)?.0;
        binary_search(self.enum_value_index, ns, qualified)
    }

    /// Look up an enumeration literal by qualified name in any namespace.
    ///
    /// Exporters do misplace these: published CGMES 2.4.15 test models write the
    /// ENTSO-E extension literal `LimitTypeKind.patl` under the `cim` namespace. The
    /// qualified name is unambiguous on its own, so the value can still be recovered —
    /// callers should report the namespace mismatch rather than accept it silently.
    pub fn find_enum_value_any_ns(&self, qualified: &str) -> Option<EnumValueId> {
        self.enum_value_index
            .iter()
            .find(|(_, name, _)| *name == qualified)
            .map(|(_, _, id)| *id)
    }

    /// Whether `class` is `ancestor` or inherits from it.
    pub fn is_a(&self, class: ClassId, ancestor: ClassId) -> bool {
        class == ancestor || self.class(class).ancestors.contains(&ancestor)
    }

    /// Find an attribute visible on `class` by its short label.
    ///
    /// Searches the class and its ancestors, most-derived first.
    pub fn attr_by_label(&self, class: ClassId, label: &str) -> Option<AttrId> {
        self.class(class)
            .all_attrs
            .iter()
            .rev()
            .copied()
            .find(|&a| self.attr(a).label == label)
    }

    /// The profiles a stored value belongs in — the single rule the writer and the
    /// per-profile coverage report share, so the report cannot drift from the export.
    ///
    /// `provenance` is the profile mask recorded on the value when it was read (zero for
    /// values set programmatically). It is *intersected* with the profiles that actually
    /// declare the attribute, because one instance file routinely declares several
    /// profiles: a CGMES 3.0 Equipment file commonly declares both `CoreEquipment` and
    /// `ShortCircuit`, and without the intersection every value in it would be written to
    /// both exports, duplicating the whole file.
    ///
    /// The intersection can be empty, because published files do set attributes their own
    /// header does not declare — a CGMES 2.4.15 boundary file writes
    /// `Terminal.ConnectivityNode`, which only the Equipment profile declares. When that
    /// happens the value goes back where it came from: the data is evidence, the
    /// vocabulary is a claim about it, and re-filing the value under a profile its source
    /// does not serve would drop it from every export. [`validate`](mod@crate::validate)
    /// reports the discrepancy as [`Rule::AttributeNotInProfile`](crate::Rule).
    ///
    /// Only a value with no provenance at all — one set programmatically — falls back to
    /// the attribute's declaration, which is then the sole available answer.
    pub fn effective_profiles(&self, attr: AttrId, provenance: ProfileMask) -> ProfileMask {
        let declared = self.attr(attr).used_in;
        match provenance & declared {
            0 if provenance != 0 => provenance,
            0 => declared,
            narrowed => narrowed,
        }
    }

    /// Whether a value's source file declared a profile that declares its attribute.
    ///
    /// False means the file under-declares its own content: it carries data belonging to
    /// a profile its `md:Model.profile` does not name. Published models do this — the
    /// `RealGrid` Equipment file carries ShortCircuit attributes while declaring only
    /// `CoreEquipment` — so it is a warning about the header, not a reason to lose data.
    pub fn declaration_covers(&self, attr: AttrId, provenance: ProfileMask) -> bool {
        provenance == 0 || self.attr(attr).used_in & provenance != 0
    }

    /// How strongly a document declaring `namespaces` looks like this vintage.
    ///
    /// The number of this vintage's classes that live in namespaces the document declares.
    ///
    /// Weighted by class count rather than by namespace, because every namespace a schema
    /// knows holds at least one class — the 61970-552 header vocabulary puts classes in
    /// `md`, `dm` and even `rdf` — so presence alone discriminates nothing. Bulk does:
    /// `CIM100#` holds 436 classes against `rdf`'s one, so a CGMES 2.4.15 document scores
    /// 400 for 2.4.15 and 3 for 3.0.
    pub fn match_score<'a>(&self, namespaces: impl IntoIterator<Item = &'a str>) -> usize {
        let declared: Vec<&str> = namespaces.into_iter().collect();
        let wanted: Vec<NsId> = self
            .namespaces
            .iter()
            .enumerate()
            .filter(|(_, n)| declared.contains(&n.iri))
            .map(|(i, _)| NsId(i as u16))
            .collect();
        self.classes
            .iter()
            .filter(|c| wanted.contains(&c.ns))
            .count()
    }

    /// The compiled-in vintage a document declaring `namespaces` belongs to.
    ///
    /// CIM/XML names its vocabulary in the document — `xmlns:cim="…/CIM100#"` for CGMES 3.0,
    /// `"…/CIM-schema-cim16#"` for 2.4.15 — so the caller never has to guess, and guessing
    /// wrong is expensive: reading a 2.4.15 file against the CGMES 3.0 tables resolves no
    /// class at all and yields an empty model with one "unknown class" warning per element.
    ///
    /// Returns `None` when nothing in the document points at any compiled-in vintage, which
    /// is the honest answer for a build without the matching feature.
    ///
    /// See [`reader::sniff`](crate::reader::sniff), which answers this from a document's
    /// root element without parsing the rest.
    pub fn detect<'a>(namespaces: impl IntoIterator<Item = &'a str>) -> Option<&'static Schema> {
        let declared: Vec<&str> = namespaces.into_iter().collect();
        crate::VINTAGES
            .iter()
            .map(|s| (s.match_score(declared.iter().copied()), *s))
            .filter(|(score, _)| *score > 0)
            .max_by_key(|(score, _)| *score)
            .map(|(_, s)| s)
    }

    /// Every concrete class, useful for tooling that enumerates the model.
    pub fn concrete_classes(&self) -> impl Iterator<Item = ClassId> + '_ {
        self.classes
            .iter()
            .enumerate()
            .filter(|(_, c)| c.concrete)
            .map(|(i, _)| ClassId(i as u16))
    }
}

fn binary_search<T: Copy>(
    index: &'static [(u16, &'static str, T)],
    ns: u16,
    key: &str,
) -> Option<T> {
    index
        .binary_search_by(|(n, k, _)| n.cmp(&ns).then_with(|| (*k).cmp(key)))
        .ok()
        .map(|i| index[i].2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplicity_bounds() {
        assert!(Mult::Required.is_required());
        assert!(!Mult::Optional.is_required());
        assert!(Mult::Many.is_many());
        assert_eq!(Mult::Optional.max(), 1);
        assert_eq!(Mult::Many.max(), u32::MAX);
        assert_eq!(Mult::Bounded { min: 0, max: 2 }.max(), 2);
        assert!(Mult::Bounded { min: 1, max: 2 }.is_required());
    }

    #[test]
    fn profile_masks_are_distinct_bits() {
        assert_eq!(ProfileId(0).mask(), 1);
        assert_eq!(ProfileId(3).mask(), 8);
    }
}
