//! Model headers: `md:FullModel` and `dm:DifferenceModel` (IEC 61970-552).
//!
//! Every CIM/XML instance file starts with a header describing what the file contains,
//! which profiles it conforms to, and which other models it depends on. Assembling a
//! multi-file model correctly requires reading these, so headers are first-class here
//! rather than metadata to be skipped.

use crate::mrid::Mrid;

/// Namespace of the model description vocabulary.
pub const MD_NS: &str = "http://iec.ch/TC57/61970-552/ModelDescription/1#";
/// Namespace of the difference model vocabulary.
pub const DM_NS: &str = "http://iec.ch/TC57/61970-552/DifferenceModel/1#";

/// What kind of header a file carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModelKind {
    /// `md:FullModel` — a complete model instance.
    #[default]
    Full,
    /// `dm:DifferenceModel` — a change set against a superseded model.
    Difference,
}

/// The header of one instance file.
#[derive(Clone, Debug, Default)]
pub struct ModelHeader {
    pub kind: ModelKind,
    /// Identifier of this model, written as `rdf:about="urn:uuid:..."`.
    pub id: Option<Mrid>,
    /// `md:Model.created`, in the file's lexical form.
    pub created: Option<String>,
    /// `md:Model.scenarioTime`.
    pub scenario_time: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub modeling_authority_set: Option<String>,
    /// `md:Model.profile`, repeatable: one file may serve several profiles, as CGMES 3.0
    /// does when Equipment, Operation and ShortCircuit share an instance file.
    pub profiles: Vec<String>,
    /// `md:Model.DependentOn` — models that must be loaded for this one to resolve.
    pub dependent_on: Vec<Mrid>,
    /// `md:Model.Supersedes` — models this one replaces.
    pub supersedes: Vec<Mrid>,
    /// Header properties outside the vocabulary above, preserved verbatim.
    ///
    /// CGMES 3.0 headers may carry W3C DCAT properties alongside `md:Model.*`, and
    /// producers add their own. Each keeps the prefix, namespace and value form it was
    /// read with, so a header round-trips through this crate unchanged rather than being
    /// flattened into `md:` element text.
    pub extra: Vec<HeaderProperty>,
    /// Name of the file this header came from, when read from disk or an archive.
    pub source: Option<String>,
}

/// A header property this crate does not model explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderProperty {
    /// Prefix as written in the source document, e.g. `dcat`.
    pub prefix: String,
    /// Namespace the prefix resolved to.
    pub ns: String,
    /// Local name, e.g. `Model.something`.
    pub local: String,
    pub value: HeaderValue,
}

/// Whether a header property was written as element text or as an `rdf:resource`.
///
/// The distinction matters: re-emitting a resource as text changes an IRI-valued property
/// into a literal, which no longer resolves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderValue {
    Text(String),
    Resource(String),
}

impl HeaderProperty {
    pub fn text(prefix: &str, ns: &str, local: &str, value: impl Into<String>) -> HeaderProperty {
        HeaderProperty {
            prefix: prefix.to_owned(),
            ns: ns.to_owned(),
            local: local.to_owned(),
            value: HeaderValue::Text(value.into()),
        }
    }

    pub fn resource(
        prefix: &str,
        ns: &str,
        local: &str,
        value: impl Into<String>,
    ) -> HeaderProperty {
        HeaderProperty {
            prefix: prefix.to_owned(),
            ns: ns.to_owned(),
            local: local.to_owned(),
            value: HeaderValue::Resource(value.into()),
        }
    }

    /// The value as text, whichever form it was written in.
    pub fn as_str(&self) -> &str {
        match &self.value {
            HeaderValue::Text(s) | HeaderValue::Resource(s) => s,
        }
    }
}

impl ModelHeader {
    pub fn new(kind: ModelKind) -> ModelHeader {
        ModelHeader {
            kind,
            ..Default::default()
        }
    }

    /// Whether this header declares `profile_iri`, ignoring the version segment.
    pub fn declares_profile(&self, profile_iri: &str) -> bool {
        let stem = |s: &str| s.rsplit_once('/').map(|(a, _)| a).unwrap_or(s).to_owned();
        let want = stem(profile_iri);
        self.profiles
            .iter()
            .any(|p| p == profile_iri || stem(p) == want)
    }

