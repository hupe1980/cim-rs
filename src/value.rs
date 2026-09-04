//! Attribute values.
//!
//! [`Mrid`]: crate::mrid::Mrid

use crate::mrid::Mrid;
use crate::object::Compound;
use crate::schema::{EnumValueId, Primitive, Schema};
use std::fmt;

/// The value of a single attribute occurrence.
///
/// CIM datatypes (`Resistance`, `ActivePower`, ...) serialize as their underlying
/// primitive, so they are represented by [`Value::Float`] and friends; the unit and
/// multiplier are schema constants recoverable from the attribute's
/// [`DatatypeDef`](crate::schema::DatatypeDef).
#[derive(Clone, PartialEq)]
pub enum Value {
    Boolean(bool),
    Integer(i64),
    Float(Real),
    /// Strings, dates, times and durations; CIM keeps these in their lexical form.
    Text(Box<str>),
    /// An enumeration literal.
    Enum(EnumValueId),
    /// A reference to another object, by mRID.
    Reference(Mrid),
    /// An inline structured value with no identity of its own.
    ///
    /// Boxed because compounds are rare — four types in all of CGMES 3.0 — and a
    /// `Compound` inline would triple the size of every value in the model.
    Compound(Box<Compound>),
}

/// A CIM `Float` or `Decimal` value: the number, and the spelling it arrived in.
///
/// The number is the value; the spelling is how one document happened to write it. They
/// are kept apart for the same reason [`Mrid`] keeps a UUID apart from its spelling, and
/// the consequences are the same shape: two `Real`s that denote one number are **equal
/// whatever they were written as**, so a difference between two model states never reports
/// a change that is only a reformatting — while a re-export still reproduces the document
/// it read.
///
/// It matters because published models are full of forms no formatter would choose. The
/// CGMES 3.0 conformity corpus writes `2.62637E-05`, `0e+000` and `250.000000`; rendering
/// those as `0.0000262637`, `0` and `250` re-exports a numerically identical model as a
/// textually different file, which is what a receiving system diffs. ENTSO-E's RDF-Syntax
/// User Guide is explicit that engineering notation is chosen deliberately — from CIM18
/// active power travels as `-90E6` rather than as `-90000000` precisely to carry the
/// producer's precision — so a number's spelling is information, not noise.
///
/// A spelling is kept **only when it is a conforming lexical form for the primitive it was
/// read as**. The reader accepts `1,5` and `1.5D+3` from files that deviate
/// ([`Value::parse_primitive`]), and reproducing those would propagate a defect into
/// output this crate promises is valid; those are repaired to the canonical form, which is
/// the same stance the header takes on a malformed `urn:uuid:`.
#[derive(Clone)]
pub struct Real(Repr);

/// Two shapes rather than a number beside an `Option<Box<str>>`, and the reason is
/// [`Value`]'s size.
///
/// An optional string inline makes `Real` 24 bytes, which makes `Value` 32 and every slot
/// in the model 8 bytes larger — a measured ~10% on read for a spelling 98.5% of values do
/// not have. Boxing the rare case keeps `Real` pointer-sized, so `Value` stays 24 bytes and
/// the common value costs exactly what it did before: a `f64`. The published CGMES 3.0
/// corpus allocates 8,623 times for its 951,340 values.
#[derive(Clone)]
enum Repr {
    /// Renders canonically.
    Plain(f64),
    /// The number, and the spelling a document wrote it with.
    Written(Box<(f64, Box<str>)>),
}

impl Real {
    /// A number with no particular spelling: it renders canonically.
    pub fn new(value: f64) -> Real {
        Real(Repr::Plain(value))
    }

    /// A number as a document wrote it.
    ///
    /// The text is kept only when it differs from what this crate would write anyway and
    /// is a conforming lexical form for `prim` — so a value that was already canonical
    /// costs nothing, and a deviating one is repaired rather than echoed.
    pub fn from_document(value: f64, text: &str, prim: Primitive) -> Real {
        if is_canonical_plain(value, text) || text == canonical(value, prim) {
            return Real(Repr::Plain(value));
        }
        if !is_lexical(prim, text) {
            return Real(Repr::Plain(value));
        }
        Real(Repr::Written(Box::new((value, text.into()))))
    }

