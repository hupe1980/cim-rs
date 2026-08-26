//! Master resource identifiers.
//!
//! IEC 61970-552 requires every CIM object to be identified by a UUID, serialized as
//! `rdf:ID="_<uuid>"` and referenced as `rdf:resource="#_<uuid>"`, with model headers
//! using the `urn:uuid:<uuid>` form. Real exchange files do not always comply, so an
//! [`Mrid`] preserves any identifier verbatim while storing conforming ones compactly.

use std::fmt;

/// A master resource identifier.
///
/// Conforming identifiers are stored as 16 raw UUID bytes; anything else is preserved
/// as-is so that reading and re-writing a non-conforming file is lossless.
///
/// Comparison is over the canonical form, so `_ABC…`, `#_abc…` and `urn:uuid:abc…`
/// all denote the same object.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mrid(Repr);

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Repr {
    /// A well-formed UUID, stored as raw bytes (align 1, keeping `Mrid` at 24 bytes).
    Uuid([u8; 16]),
    /// A non-conforming identifier, preserved verbatim.
    Other(Box<str>),
}

impl Mrid {
    /// Parse an identifier in any of the forms CIM/XML uses.
    ///
    /// Accepts `<uuid>`, `_<uuid>`, `#_<uuid>`, `#<uuid>` and `urn:uuid:<uuid>`, as well
    /// as arbitrary strings (kept verbatim, reported by [`Mrid::is_uuid`] as `false`).
    pub fn parse(raw: &str) -> Mrid {
        let core = strip_decoration(raw);
        match parse_uuid(core) {
            Some(bytes) => Mrid(Repr::Uuid(bytes)),
            None => Mrid(Repr::Other(raw.into())),
        }
    }

    /// Build from raw UUID bytes.
    pub const fn from_uuid_bytes(bytes: [u8; 16]) -> Mrid {
        Mrid(Repr::Uuid(bytes))
    }

    /// Whether this identifier conforms to IEC 61970-552 (a syntactically valid UUID).
    pub fn is_uuid(&self) -> bool {
        matches!(self.0, Repr::Uuid(_))
    }

    /// Raw UUID bytes, if conforming.
    pub fn as_uuid_bytes(&self) -> Option<&[u8; 16]> {
        match &self.0 {
            Repr::Uuid(b) => Some(b),
            Repr::Other(_) => None,
        }
    }

    /// Canonical lowercase hyphenated form without decoration, e.g. `70c4656c-f7a0-…`.
    ///
    /// Non-conforming identifiers are returned verbatim.
    pub fn canonical(&self) -> String {
        match &self.0 {
            Repr::Uuid(b) => format_uuid(b),
            Repr::Other(s) => s.to_string(),
        }
    }

    /// The `rdf:ID` form: a leading underscore, as mandated for locally defined objects.
    pub fn to_rdf_id(&self) -> String {
        match &self.0 {
            Repr::Uuid(b) => format!("_{}", format_uuid(b)),
            // Preserve verbatim; adding an underscore would change a foreign identifier.
            Repr::Other(s) => s.to_string(),
        }
    }

    /// The `rdf:resource` form for a same-document reference: `#_<uuid>`.
    pub fn to_rdf_resource(&self) -> String {
        format!("#{}", self.to_rdf_id())
    }

    /// The `urn:uuid:` form used by `md:FullModel` identifiers.
    pub fn to_urn(&self) -> String {
        match &self.0 {
            Repr::Uuid(b) => format!("urn:uuid:{}", format_uuid(b)),
            Repr::Other(s) => s.to_string(),
        }
    }
}

/// Remove the decorations CIM/XML puts around identifiers.
fn strip_decoration(raw: &str) -> &str {
    let s = raw.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    // `urn:uuid:` is case-insensitive per RFC 8141.
    let s = if s.len() >= 9 && s[..9].eq_ignore_ascii_case("urn:uuid:") {
        &s[9..]
    } else {
        s
    };
    s.strip_prefix('_').unwrap_or(s)
}

fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    // 8-4-4-4-12 hyphenated form; the only form 61970-552 permits.
    if s.len() != 36 {
        return None;
    }
    let b = s.as_bytes();
    if b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
        return None;
    }
    let mut out = [0u8; 16];
    let mut oi = 0;
    let mut i = 0;
    while i < 36 {
        if b[i] == b'-' {
            i += 1;
            continue;
        }
        let hi = hex(b[i])?;
        let lo = hex(*b.get(i + 1)?)?;
        out[oi] = (hi << 4) | lo;
        oi += 1;
        i += 2;
    }
    (oi == 16).then_some(out)
}

const fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn format_uuid(b: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push(HEX[(byte >> 4) as usize] as char);
        s.push(HEX[(byte & 0x0f) as usize] as char);
    }
    s
}

impl fmt::Display for Mrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

impl fmt::Debug for Mrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Mrid({})", self.canonical())
    }
}

impl From<&str> for Mrid {
    fn from(s: &str) -> Mrid {
        Mrid::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANON: &str = "70c4656c-f7a0-4319-98bb-84fb5e2e9b37";

    #[test]
    fn all_serialized_forms_are_the_same_identifier() {
        let forms = [
            CANON,
            "_70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "#_70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "#70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "urn:uuid:70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "URN:UUID:70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            // 61970-552 mandates lowercase, but uppercase occurs in the wild.
            "_70C4656C-F7A0-4319-98BB-84FB5E2E9B37",
        ];
        for f in forms {
            let m = Mrid::parse(f);
            assert!(m.is_uuid(), "{f} should parse as a UUID");
            assert_eq!(m.canonical(), CANON, "{f}");
            assert_eq!(m, Mrid::parse(CANON), "{f}");
        }
    }

    #[test]
    fn serialization_forms_round_trip() {
        let m = Mrid::parse(CANON);
        assert_eq!(m.to_rdf_id(), format!("_{CANON}"));
        assert_eq!(m.to_rdf_resource(), format!("#_{CANON}"));
        assert_eq!(m.to_urn(), format!("urn:uuid:{CANON}"));
        assert_eq!(Mrid::parse(&m.to_rdf_resource()), m);
    }

    #[test]
    fn non_conforming_identifiers_are_preserved_verbatim() {
        for raw in ["not-a-uuid", "12345", "", "urn:uuid:garbage"] {
            let m = Mrid::parse(raw);
            assert!(!m.is_uuid(), "{raw:?}");
            assert_eq!(m.canonical(), raw);
            // Round-tripping must not invent decoration for foreign identifiers.
            assert_eq!(m.to_rdf_id(), raw);
        }
    }

    #[test]
    fn malformed_uuids_do_not_parse_as_uuid() {
        for raw in [
            "70c4656c-f7a0-4319-98bb-84fb5e2e9b3",   // too short
            "70c4656c-f7a0-4319-98bb-84fb5e2e9b377", // too long
            "70c4656cxf7a0-4319-98bb-84fb5e2e9b37",  // wrong separator
            "70c4656c-f7a0-4319-98bb-84fb5e2e9bzz",  // non-hex
        ] {
            assert!(!Mrid::parse(raw).is_uuid(), "{raw}");
        }
    }
}
