//! Errors and structured diagnostics.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

/// A hard failure: the operation could not be completed.
///
/// Problems that leave the data usable — an unknown class, an unresolved reference in
/// lenient mode — are reported as [`Diagnostic`]s instead, so that reading a
/// non-conforming file still yields a model plus a precise report.
///
/// `#[non_exhaustive]`: a new CIM vintage or a new input syntax is a regeneration away, and
/// a variant added for one should not be a breaking change for everyone. Match with a
/// wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(std::io::Error),
    /// The document is not well-formed XML.
    Xml(String),
    /// The document is well-formed XML but not a CIM/XML document.
    NotCimXml(String),
    /// A reference could not be resolved and the link policy is strict.
    DanglingReference {
        object: String,
        attribute: &'static str,
        target: String,
        /// Total number of unresolved references found.
        total: usize,
    },
    /// Two models built from different CIM vintages cannot be combined.
    ///
    /// Class and attribute identifiers are indices into one vintage's tables, so the same
    /// number means different things in two of them. Combining the models regardless would
    /// not fail loudly — it would silently file `ACLineSegment.r` under whatever the other
    /// vintage happens to put at that index.
    SchemaMismatch {
        left: &'static str,
        right: &'static str,
    },
    /// No file under the given path declares a CIM vocabulary this build recognises.
    ///
    /// Either the input is not CIM/XML, or it is a vintage this build has no feature for.
    UnknownVintage {
        path: String,
        /// The vintages that *are* compiled in.
        known: Vec<&'static str>,
    },
    /// A zip archive could not be read.
    #[cfg(feature = "zip")]
    Zip(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::Xml(m) => write!(f, "malformed XML: {m}"),
            Error::NotCimXml(m) => write!(f, "not a CIM/XML document: {m}"),
            Error::DanglingReference {
                object,
                attribute,
                target,
                total,
            } => write!(
                f,
                "unresolved reference {object}.{attribute} -> {target} \
                 ({total} unresolved in total); load the missing profile or \
                 use LinkPolicy::Lenient"
            ),
            Error::SchemaMismatch { left, right } => write!(
                f,
                "cannot combine a {left} model with a {right} one: class and attribute \
                 identifiers belong to one vintage's tables"
            ),
            Error::UnknownVintage { path, known } => write!(
                f,
                "no CIM vintage recognised in {path}: not a CIM/XML input, or a vocabulary \
                 this build has no feature for (it has {known:?})"
            ),
            #[cfg(feature = "zip")]
            Error::Zip(m) => write!(f, "zip archive error: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

impl From<quick_xml::Error> for Error {
    fn from(e: quick_xml::Error) -> Error {
        Error::Xml(e.to_string())
    }
}

impl From<quick_xml::events::attributes::AttrError> for Error {
    fn from(e: quick_xml::events::attributes::AttrError) -> Error {
        Error::Xml(e.to_string())
    }
}

/// How much a diagnostic matters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    /// Violates the standard; the model is not conforming.
    Error,
    /// Legal but suspect, or a tolerated deviation.
    Warning,
    /// Informational, e.g. a deprecated attribute in use.
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        })
    }
}

/// A stable identifier for the rule that produced a diagnostic.
///
/// Rule codes let a pipeline filter, suppress or fail on specific classes of problem
/// without matching message text.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Rule {
    /// An element names a class the schema does not define.
    UnknownClass,
    /// An element names an attribute the schema does not define for its class.
    UnknownAttribute,
    /// An attribute value could not be parsed as its declared type.
    InvalidValue,
    /// An identifier is not a UUID, as IEC 61970-552 requires.
    NonConformingMrid,
    /// Two objects share an mRID.
    DuplicateMrid,
    /// A reference points at an object not present in the dataset.
    DanglingReference,
    /// A required attribute is missing.
    MissingRequired,
    /// An attribute occurs more often than its multiplicity permits.
    CardinalityExceeded,
    /// A reference points at an object of the wrong class.
    WrongReferenceTarget,
    /// An attribute is used outside the profiles that declare it.
    AttributeNotInProfile,
    /// An abstract class was instantiated.
    AbstractInstantiated,
    /// A deprecated element is in use.
    Deprecated,
    /// The file header is missing or incomplete.
    MalformedHeader,
    /// A header declares a dependency that was not loaded.
    UnsatisfiedDependency,
    /// A structural problem in the document that did not prevent reading.
    Structure,
    /// A stored value does not have the shape its attribute declares.
    ///
    /// CIM/XML carries no datatype information — IEC 61970-552 leaves it to the RDFS —
    /// so a value read as text from an element the schema types as a float, or an
    /// enumeration literal belonging to a different enumeration, is only detectable
    /// against the schema.
    DatatypeMismatch,
    /// An attribute the schema pins with `cims:isFixed` carries a different value.
    FixedValueMismatch,
    /// Two loaded files gave different values for one single-valued attribute.
    ///
    /// Merging by mRID assembles a model from files that each describe part of it. Where
    /// two of them disagree, one value has to be discarded — so the disagreement is
    /// reported rather than resolved silently.
    ConflictingValue,
    /// A value holds a character XML 1.0 cannot represent.
    ///
    /// Not a matter of escaping: XML's `Char` production excludes most of the C0 range,
    /// and forbids a numeric character reference to it as well, so there is no way to
    /// write such a character at all. A document containing one is rejected by every
    /// conforming parser at the offending byte. `quick-xml` does not enforce the
    /// production, so nothing else catches one arriving from a mis-encoded or corrupted
    /// source file. The reader strips the character and reports it; this rule also fires
    /// for a value put in through the public API.
    IllegalXmlCharacter,
    /// The document is written against a different schema vintage than the one reading it.
    ///
    /// CIM/XML declares its vocabulary — `…/CIM100#` for CGMES 3.0, `…/CIM-schema-cim16#`
    /// for 2.4.15 — so this is knowable from the root element. Without it a vintage mismatch
    /// surfaces only as one `CIM0001` per element and a model with nothing in it.
    WrongVintage,
    /// An identifier has no valid RDF/XML serialization.
    ///
    /// `rdf:ID` is an XML `NCName` and `rdf:about` is an IRI reference. Every UUID
    /// satisfies the first, so this cannot fire on conforming input; an identifier that is
    /// neither a name nor an IRI — one containing a space, say — is written anyway,
    /// because losing the object would be worse, but the document is not valid RDF/XML and
    /// a general RDF toolchain will refuse it. Well-formedness checking cannot see this:
    /// the document *is* well-formed XML.
    UnserializableIdentifier,
}