    #[inline]
    pub fn get(&self) -> f64 {
        match &self.0 {
            Repr::Plain(v) => *v,
            Repr::Written(w) => w.0,
        }
    }

    /// The spelling this value was read with, where it differs from the canonical form.
    pub fn lexical(&self) -> Option<&str> {
        match &self.0 {
            Repr::Plain(_) => None,
            Repr::Written(w) => Some(&w.1),
        }
    }

    /// How to write it: the document's spelling where there is one, else canonical.
    pub fn to_lexical_as(&self, prim: Primitive) -> String {
        match &self.0 {
            Repr::Plain(v) => canonical(*v, prim),
            // A spelling read as one primitive can be wrong for another: `0e+000` is a
            // valid `xsd:float` and not a valid `xsd:decimal`, and the RDF writer types a
            // literal from the profile rather than from how it was read.
            Repr::Written(w) if is_lexical(prim, &w.1) => w.1.to_string(),
            Repr::Written(w) => canonical(w.0, prim),
        }
    }
}

/// Equality is the number. The spelling is not part of the value, so `1.0` read from one
/// file equals `1` computed in memory — which is what keeps [`crate::diff`] from reporting
/// a change where a producer only changed its formatter.
///
/// It is `f64` equality, so `NaN` equals nothing, itself included. CIM has no use for one
/// and no published model contains one, but a difference computed over a model that does
/// will report that value as changed on every run.
impl PartialEq for Real {
    fn eq(&self, other: &Real) -> bool {
        self.get() == other.get()
    }
}

impl fmt::Debug for Real {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.lexical() {
            Some(s) => write!(f, "{}({s})", self.get()),
            None => write!(f, "{}", self.get()),
        }
    }
}

impl fmt::Display for Real {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_lexical_as(Primitive::Float))
    }
}

impl From<f64> for Real {
    fn from(value: f64) -> Real {
        Real::new(value)
    }
}

/// Whether `text` is already exactly what [`canonical`] would render, decided without
/// rendering it.
///
/// The comparison it replaces formats a `f64` into a fresh `String` for **every** float in
/// the document, which measured 6% of the time to read a 112 MiB model — paid on 100% of
/// values to learn something about 1.5% of them. This is the same question answered by one
/// pass over the bytes.
///
/// It is deliberately conservative: `false` means "format it and see", so a shape this does
/// not recognise costs the old comparison and never a wrong answer. The two ways to be
/// wrong are both safe — claiming canonical when it is not re-exports the canonical form,
/// which is what the census over the published corpus would catch, and claiming
/// non-canonical when it is costs one allocation and writes identical text.
///
/// The reasoning: Rust's `Display` for `f64` is the shortest decimal that round-trips, and
/// [`format_float`] uses it inside `1e-5..1e16`. A text in that range which has no leading
/// zero, no trailing fraction zero, no sign but `-`, no exponent and at most 17 significant
/// digits and which parsed to `value` *is* that shortest form: any shorter decimal would
/// round-trip to a different `f64`, and 17 digits always suffice to name one.
fn is_canonical_plain(value: f64, text: &str) -> bool {
    let abs = value.abs();
    if abs != 0.0 && !(1e-5..1e16).contains(&abs) {
        // `format_float` writes these in exponent form; let the exact comparison decide.
        return false;
    }
    let body = match text.strip_prefix('-') {
        Some(rest) => {
            // `-0` renders as `0`, and `-0.0` is not reachable here for another reason:
            // it would have to have a trailing zero.
            if value == 0.0 {
                return false;
            }
            rest
        }
        None => {
            if value < 0.0 {
                return false;
            }
            text
        }
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || (int.len() > 1 && int.starts_with('0')) {
        return false;
    }
    if let Some(f) = frac
        && (!digits(f) || f.ends_with('0'))
    {
        return false;
    }
    // Enough digits to name any `f64` uniquely; beyond that a longer text can still parse
    // to a value with a shorter rendering, so it is not decidable here.
    int.len() + frac.map_or(0, str::len) <= 17
}

/// The form this crate writes for `value` when nothing better is known.
fn canonical(value: f64, prim: Primitive) -> String {
    match prim {
        Primitive::Decimal => plain_decimal(value),
        _ => format_float(value),
    }
}

/// Whether `s` is a lexical form of `prim` that a conforming consumer will accept.
///
/// Only the two numeric primitives are asked about, because they are the only ones whose
/// stored value is not the text itself. The grammars are XML Schema's, and the difference
/// between them is the one that matters here: `xsd:decimal` has no exponent.
fn is_lexical(prim: Primitive, s: &str) -> bool {
    let exponent_allowed = match prim {
        Primitive::Float => true,
        Primitive::Decimal => false,
        _ => return false,
    };
    if matches!(s, "INF" | "-INF" | "+INF" | "NaN") {
        return exponent_allowed;
    }
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) if exponent_allowed => (&s[..i], Some(&s[i + 1..])),
        Some(_) => return false,
        None => (s, None),
    };
    let digits_around_point = |t: &str| {
        let t = t.strip_prefix(['+', '-']).unwrap_or(t);
        let (int, frac) = t.split_once('.').unwrap_or((t, ""));
        !(int.is_empty() && frac.is_empty())
            && int.chars().all(|c| c.is_ascii_digit())
            && frac.chars().all(|c| c.is_ascii_digit())
    };
    let integer = |t: &str| {
        let t = t.strip_prefix(['+', '-']).unwrap_or(t);
        !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
    };
    digits_around_point(mantissa) && exponent.is_none_or(integer)
}

