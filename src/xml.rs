//! What XML and RDF/XML can actually represent.
//!
//! Two constraints of the output syntaxes are not expressible in the object model, so
//! nothing upstream of serialization enforces them and every layer downstream assumes them.
//!
//! **Not every character can appear in an XML document.** XML 1.0 defines the `Char`
//! production, and a character outside it — a NUL, a `SOH`, most of the C0 range — is not
//! merely in need of escaping: there is *no* representation for it, numeric character
//! reference included. A value carrying one makes the whole document unparseable, and
//! `quick-xml` neither rejects it on the way in nor notices it on the way out, so a corrupt
//! or mis-encoded source file is the only place one comes from and this is the only place
//! it is caught.
//!
//! **`rdf:ID` is an XML `NCName`.** IEC 61970-552 identifiers are UUIDs, and `_` followed
//! by a UUID is always a valid one — but this crate deliberately keeps non-conforming
//! identifiers verbatim, and a published file may identify an object with anything at all.
//! `rdf:ID="_http://host/EQ.xml#Sub1"` is well-formed XML, so a well-formedness check
//! cannot see it, and is not valid RDF/XML, so every RDF toolchain rejects it. The writer
//! asks [`identifier_form`] which of the two serializations an identifier can carry.

/// Whether `c` is in XML 1.0's `Char` production.
///
/// Everything outside it is unrepresentable rather than merely in need of escaping: XML
/// forbids the character *and* a numeric reference to it, so a document containing one
/// cannot be repaired by escaping. Rust's `char` cannot hold a surrogate, so the surrogate
/// range needs no test.
#[inline]
pub const fn is_xml_char(c: char) -> bool {
    matches!(c, '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

/// The first character of `s` that XML 1.0 cannot represent, with its byte offset.
///
/// The scan is guarded by a one-byte-per-byte test, because the answer is `None` for every
/// value in a well-formed model and this runs on every value read and every value written.
/// Every illegal character is either a C0 control other than tab, newline and carriage
/// return, or one of `U+FFFE`/`U+FFFF`, whose UTF-8 encodings begin `EF BF`.
pub fn find_illegal(s: &str) -> Option<(usize, char)> {
    if !s
        .bytes()
        .any(|b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')) || b == 0xEF)
    {
        return None;
    }
    s.char_indices().find(|&(_, c)| !is_xml_char(c))
}

/// Whether `s` contains a character XML 1.0 cannot represent.
#[inline]
pub fn has_illegal(s: &str) -> bool {
    find_illegal(s).is_some()
}

/// `s` with every character XML cannot represent removed.
///
/// Borrows unchanged where there is nothing to remove, which is every value in a
/// well-formed model. Removal is the only option available: the characters concerned have
/// no escaped form, so the choice is between dropping them and writing a document nothing
/// can read. The loss is reported rather than silent — the reader raises
/// [`Rule::IllegalXmlCharacter`](crate::Rule::IllegalXmlCharacter) where the value enters,
/// and [`validate`](mod@crate::validate) raises it for a value already in the store.
pub fn strip_illegal(s: &str) -> std::borrow::Cow<'_, str> {
    match has_illegal(s) {
        false => std::borrow::Cow::Borrowed(s),
        true => std::borrow::Cow::Owned(s.chars().filter(|&c| is_xml_char(c)).collect()),
    }
}

/// Render a character the way a diagnostic should name it.
///
/// The character itself must not go into the message: a diagnostic is printed, logged and
/// put in a CI annotation, and embedding a control character in it spreads the problem
/// rather than describing it.
pub fn describe_char(c: char) -> String {
    format!("U+{:04X}", c as u32)
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Whether `s` is an XML `NCName` — a `Name` with no colon.
///
/// This is what `rdf:ID` requires. The character classes are XML 1.0's `NameStartChar` and
/// `NameChar` minus `:`.
pub fn is_ncname(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_name_start_char) && chars.all(is_name_char)
}

/// XML 1.0's `NameStartChar`, minus `:`.
const fn is_name_start_char(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z' | '_'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// XML 1.0's `NameChar`, minus `:`.
const fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}'
            | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// Whether `s` is an absolute IRI: a scheme, a colon, and something after it.
///
/// Deliberately a shape test rather than a parse. It answers one question — may this be
/// written into `rdf:about` unchanged — and for that the scheme grammar of RFC 3987 plus
/// the absence of characters an IRI reference forbids is exactly the condition.
pub fn is_absolute_iri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let scheme_ok = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    // One fragment at most, and none of the characters an IRI reference may not hold.
    scheme_ok
        && !rest.is_empty()
        && s.matches('#').count() <= 1
        && !s.chars().any(|c| {
            (c as u32) <= 0x20
                || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '\\' | '^' | '`')
                || !is_xml_char(c)
        })
}

/// How an identifier can be written into a CIM/XML document.
///
/// IEC 61970-552 identifiers are UUIDs and every UUID answers [`IdentifierForm::Name`], so
/// this distinction is invisible on conforming input. It exists for the input that is not
/// conforming, which this crate keeps verbatim on purpose — see `Mrid`'s documentation for
/// why an identifier's spelling is preserved rather than normalized away.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdentifierForm {
    /// `_<id>` is an `NCName`, so `rdf:ID`/`rdf:about="#_<id>"` are both available.
    Name,
    /// Not a name, but an absolute IRI, so `rdf:about="<id>"` states it exactly.
    ///
    /// This is the realistic non-conforming case: a producer writes a cross-document
    /// reference as `rdf:about="http://host/EQ.xml#Sub1"`, whose fragment is not a UUID, so
    /// the identifier stays opaque in one piece and cannot become an `NCName`.
    Iri,
    /// Neither. No conforming RDF/XML serialization of this identifier exists.
    ///
    /// The writer still emits the identifier — losing the object would be worse, and the
    /// document remains well-formed XML — but the result is not valid RDF/XML, and
    /// [`validate`](mod@crate::validate) says so as
    /// [`Rule::UnserializableIdentifier`](crate::Rule::UnserializableIdentifier).
    Unwritable,
}