impl Rule {
    /// Short stable code, e.g. `CIM0007`, suitable for CI filters.
    pub const fn code(self) -> &'static str {
        match self {
            Rule::UnknownClass => "CIM0001",
            Rule::UnknownAttribute => "CIM0002",
            Rule::InvalidValue => "CIM0003",
            Rule::NonConformingMrid => "CIM0004",
            Rule::DuplicateMrid => "CIM0005",
            Rule::DanglingReference => "CIM0006",
            Rule::MissingRequired => "CIM0007",
            Rule::CardinalityExceeded => "CIM0008",
            Rule::WrongReferenceTarget => "CIM0009",
            Rule::AttributeNotInProfile => "CIM0010",
            Rule::AbstractInstantiated => "CIM0011",
            Rule::Deprecated => "CIM0012",
            Rule::MalformedHeader => "CIM0013",
            Rule::UnsatisfiedDependency => "CIM0014",
            Rule::Structure => "CIM0015",
            Rule::DatatypeMismatch => "CIM0016",
            Rule::FixedValueMismatch => "CIM0017",
            Rule::ConflictingValue => "CIM0018",
            Rule::IllegalXmlCharacter => "CIM0019",
            Rule::UnserializableIdentifier => "CIM0020",
            Rule::WrongVintage => "CIM0021",
        }
    }

    /// Every rule, so tooling can enumerate the catalogue.
    pub const ALL: &'static [Rule] = &[
        Rule::UnknownClass,
        Rule::UnknownAttribute,
        Rule::InvalidValue,
        Rule::NonConformingMrid,
        Rule::DuplicateMrid,
        Rule::DanglingReference,
        Rule::MissingRequired,
        Rule::CardinalityExceeded,
        Rule::WrongReferenceTarget,
        Rule::AttributeNotInProfile,
        Rule::AbstractInstantiated,
        Rule::Deprecated,
        Rule::MalformedHeader,
        Rule::UnsatisfiedDependency,
        Rule::Structure,
        Rule::DatatypeMismatch,
        Rule::FixedValueMismatch,
        Rule::ConflictingValue,
        Rule::IllegalXmlCharacter,
        Rule::UnserializableIdentifier,
        Rule::WrongVintage,
    ];
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A structured finding, usable by CI and tooling rather than only printable.
///
/// The object and attribute fields hold the identifier and the schema's own name rather
/// than rendered text: a caller that wants to look the object up, group findings by it or
/// re-serialize it should not have to parse a diagnostic message back into data, and
/// rendering a UUID costs an allocation on a path that fires once per finding.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub rule: Rule,
    /// Identifier of the object concerned, when the finding is object-scoped.
    pub object: Option<crate::mrid::Mrid>,
    /// Class name of that object.
    pub class: Option<&'static str>,
    /// Qualified attribute name, when the finding is attribute-scoped.
    pub attribute: Option<&'static str>,
    /// Source file the finding came from.
    pub source: Option<String>,
    /// Byte offset in the source document, when the finding came from parsing.
    ///
    /// A byte offset rather than a line number, because the reader is streaming: it never
    /// holds the document, so it cannot count lines without either buffering it or
    /// carrying an index proportional to its size. An offset is exact, free to record and
    /// what an editor or a `dd`-style seek wants anyway; [`line_and_column`] converts one
    /// when a human needs to read it.
    pub position: Option<u64>,
    pub message: String,
}