impl Value {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            Value::Float(f) => Some(f.get() as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(f.get()),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_enum(&self) -> Option<EnumValueId> {
        match self {
            Value::Enum(e) => Some(*e),
            _ => None,
        }
    }
    pub fn as_reference(&self) -> Option<&Mrid> {
        match self {
            Value::Reference(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Value::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// Render in the lexical form CIM/XML uses for element text.
    ///
    /// Only meaningful for values serialized as text; enumerations, references and
    /// compounds have their own serializations and return `None`.
    pub fn to_lexical(&self) -> Option<String> {
        Some(match self {
            Value::Boolean(b) => if *b { "true" } else { "false" }.to_owned(),
            Value::Integer(i) => i.to_string(),
            Value::Float(f) => f.to_lexical_as(Primitive::Float),
            Value::Text(s) => s.to_string(),
            Value::Enum(_) | Value::Reference(_) | Value::Compound(_) => return None,
        })
    }

    /// Render in the lexical form `prim` permits.
    ///
    /// Only one primitive is fussier than [`Value::to_lexical`]: `xsd:decimal`'s lexical
    /// space has **no exponent notation**, while `xsd:float`'s does and needs it — Rust's
    /// plain float formatting would otherwise render `1.5e300` as 301 digits. Decimals are
    /// therefore always written out in full, which is bounded in practice because the
    /// values CIM types as `Decimal` are money and per-unit quantities.
    pub fn to_lexical_as(&self, prim: Primitive) -> Option<String> {
        match self {
            Value::Float(f) => Some(f.to_lexical_as(prim)),
            _ => self.to_lexical(),
        }
    }

    /// Whether this value can serve an attribute declared as `prim`.
    ///
    /// The single definition of that question, shared by
    /// [`validate`](mod@crate::validate) — which reports a mismatch — and by the RDF writer,
    /// which has to decide whether to label the literal with the declared type or with
    /// the one the value actually has.
    ///
    /// Two allowances are deliberate. An integer serves a float or decimal, because
    /// `1` is a perfectly good float and CIM/XML gives no way to say otherwise. And dates,
    /// times and durations are kept in their lexical form by design, so text serves them.
    pub fn fits(&self, prim: Primitive) -> bool {
        match prim {
            Primitive::Boolean => matches!(self, Value::Boolean(_)),
            Primitive::Integer => matches!(self, Value::Integer(_)),
            Primitive::Float | Primitive::Decimal => {
                matches!(self, Value::Float(_) | Value::Integer(_))
            }
            Primitive::String
            | Primitive::Date
            | Primitive::DateTime
            | Primitive::MonthDay
            | Primitive::Time
            | Primitive::Duration
            | Primitive::Uri => matches!(self, Value::Text(_)),
        }
    }

    /// A one-word description of what this value is, for diagnostics.
    pub fn shape(&self) -> &'static str {
        match self {
            Value::Boolean(_) => "a boolean",
            Value::Integer(_) => "an integer",
            Value::Float(_) => "a float",
            Value::Text(_) => "text",
            Value::Enum(_) => "an enumeration literal",
            Value::Reference(_) => "a reference",
            Value::Compound(_) => "a compound",
        }
    }

    /// Parse element text according to the attribute's primitive type.
    pub fn parse_primitive(prim: Primitive, text: &str) -> Result<Value, ParseValueError> {
        let t = text.trim();
        Ok(match prim {
            Primitive::Boolean => match t {
                "true" | "1" => Value::Boolean(true),
                "false" | "0" => Value::Boolean(false),
                _ => return Err(ParseValueError::new(prim, text)),
            },
            Primitive::Integer => Value::Integer(
                t.parse::<i64>()
                    // A few exporters write integral values with a decimal point.
                    .or_else(|_| t.parse::<f64>().map(|f| f as i64))
                    .map_err(|_| ParseValueError::new(prim, text))?,
            ),
            Primitive::Float | Primitive::Decimal => {
                let f = parse_float(t).ok_or_else(|| ParseValueError::new(prim, text))?;
                Value::Float(Real::from_document(f, t, prim))
            }
            Primitive::String
            | Primitive::Date
            | Primitive::DateTime
            | Primitive::MonthDay
            | Primitive::Time
            | Primitive::Duration
            | Primitive::Uri => Value::Text(text.into()),
        })
    }
}

// Conversions, so building a model reads like the data rather than like the storage.
//
// `Object::set(attrs::ac_line_segment::r, 2.2.into())` beats
// `Object::set(attrs::ac_line_segment::r, Value::from(2.2))`, and the generated attribute
// constants already make the *left* side typed. There is deliberately no `From<i32>` or
// `From<f32>`: CIM's `Integer` is 64-bit and its `Float` is stored as `f64`, and silently
// widening a narrower literal is how a value ends up a different type than it looks.

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Boolean(b)
    }
}
impl From<i64> for Value {
    fn from(i: i64) -> Value {
        Value::Integer(i)
    }
}
impl From<f64> for Value {
    fn from(f: f64) -> Value {
        Value::Float(Real::new(f))
    }
}

impl From<Real> for Value {
    fn from(r: Real) -> Value {
        Value::Float(r)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Text(s.into())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Text(s.into_boxed_str())
    }
}
impl From<EnumValueId> for Value {
    fn from(e: EnumValueId) -> Value {
        Value::Enum(e)
    }
}
impl From<Mrid> for Value {
    fn from(m: Mrid) -> Value {
        Value::Reference(m)
    }
}
impl From<Compound> for Value {
    fn from(c: Compound) -> Value {
        Value::Compound(Box::new(c))
    }
}

/// Parse a float, accepting the forms that appear in published exchange files.
///
/// Two deviations are tolerated and a third is deliberately not.
///
/// A Fortran-style exponent (`1.5D+3`) is unambiguous, so it is accepted. A comma decimal
/// mark is *not* unambiguous: `1,234` is `1.234` to a producer with a European locale and
/// `1234` to one writing a thousands separator, and the two readings differ by a factor of
/// a thousand on a quantity like a line impedance. It is accepted only where the thousands
/// reading is impossible — a single comma, no decimal point, and a group of digits after it
/// that is not three long. Anything still ambiguous is refused, which surfaces as a
/// [`Rule::InvalidValue`](crate::Rule::InvalidValue) diagnostic naming the text, rather
/// than as a number that is silently wrong by 1000×.
fn parse_float(t: &str) -> Option<f64> {
    if let Ok(f) = t.parse::<f64>() {
        return Some(f);
    }
    let normalized = t.replace(['D', 'd'], "E");
    if let Ok(f) = normalized.parse::<f64>() {
        return Some(f);
    }
    let (_, tail) = normalized.split_once(',')?;
    let unambiguous = !normalized.contains('.')
        && !tail.contains(',')
        && tail.chars().take_while(char::is_ascii_digit).count() != 3;
    unambiguous
        .then(|| normalized.replace(',', ".").parse::<f64>().ok())
        .flatten()
}

/// Format a float in the shortest form that round-trips exactly.
///
/// `NaN`, `INF` and `-INF` are the lexical forms XML Schema defines for the floating-point
/// types, which is what CIM `Float` becomes in RDF — see [`Primitive::xsd_datatype`].
///
/// Rust's `Display` for `f64` is shortest-round-trip but never uses exponent notation, so
/// it renders `1.5e300` as a 301-digit integer and `1e-9` as `0.000000001`. Both are legal
/// `xsd:double` and both re-read exactly, but neither resembles what the producer wrote,
/// and the first is a denial-of-service-shaped output amplification. Values outside the
/// range where plain decimal stays compact are therefore written in exponent form, which
/// is also shortest-round-trip because `LowerExp` shares the same algorithm.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF" } else { "-INF" }.to_owned();
    }
    let abs = f.abs();
    // The bounds are where `{}` stops being the shorter rendering of the two.
    if abs != 0.0 && !(1e-5..1e16).contains(&abs) {
        return format!("{f:E}");
    }
    plain_decimal(f)
}