/// Which serializations `id` — an identifier stripped of CIM/XML decoration — can carry.
pub fn identifier_form(id: &str) -> IdentifierForm {
    // `rdf:ID` carries the identifier under a leading underscore, which is what makes a
    // UUID a name at all: `NCName` may not begin with a digit.
    if !id.is_empty() && !id.contains(':') && is_ncname_tail(id) {
        return IdentifierForm::Name;
    }
    if is_absolute_iri(id) {
        return IdentifierForm::Iri;
    }
    IdentifierForm::Unwritable
}

/// Whether every character of `s` may follow the `_` that `rdf:ID` prefixes.
///
/// `_` is itself a `NameStartChar`, so only the tail is in question and the prefixed string
/// need not be built.
fn is_ncname_tail(s: &str) -> bool {
    s.chars().all(is_name_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_char_production_admits_text_and_refuses_the_c0_range() {
        for legal in [
            '\t', '\n', '\r', ' ', 'a', 'é', '\u{D7FF}', '\u{E000}', '🙂',
        ] {
            assert!(is_xml_char(legal), "{legal:?}");
        }
        for illegal in ['\u{0}', '\u{1}', '\u{8}', '\u{B}', '\u{C}', '\u{1F}'] {
            assert!(!is_xml_char(illegal), "{illegal:?}");
        }
        // The two non-characters at the end of the BMP are excluded too.
        assert!(!is_xml_char('\u{FFFE}'));
        assert!(!is_xml_char('\u{FFFF}'));
        assert!(is_xml_char('\u{FFFD}'));
    }

    #[test]
    fn illegal_characters_are_found_at_their_byte_offset() {
        assert_eq!(find_illegal("plain text"), None);
        // Tab, newline and carriage return are legal and must not trip the fast path.
        assert_eq!(find_illegal("a\tb\nc\rd"), None);
        // Nor must ordinary non-ASCII, whose lead bytes include the one the scan watches.
        assert_eq!(find_illegal("naïve ïnput ﬀ"), None);
        assert_eq!(find_illegal("ab\u{0}c"), Some((2, '\u{0}')));
        // The offset is a byte offset, so it counts the multi-byte character before it.
        assert_eq!(find_illegal("é\u{1}"), Some((2, '\u{1}')));
        assert_eq!(find_illegal("x\u{FFFF}"), Some((1, '\u{FFFF}')));
    }

    #[test]
    fn stripping_removes_only_what_cannot_be_written_and_borrows_otherwise() {
        assert!(matches!(
            strip_illegal("clean"),
            std::borrow::Cow::Borrowed("clean")
        ));
        assert_eq!(strip_illegal("bad\u{0}\u{8}name"), "badname");
        assert_eq!(
            strip_illegal("keep\ttabs\nand\rreturns"),
            "keep\ttabs\nand\rreturns"
        );
    }

    #[test]
    fn ncname_follows_the_xml_names_recommendation() {
        for ok in ["_abc", "a", "A-1.2", "_1fa19c28-1c8f-4e1e", "é"] {
            assert!(is_ncname(ok), "{ok}");
        }
        for bad in ["", "1abc", "-abc", "a:b", "a b", "a/b", "a#b", ".abc"] {
            assert!(!is_ncname(bad), "{bad}");
        }
    }

    #[test]
    fn absolute_iris_are_recognised_without_being_parsed() {
        for ok in [
            "http://host/EQ.xml#Sub1",
            "urn:uuid:70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "file:///models/EQ.xml#x",
        ] {
            assert!(is_absolute_iri(ok), "{ok}");
        }
        for bad in [
            "Substation1",    // no scheme
            "1http://x",      // scheme may not start with a digit
            "http://a b",     // space
            "http://a#b#c",   // two fragments
            "http://a\u{0}b", // unrepresentable character
            ":no-scheme",
        ] {
            assert!(!is_absolute_iri(bad), "{bad}");
        }
    }

    /// The classification the writer acts on, over the identifier shapes real files carry.
    #[test]
    fn identifiers_are_classified_by_what_can_be_written() {
        // A UUID, in either spelling, is always a name once `rdf:ID`'s underscore is added.
        for uuid in [
            "70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "1fa19c281c8f4e1eaad9e1cab70f923e",
        ] {
            assert_eq!(identifier_form(uuid), IdentifierForm::Name, "{uuid}");
        }
        // Ordinary opaque identifiers are names too, which is why this was never noticed.
        // A leading digit is fine for the same reason a UUID is: `rdf:ID` writes the
        // underscore, and `NCName` only forbids a digit in the *first* position.
        for name in ["Substation1", "BE-Line_1", "a.b-c", "1leading-digit"] {
            assert_eq!(identifier_form(name), IdentifierForm::Name, "{name}");
        }
        // A cross-document reference whose fragment is not a UUID: not a name, but an IRI.
        assert_eq!(
            identifier_form("http://host/EQ.xml#Sub1"),
            IdentifierForm::Iri
        );
        // A colon alone disqualifies a name, since `NCName` is `Name` without one.
        assert_eq!(identifier_form("a:b"), IdentifierForm::Iri);
        // And what is neither is admitted to be neither, rather than written as a name.
        for bad in ["has space", "a#b#c", "", "a/b"] {
            assert_eq!(identifier_form(bad), IdentifierForm::Unwritable, "{bad}");
        }
    }
}