/// Convert a byte offset in `source` to a 1-based line and column.
///
/// Pairs with [`Diagnostic::position`], which the streaming reader records instead of a
/// line number. Reads `source` up to `position` only.
pub fn line_and_column(source: &str, position: u64) -> (u64, u64) {
    let upto = &source.as_bytes()[..(position as usize).min(source.len())];
    let line = upto.iter().filter(|&&b| b == b'\n').count() as u64 + 1;
    let col = match upto.iter().rposition(|&b| b == b'\n') {
        Some(i) => (upto.len() - i) as u64,
        None => upto.len() as u64 + 1,
    };
    (line, col)
}

impl Diagnostic {
    pub fn new(severity: Severity, rule: Rule, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity,
            rule,
            object: None,
            class: None,
            attribute: None,
            source: None,
            position: None,
            message: message.into(),
        }
    }

    pub fn error(rule: Rule, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, rule, message)
    }
    pub fn warning(rule: Rule, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Warning, rule, message)
    }
    pub fn info(rule: Rule, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Info, rule, message)
    }

    pub fn with_object(mut self, mrid: impl Into<crate::mrid::Mrid>) -> Diagnostic {
        self.object = Some(mrid.into());
        self
    }
    pub fn with_class(mut self, class: &'static str) -> Diagnostic {
        self.class = Some(class);
        self
    }
    pub fn with_attribute(mut self, attr: &'static str) -> Diagnostic {
        self.attribute = Some(attr);
        self
    }
    pub fn with_source(mut self, source: impl Into<String>) -> Diagnostic {
        self.source = Some(source.into());
        self
    }
    /// Record the byte offset in the source document the finding came from.
    pub fn with_position(mut self, position: u64) -> Diagnostic {
        self.position = Some(position);
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.severity, self.rule.code())?;
        if let Some(src) = &self.source {
            write!(f, " {src}")?;
            if let Some(pos) = self.position {
                // `@` rather than `:` so the number is not mistaken for a line.
                write!(f, "@{pos}")?;
            }
        }
        write!(f, ": {}", self.message)?;
        if let Some(class) = self.class {
            write!(f, " [{class}")?;
            match &self.object {
                Some(o) => write!(f, " {o}]")?,
                None => write!(f, "]")?,
            }
        } else if let Some(o) = &self.object {
            write!(f, " [{o}]")?;
        }
        Ok(())
    }
}

/// A collection of diagnostics with convenience queries.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn push(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }

    pub fn extend(&mut self, other: Report) {
        self.diagnostics.extend(other.diagnostics);
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }

    /// Whether any diagnostic is an error, i.e. the model is non-conforming.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.diagnostics.iter()
    }

    pub fn by_rule(&self, rule: Rule) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(move |d| d.rule == rule)
    }

    /// Number of findings per rule, most frequent first — a useful CI summary.
    pub fn summary(&self) -> Vec<(Rule, usize)> {
        let mut counts: Vec<(Rule, usize)> = Vec::new();
        for d in &self.diagnostics {
            match counts.iter_mut().find(|(r, _)| *r == d.rule) {
                Some((_, n)) => *n += 1,
                None => counts.push((d.rule, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        counts
    }
}

impl IntoIterator for Report {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;
    fn into_iter(self) -> Self::IntoIter {
        self.diagnostics.into_iter()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.diagnostics {
            writeln!(f, "{d}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_codes_are_unique() {
        let mut codes: Vec<&str> = Rule::ALL.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), n, "duplicate rule codes");
    }

    #[test]
    fn byte_offsets_convert_to_lines_and_columns() {
        let src = "one\ntwo\nthree";
        assert_eq!(line_and_column(src, 0), (1, 1));
        assert_eq!(line_and_column(src, 4), (2, 1));
        assert_eq!(line_and_column(src, 6), (2, 3));
        // Past the end clamps rather than panicking.
        assert_eq!(line_and_column(src, 9_999).0, 3);
    }

    #[test]
    fn report_summarizes_by_rule() {
        let mut r = Report::default();
        r.push(Diagnostic::error(Rule::MissingRequired, "a"));
        r.push(Diagnostic::error(Rule::MissingRequired, "b"));
        r.push(Diagnostic::warning(Rule::Deprecated, "c"));
        assert!(r.has_errors());
        assert_eq!(r.count(Severity::Error), 2);
        assert_eq!(
            r.summary(),
            vec![(Rule::MissingRequired, 2), (Rule::Deprecated, 1)]
        );
    }

    #[test]
    fn diagnostic_display_includes_location_and_code() {
        let d = Diagnostic::error(Rule::MissingRequired, "missing name")
            .with_class("ACLineSegment")
            .with_object("abc")
            .with_source("EQ.xml")
            .with_position(1234);
        let s = d.to_string();
        assert!(s.contains("error[CIM0007]"), "{s}");
        assert!(s.contains("EQ.xml@1234"), "{s}");
        assert!(s.contains("[ACLineSegment abc]"), "{s}");
    }
}