/// Format a float without an exponent, which is all `xsd:decimal` allows.
///
/// Rust's `Display` for `f64` never uses exponent notation, so it is already the plain
/// form; only the two spellings of zero need normalizing.
fn plain_decimal(f: f64) -> String {
    if !f.is_finite() {
        // Neither `NaN` nor `INF` is in the decimal lexical space, but losing the value
        // would be worse than writing what it is.
        return format_float(f);
    }
    let s = f.to_string();
    if s == "-0" { "0".to_owned() } else { s }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseValueError {
    pub expected: Primitive,
    pub text: String,
}

impl ParseValueError {
    fn new(expected: Primitive, text: &str) -> Self {
        ParseValueError {
            expected,
            // Keep diagnostics bounded even if a file contains a huge text node.
            text: text.chars().take(64).collect(),
        }
    }
}

impl fmt::Display for ParseValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot parse {:?} from {:?}", self.expected, self.text)
    }
}

impl std::error::Error for ParseValueError {}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Boolean(b) => write!(f, "{b}"),
            Value::Integer(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Text(s) => write!(f, "{s:?}"),
            Value::Enum(e) => write!(f, "enum#{}", e.0),
            Value::Reference(m) => write!(f, "->{m}"),
            Value::Compound(c) => write!(f, "{{{} fields}}", c.len()),
        }
    }
}

