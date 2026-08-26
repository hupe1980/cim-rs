//! Attribute values.

use crate::mrid::Mrid;
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

    /// Render in the lexical form CIM/XML uses for element text.
    ///
    /// Only meaningful for values serialized as text; enumerations and references are
    /// written as `rdf:resource` IRIs instead and return `None`.
    pub fn to_lexical(&self) -> Option<String> {
        Some(match self {
            Value::Boolean(b) => if *b { "true" } else { "false" }.to_owned(),
            Value::Integer(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Text(s) => s.to_string(),
            Value::Enum(_) | Value::Reference(_) => return None,
        })
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

/// Parse a float, accepting the forms that appear in published exchange files.
fn parse_float(t: &str) -> Option<f64> {
    if let Ok(f) = t.parse::<f64>() {
        return Some(f);
    }
    // Some exporters emit Fortran-style exponents ("1.5D+3") or a comma decimal mark.
    let normalized = t.replace(['D', 'd'], "E").replace(',', ".");
    normalized.parse::<f64>().ok()
}

/// Format a float in the shortest form that round-trips exactly.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF" } else { "-INF" }.to_owned();
    }
    // Rust's default float formatting is already shortest-round-trip.
    let s = f.to_string();
    // `-0` and `0` both denote zero; normalize to keep output stable.
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
        }
    }
}

/// Human-readable rendering that resolves enumeration literals through `schema`.
pub fn display_value(schema: &Schema, v: &Value) -> String {
    match v {
        Value::Enum(e) => schema.enum_value(*e).name.to_owned(),
        Value::Reference(m) => m.canonical(),
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
        for f in [0.0, -0.0, 1.0, -67.2335, 1e-7, 1.5e300, 8.953211] {
            let s = format_float(f);
            assert_eq!(
                s.parse::<f64>().unwrap(),
                if f == 0.0 { 0.0 } else { f },
                "{f}"
            );
        }
        assert_eq!(format_float(-0.0), "0");
    }
}
