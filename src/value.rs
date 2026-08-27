//! Attribute values.

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
    Float(f64),
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
            Value::Float(f) => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
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
            Value::Float(f) => format_float(*f),
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
        match (prim, self) {
            (Primitive::Decimal, Value::Float(f)) => Some(plain_decimal(*f)),
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
                Value::Float(parse_float(t).ok_or_else(|| ParseValueError::new(prim, text))?)
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
// `Object::set(attrs::ac_line_segment::r, Value::Float(2.2))`, and the generated attribute
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
        Value::Float(f)
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
            Value::Float(-67.2335)
        );
        assert_eq!(
            Value::parse_primitive(Primitive::Float, "1.5E3").unwrap(),
            Value::Float(1500.0)
        );
    }

    #[test]
    fn tolerates_non_conforming_numeric_forms() {
        assert_eq!(
            Value::parse_primitive(Primitive::Float, "1.5D+3").unwrap(),
            Value::Float(1500.0)
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
        assert_eq!(f("-67,2335").unwrap(), Value::Float(-67.2335));
        assert_eq!(f("1,5").unwrap(), Value::Float(1.5));
        assert_eq!(f("1,5E3").unwrap(), Value::Float(1500.0));
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
        assert_eq!(Value::from(2.2f64), Value::Float(2.2));
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