/// Human-readable rendering that resolves enumeration literals through `schema`.
pub fn display_value(schema: &Schema, v: &Value) -> String {
    match v {
        Value::Enum(e) => schema.enum_value(*e).name.to_owned(),
        Value::Reference(m) => m.canonical(),
        Value::Compound(c) => {
            let fields: Vec<String> = c
                .values()
                .iter()
                .map(|(a, v)| format!("{}={}", schema.attr(*a).label, display_value(schema, v)))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
        other => other.to_lexical().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A number's spelling is information: the published corpus writes `2.62637E-05` and
    /// `250.000000`, and re-exporting those as `0.0000262637` and `250` changes 8,623
    /// values in a model set that is supposed to come back as it went in.
    #[test]
    fn a_documents_spelling_of_a_number_survives_a_round_trip() {
        for (text, prim) in [
            ("2.62637E-05", Primitive::Float),
            ("0e+000", Primitive::Float),
            ("250.000000", Primitive::Float),
            ("-330.750000", Primitive::Float),
            ("6.8E-08", Primitive::Float),
            ("1.0", Primitive::Float),
            ("250.000000", Primitive::Decimal),
        ] {
            let v = Value::parse_primitive(prim, text).unwrap();
            assert_eq!(
                v.to_lexical_as(prim).as_deref(),
                Some(text),
                "{text:?} as {prim:?}"
            );
        }
    }

    /// Spelling is not value. Two models that differ only in how a producer formatted a
    /// number are the same model — otherwise every difference report would fill with
    /// changes nobody made.
    #[test]
    fn spelling_is_not_part_of_the_value() {
        let written = Value::parse_primitive(Primitive::Float, "250.000000").unwrap();
        let plain = Value::from(250.0);
        assert_eq!(written, plain);
        assert_eq!(written.as_f64(), Some(250.0));
    }

    /// The reader accepts forms that published files deviate with; the writer must not
    /// hand them on, because output this crate promises is valid has to be valid.
    #[test]
    fn a_spelling_is_only_kept_when_it_is_a_conforming_lexical_form() {
        // A comma decimal mark parses (where it is unambiguous) and is repaired on the
        // way out, not echoed.
        let v = Value::parse_primitive(Primitive::Float, "1,5").unwrap();
        assert_eq!(v.as_f64(), Some(1.5));
        assert_eq!(v.to_lexical().as_deref(), Some("1.5"));
        // A Fortran exponent, likewise — and 1500 is inside the range this crate writes
        // plainly, so the canonical form is not an exponent at all.
        let v = Value::parse_primitive(Primitive::Float, "1.5D+3").unwrap();
        assert_eq!(v.to_lexical().as_deref(), Some("1500"));
        // `xsd:decimal` has no exponent notation, so a float spelling that uses one is not
        // reused when the profile types the attribute as a decimal.
        let v = Value::parse_primitive(Primitive::Float, "6.8E-08").unwrap();
        assert_eq!(
            v.to_lexical_as(Primitive::Decimal).as_deref(),
            Some("0.000000068")
        );
    }

    /// The fast path may only ever answer "canonical" where the exact comparison would.
    #[test]
    fn the_shape_test_never_disagrees_with_formatting() {
        let texts = [
            "0",
            "1",
            "-1",
            "250",
            "2.2",
            "-330.75",
            "0.0000262637",
            "1e5",
            "1E5",
            "0e+000",
            "250.000000",
            "-0",
            "007",
            "1.0",
            "0.10",
            "+1",
            "1.5D+3",
            "1,5",
            "12345678901234567",
            "0.30000000000000004",
            "1.7976931348623157E308",
            "5E-324",
            "-2.62637E-05",
        ];
        for text in texts {
            let Some(v) = parse_float(text) else { continue };
            for prim in [Primitive::Float, Primitive::Decimal] {
                if is_canonical_plain(v, text) {
                    assert_eq!(
                        text,
                        canonical(v, prim),
                        "{text:?} claimed canonical for {prim:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn parses_the_primitive_forms_used_in_exchange_files() {
        assert_eq!(
            Value::parse_primitive(Primitive::Boolean, "true").unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            Value::parse_primitive(Primitive::Boolean, " false ").unwrap(),
            Value::Boolean(false)
        );
        assert_eq!(
            Value::parse_primitive(Primitive::Integer, "42").unwrap(),
            Value::Integer(42)
        );
        assert_eq!(
            Value::parse_primitive(Primitive::Float, "-67.2335").unwrap(),
            Value::from(-67.2335)
        );
        assert_eq!(
            Value::parse_primitive(Primitive::Float, "1.5E3").unwrap(),
            Value::from(1500.0)
        );
    }

    #[test]
    fn tolerates_non_conforming_numeric_forms() {
        assert_eq!(
            Value::parse_primitive(Primitive::Float, "1.5D+3").unwrap(),
            Value::from(1500.0)
        );
        // Integral value written with a decimal point.
        assert_eq!(
            Value::parse_primitive(Primitive::Integer, "7.0").unwrap(),
            Value::Integer(7)
        );
    }

    #[test]
    fn rejects_unparseable_values() {
        assert!(Value::parse_primitive(Primitive::Boolean, "yes").is_err());
        assert!(Value::parse_primitive(Primitive::Float, "abc").is_err());
    }

    /// A comma is a decimal mark to one producer and a thousands separator to another, and
    /// the two readings differ by a factor of a thousand. Guessing is worse than refusing.
    #[test]
    fn a_comma_is_only_read_as_a_decimal_mark_where_it_cannot_be_a_thousands_separator() {
        let f = |s: &str| Value::parse_primitive(Primitive::Float, s);
        // Not three digits after the comma: no locale writes a thousands group that way.
        assert_eq!(f("-67,2335").unwrap(), Value::from(-67.2335));
        assert_eq!(f("1,5").unwrap(), Value::from(1.5));
        assert_eq!(f("1,5E3").unwrap(), Value::from(1500.0));
        // Exactly three: `1,234` is 1.234 or 1234 and nothing in the document says which.
        assert!(f("1,234").is_err(), "1,234 is ambiguous");
        assert!(f("1,000").is_err(), "1,000 is ambiguous");
        // Both marks, or two commas: a grouped number, which CIM/XML never writes.
        assert!(f("1,234.5").is_err());
        assert!(f("1,234,567").is_err());
    }

    #[test]
    fn ordinary_rust_values_convert_to_the_right_variant() {
        assert_eq!(Value::from(true), Value::Boolean(true));
        assert_eq!(Value::from(7i64), Value::Integer(7));
        assert_eq!(Value::from(2.2f64), Value::from(2.2));
        assert_eq!(Value::from("BE-Line_1"), Value::Text("BE-Line_1".into()));
        assert_eq!(
            Value::from(String::from("owned")),
            Value::Text("owned".into())
        );
        let m = Mrid::parse("70c4656c-f7a0-4319-98bb-84fb5e2e9b37");
        assert_eq!(Value::from(m.clone()), Value::Reference(m));
    }

    #[test]
    fn strings_keep_surrounding_whitespace() {
        // Names may legitimately contain leading/trailing spaces; only numeric and
        // boolean parsing trims.
        assert_eq!(
            Value::parse_primitive(Primitive::String, " NL LoadArea ").unwrap(),
            Value::Text(" NL LoadArea ".into())
        );
    }

    #[test]
    fn float_formatting_round_trips() {
        for f in [
            0.0,
            -0.0,
            1.0,
            -67.2335,
            1e-7,
            1.5e300,
            8.953211,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            1e15,
            1e16,
            1e-5,
            1e-6,
        ] {
            let s = format_float(f);
            assert_eq!(
                s.parse::<f64>().unwrap(),
                if f == 0.0 { 0.0 } else { f },
                "{f} rendered as {s}"
            );
        }
        assert_eq!(format_float(-0.0), "0");
    }

    #[test]
    fn extreme_floats_use_exponent_notation_rather_than_hundreds_of_digits() {
        // `f64::to_string` never uses an exponent, so it renders 1.5e300 as 301 digits.
        assert_eq!(format_float(1.5e300), "1.5E300");
        assert_eq!(format_float(1e-9), "1E-9");
        assert!(format_float(f64::MAX).len() < 24);
        // The ordinary range keeps its plain decimal form.
        assert_eq!(format_float(-67.2335), "-67.2335");
        assert_eq!(format_float(0.0001), "0.0001");
    }

    #[test]
    fn non_finite_floats_use_the_xml_schema_lexical_forms() {
        assert_eq!(format_float(f64::NAN), "NaN");
        assert_eq!(format_float(f64::INFINITY), "INF");
        assert_eq!(format_float(f64::NEG_INFINITY), "-INF");
        // And they read back.
        for s in ["NaN", "INF", "-INF"] {
            assert!(
                Value::parse_primitive(Primitive::Float, s).is_ok(),
                "{s} should parse"
            );
        }
    }
}