    /// The short profile keyword, derived from the first declared profile IRI.
    ///
    /// For example `http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0` yields
    /// `CoreEquipment-EU`.
    pub fn primary_profile_name(&self) -> Option<&str> {
        let iri = self.profiles.first()?;
        let without_version = iri.rsplit_once('/').map(|(a, _)| a).unwrap_or(iri);
        without_version.rsplit('/').next()
    }
}

/// A difference model: statements to remove, then statements to add.
///
/// IEC 61970-552 defines a difference as a pair of RDF statement sets. Applying it means
/// removing every reverse statement and then asserting every forward statement.
#[derive(Clone, Debug, Default)]
pub struct DifferenceModel {
    pub header: ModelHeader,
    /// Statements describing the *previous* state, to be removed.
    pub reverse: Vec<Statement>,
    /// Statements describing the *new* state, to be added.
    pub forward: Vec<Statement>,
}

/// One statement inside a difference model: a subject, a predicate and a value.
///
/// Statements are kept in unresolved form (IRIs, raw text) because a difference model
/// may legitimately reference objects and properties that are not in the dataset yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// mRID of the object the statement is about.
    pub subject: Mrid,
    /// Class named on the statement group's element.
    ///
    /// A difference names its subject either with a bare `rdf:Description` — in which
    /// case this is `None` — or with the class element itself. The latter carries real
    /// information: a difference may *reclassify* an object, and the published CGMES
    /// conformity tests do exactly that, replacing a `LinearShuntCompensator` with a
    /// `NonlinearShuntCompensator` under the same identifier.
    pub class: Option<QualifiedName>,
    /// Whether the statement group introduced its subject (`rdf:ID`) rather than updating
    /// one defined elsewhere (`rdf:about`).
    ///
    /// Both forms occur in the published change sets: an Equipment difference that adds
    /// breakers writes `rdf:ID`, while one that edits a limit writes `rdf:about`. Keeping
    /// the distinction is what lets a difference be written back as it was read.
    pub defines_subject: bool,
    /// Namespace IRI of the predicate.
    pub predicate_ns: String,
    /// Qualified predicate name, e.g. `TapChangerTablePoint.x`.
    pub predicate: String,
    pub value: StatementValue,
}

/// A namespace-qualified name as it appeared in a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualifiedName {
    pub ns: String,
    pub local: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementValue {
    /// Element text.
    Literal(String),
    /// An `rdf:resource` IRI: another object, or an enumeration literal.
    Resource(String),
}

impl DifferenceModel {
    /// Objects touched by this difference, in first-seen order.
    pub fn affected_objects(&self) -> Vec<Mrid> {
        let mut seen = Vec::new();
        for s in self.reverse.iter().chain(&self.forward) {
            if !seen.contains(&s.subject) {
                seen.push(s.subject.clone());
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_matching_ignores_the_version_segment() {
        let h = ModelHeader {
            profiles: vec!["http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0".into()],
            ..Default::default()
        };
        assert!(h.declares_profile("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0"));
        assert!(h.declares_profile("http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/4.0"));
        assert!(!h.declares_profile("http://iec.ch/TC57/ns/CIM/Topology-EU/3.0"));
        assert_eq!(h.primary_profile_name(), Some("CoreEquipment-EU"));
    }

    #[test]
    fn affected_objects_are_deduplicated_in_order() {
        let mk = |m: &str, p: &str| Statement {
            subject: Mrid::parse(m),
            class: None,
            defines_subject: false,
            predicate_ns: "http://iec.ch/TC57/CIM100#".into(),
            predicate: p.into(),
            value: StatementValue::Literal("1".into()),
        };
        let a = "70c4656c-f7a0-4319-98bb-84fb5e2e9b37";
        let b = "c4e8eb3a-76b0-4192-91fe-c2f0a60af0df";
        let d = DifferenceModel {
            reverse: vec![mk(a, "X.y"), mk(b, "X.y")],
            forward: vec![mk(a, "X.z")],
            ..Default::default()
        };
        assert_eq!(d.affected_objects(), vec![Mrid::parse(a), Mrid::parse(b)]);
    }
}
