//! Master resource identifiers.
//!
//! IEC 61970-552 requires every CIM object to be identified by a UUID, serialized as
//! `rdf:ID="_<uuid>"` and referenced as `rdf:resource="#_<uuid>"`, with model headers
//! using the `urn:uuid:<uuid>` form. Real exchange files do not always comply, so an
//! [`Mrid`] preserves any identifier verbatim while storing conforming ones compactly.

use std::fmt;
use std::hash::{Hash, Hasher};

/// How an identifier was spelled in the document it came from.
///
/// The distinction exists because "is this a UUID" and "is this written the way
/// IEC 61970-552 requires" are different questions with different consequences, and
/// conflating them costs real interoperability. A published CGMES 2.4.15 boundary file
/// writes `rdf:ID="_1fa19c281c8f4e1eaad9e1cab70f923e"` — that *is* the UUID
/// `1fa19c28-1c8f-4e1e-aad9-e1cab70f923e`, so it has a `urn:uuid:` form and belongs in an
/// RDF graph as an IRI; only its spelling deviates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MridForm {
    /// The hyphenated `8-4-4-4-12` form IEC 61970-552 requires.
    Uuid,
    /// Thirty-two hex digits with no hyphens: a UUID value, non-conforming spelling.
    Compact,
    /// Not a UUID at all. Kept verbatim.
    Opaque,
}

/// A master resource identifier.
///
/// Identifiers that denote a UUID are stored as 16 raw bytes together with the spelling
/// they arrived in; anything else is kept as text. Either way the *decoration* CIM/XML
/// puts around an identifier — a leading `#`, a leading `_`, a `urn:uuid:` prefix — is
/// stripped on parse, so `_abc`, `#_abc` and `urn:uuid:abc` all denote the same object.
///
/// Stripping applies to non-conforming identifiers too. It has to: a file using
/// `rdf:ID="_X"` and `rdf:resource="#_X"` for a non-UUID `X` would otherwise produce two
/// distinct identifiers and every reference in it would dangle.
///
/// # Spelling is remembered, identity is not spelled
///
/// Two `Mrid`s denoting the same UUID are equal, hash alike and order alike **whatever
/// spelling they arrived in**, so a boundary file writing `_1fa19c28…923e` and an equipment
/// file referencing `#_1fa19c28-1c8f-…-923e` resolve to one object. The spelling is carried
/// only so that writing the model back reproduces the document it came from:
/// [`Mrid::to_rdf_id`] echoes the original form while [`Mrid::canonical`] and
/// [`Mrid::to_urn`] always give the hyphenated one, which is the only form RFC 4122 and
/// RFC 8141 define.
#[derive(Clone)]
pub struct Mrid(Repr);

#[derive(Clone)]
enum Repr {
    /// A UUID value, stored as raw bytes (align 1, keeping `Mrid` at 24 bytes) with the
    /// spelling it was read in.
    ///
    /// Two things about that spelling deviate in published files and a re-export has to
    /// reproduce both: whether the hyphens were left out, and how the hex digits were cased.
    /// Case is a bitmask over the 32 digit positions rather than a flag, because the CGMES
    /// 2.4.15 boundary set is *mixed* — `_24C12434-E42B-497f-928F-119C6AE92079` has a
    /// lower-case `497f` in otherwise upper-case hex. The `u32` costs nothing: the pointer
    /// in the other variant already forced the alignment it pads into.
    Uuid {
        bytes: [u8; 16],
        /// Bit `i` set means hex digit `i` was written upper case.
        case: u32,
        compact: bool,
    },
    /// A non-conforming identifier, stripped of CIM/XML decoration.
    Other(Box<str>),
}

/// Identity ignores the spelling: the same UUID written two ways is one object.
///
/// `Ord` keeps UUIDs before opaque identifiers and orders UUIDs by their bytes, which is
/// what makes the writer's output order independent of how a producer spelled things.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Key<'a> {
    Uuid(&'a [u8; 16]),
    Other(&'a str),
}

impl Mrid {
    #[inline]
    fn key(&self) -> Key<'_> {
        match &self.0 {
            Repr::Uuid { bytes, .. } => Key::Uuid(bytes),
            Repr::Other(s) => Key::Other(s),
        }
    }

    /// Parse an identifier in any of the forms CIM/XML uses.
    ///
    /// Accepts `<uuid>`, `_<uuid>`, `#_<uuid>`, `#<uuid>` and `urn:uuid:<uuid>` in either
    /// the hyphenated or the compact spelling, as well as arbitrary strings (kept verbatim,
    /// reported by [`Mrid::form`] as [`MridForm::Opaque`]).
    ///
    /// An **absolute IRI whose fragment is a UUID** — `http://host/doc#_<uuid>` — denotes
    /// that UUID. Producers write these when a reference crosses documents, and reading the
    /// whole IRI as opaque text is the mistake it looks like it is not: the object is in the
    /// model under its UUID, so the reference would dangle against an object that is right
    /// there. Only the fragment decides; a `#` in an identifier that is not followed by a
    /// UUID leaves the whole string opaque, so nothing is invented.
    pub fn parse(raw: &str) -> Mrid {
        let core = strip_decoration(raw);
        if let Some(m) = parse_uuid(core) {
            return Mrid(m);
        }
        if let Some((_, fragment)) = core.rsplit_once('#')
            && let Some(m) = parse_uuid(fragment.strip_prefix('_').unwrap_or(fragment))
        {
            return Mrid(m);
        }
        Mrid(Repr::Other(core.into()))
    }

    /// Build from raw UUID bytes, spelled the way IEC 61970-552 requires.
    pub const fn from_uuid_bytes(bytes: [u8; 16]) -> Mrid {
        Mrid(Repr::Uuid {
            bytes,
            case: 0,
            compact: false,
        })
    }

    /// The all-zero UUID, usable in a `const` or `static`.
    pub const fn nil() -> Mrid {
        Mrid::from_uuid_bytes([0u8; 16])
    }

    /// How this identifier was spelled.
    pub fn form(&self) -> MridForm {
        match &self.0 {
            Repr::Uuid { compact: false, .. } => MridForm::Uuid,
            Repr::Uuid { compact: true, .. } => MridForm::Compact,
            Repr::Other(_) => MridForm::Opaque,
        }
    }

    /// Whether this identifier denotes a UUID, in either spelling.
    ///
    /// This is the question the RDF writer asks, because a UUID has a `urn:uuid:` IRI
    /// whether or not the document hyphenated it. Use [`Mrid::is_conforming`] for the
    /// stricter question validation asks.
    pub fn is_uuid(&self) -> bool {
        matches!(self.0, Repr::Uuid { .. })
    }

    /// Whether this identifier is written exactly as IEC 61970-552 requires.
    pub fn is_conforming(&self) -> bool {
        self.form() == MridForm::Uuid
    }

    /// Raw UUID bytes, if this identifier denotes a UUID.
    pub fn as_uuid_bytes(&self) -> Option<&[u8; 16]> {
        match &self.0 {
            Repr::Uuid { bytes, .. } => Some(bytes),
            Repr::Other(_) => None,
        }
    }

    /// Canonical lowercase hyphenated form without decoration, e.g. `70c4656c-f7a0-…`.
    ///
    /// Always the hyphenated spelling, whichever form the document used — this is the
    /// identifier's canonical *name*, used for diagnostics and for `urn:uuid:` IRIs.
    /// Non-conforming identifiers are returned verbatim.
    pub fn canonical(&self) -> String {
        match &self.0 {
            Repr::Uuid { bytes, .. } => format_uuid(bytes, false, 0),
            Repr::Other(s) => s.to_string(),
        }
    }

    /// The identifier as the document spelled it, without decoration.
    ///
    /// Differs from [`Mrid::canonical`] for a UUID a producer wrote without hyphens or in
    /// upper case — both of which the published CGMES 2.4.15 boundary sets do.
    pub fn as_written(&self) -> String {
        match &self.0 {
            Repr::Uuid {
                bytes,
                case,
                compact,
            } => format_uuid(bytes, *compact, *case),
            Repr::Other(s) => s.to_string(),
        }
    }

    /// The `rdf:ID` form: a leading underscore, as mandated for locally defined objects.
    ///
    /// The underscore is required — `rdf:ID` is an XML `NCName`, which may not begin with
    /// a digit — so it is added for non-UUID identifiers too. The identifier itself keeps
    /// the spelling it was read with, so re-exporting a file reproduces it.
    ///
    /// Not every identifier this crate can hold has such a form: see [`Mrid::form_in_xml`].
    pub fn to_rdf_id(&self) -> String {
        format!("_{}", self.as_written())
    }

    /// Which RDF/XML serializations this identifier can carry.
    ///
    /// A UUID always answers [`IdentifierForm::Name`](crate::xml::IdentifierForm::Name),
    /// so this is invisible on conforming
    /// input and exists for the input that is not. `Mrid` keeps a non-conforming
    /// identifier verbatim on purpose — see [`MridForm`] and the
    /// [concepts guide](https://hupe1980.github.io/cim-rs/docs/concepts/) —
    /// and an identifier a producer chose freely need be neither an `NCName` nor an IRI.
    /// The writer asks this rather than assuming `rdf:ID` fits, because
    /// `rdf:ID="_http://host/EQ.xml#Sub1"` is well-formed XML and invalid RDF/XML, which
    /// is exactly the combination a well-formedness check cannot catch.
    pub fn form_in_xml(&self) -> crate::xml::IdentifierForm {
        match &self.0 {
            // Every UUID, in either spelling, is a name once the underscore is added.
            Repr::Uuid { .. } => crate::xml::IdentifierForm::Name,
            Repr::Other(s) => crate::xml::identifier_form(s),
        }
    }

    /// The `rdf:resource` form for a same-document reference: `#_<uuid>`.
    pub fn to_rdf_resource(&self) -> String {
        format!("#{}", self.to_rdf_id())
    }

    /// The value to write into `rdf:resource` or `rdf:about`, in a form RDF/XML accepts.
    ///
    /// [`Mrid::to_rdf_resource`]'s `#_<id>` is the right answer for a UUID and for every
    /// other identifier that is a name — which is every identifier IEC 61970-552 permits,
    /// so the two agree on all conforming input. They part company on the identifier this
    /// crate keeps verbatim precisely because it is *not* conforming: an object identified
    /// by an absolute IRI. Prefixing `#_` to one produces `#_http://host/EQ.xml#Sub1`,
    /// which has two fragments and is not an IRI reference at all, so the document is not
    /// valid RDF/XML — while remaining well-formed XML, which is why a well-formedness
    /// check never noticed. Such an identifier is written as itself, which is both valid
    /// and what the document that supplied it said.
    pub fn to_rdf_reference(&self) -> String {
        match self.form_in_xml() {
            crate::xml::IdentifierForm::Iri => self.as_written(),
            // `Unwritable` has no valid form at all; the conventional one loses least and
            // `Rule::UnserializableIdentifier` reports that it is not enough.
            _ => self.to_rdf_resource(),
        }
    }

    /// The `urn:uuid:` form used by `md:FullModel` identifiers and by RDF subjects.
    ///
    /// Always hyphenated: RFC 4122 defines no other lexical form for a UUID, and RFC 8141
    /// makes the URN's namespace-specific string exactly that form. A non-UUID identifier
    /// has no valid URN form and is returned as-is.
    pub fn to_urn(&self) -> String {
        match &self.0 {
            Repr::Uuid { bytes, .. } => format!("urn:uuid:{}", format_uuid(bytes, false, 0)),
            Repr::Other(s) => s.to_string(),
        }
    }

    /// Derive a name-based UUID (RFC 4122 version 5) from a namespace and a name.
    ///
    /// CIM has no way to mint an identifier out of thin air, and a random one would make
    /// every export of the same model differ from the last — which defeats the point of a
    /// deterministic writer. A version-5 UUID is the standard answer: the same namespace
    /// and name always give the same identifier, different names practically never
    /// collide, and no random source or extra dependency is involved.
    ///
    /// ```
    /// # use cim_rs::Mrid;
    /// let ns = Mrid::parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8"); // RFC 4122 DNS namespace
    /// let a = Mrid::new_v5(&ns, b"MicroGrid/EQ");
    /// assert_eq!(a, Mrid::new_v5(&ns, b"MicroGrid/EQ"));
    /// assert_ne!(a, Mrid::new_v5(&ns, b"MicroGrid/SSH"));
    /// assert!(a.is_conforming());
    /// ```
    pub fn new_v5(namespace: &Mrid, name: &[u8]) -> Mrid {
        let ns = namespace.as_uuid_bytes().copied().unwrap_or([0u8; 16]);
        let mut h = Sha1::new();
        h.update(&ns);
        h.update(name);
        let digest = h.finish();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // Version 5, RFC 4122 variant.
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Mrid::from_uuid_bytes(bytes)
    }
}

impl PartialEq for Mrid {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Mrid {}

impl Hash for Mrid {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl PartialOrd for Mrid {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Mrid {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

/// Remove the decorations CIM/XML puts around identifiers.
///
/// Comparison is on bytes rather than on `str` slices: an identifier is arbitrary text
/// from a possibly malformed document, and slicing it at a fixed byte index would panic
/// whenever that index falls inside a multi-byte character.
fn strip_decoration(raw: &str) -> &str {
    const URN: &[u8] = b"urn:uuid:";
    let s = raw.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    // `urn:uuid:` is case-insensitive per RFC 8141.
    let s = match s.as_bytes().split_at_checked(URN.len()) {
        Some((head, _)) if head.eq_ignore_ascii_case(URN) => &s[URN.len()..],
        _ => s,
    };
    s.strip_prefix('_').unwrap_or(s)
}

/// Parse a UUID in either the hyphenated or the compact spelling, in either case.
///
/// Returns the raw bytes and how they were spelled. Both deviations are accepted because
/// published CGMES 2.4.15 files contain both — treating such identifiers as opaque text
/// would deny them a `urn:uuid:` IRI and split an object in two when another file spells
/// the same UUID the conforming way.
///
/// Case is recorded per digit, because published files really do mix it within one
/// identifier — `_24C12434-E42B-497f-928F-119C6AE92079` is from the CGMES 2.4.15 boundary
/// set. A single upper/lower flag rewrites such an identifier on export.
fn parse_uuid(s: &str) -> Option<Repr> {
    let b = s.as_bytes();
    let compact = match b.len() {
        36 => {
            if b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
                return None;
            }
            false
        }
        32 => true,
        _ => return None,
    };
    let mut out = [0u8; 16];
    let mut case = 0u32;
    let mut oi = 0;
    let mut digit = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'-' {
            i += 1;
            continue;
        }
        let hi = hex(b[i])?;
        let lo = hex(*b.get(i + 1)?)?;
        if b[i].is_ascii_uppercase() {
            case |= 1 << digit;
        }
        if b[i + 1].is_ascii_uppercase() {
            case |= 1 << (digit + 1);
        }
        out[oi] = (hi << 4) | lo;
        oi += 1;
        digit += 2;
        i += 2;
    }
    (oi == 16).then_some(Repr::Uuid {
        bytes: out,
        case,
        compact,
    })
}

const fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Render sixteen bytes, hyphenated or not, with `case` deciding each digit.
fn format_uuid(b: &[u8; 16], compact: bool, case: u32) -> String {
    const LOWER: &[u8; 16] = b"0123456789abcdef";
    const UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(36);
    let mut digit = 0;
    for (i, byte) in b.iter().enumerate() {
        if !compact && matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        for nibble in [byte >> 4, byte & 0x0f] {
            let table = if case & (1 << digit) != 0 {
                UPPER
            } else {
                LOWER
            };
            s.push(table[nibble as usize] as char);
            digit += 1;
        }
    }
    s
}

// ---------------------------------------------------------------------------
// SHA-1, for name-based (version 5) UUIDs
// ---------------------------------------------------------------------------

/// SHA-1 (FIPS 180-4), used only to derive RFC 4122 version-5 UUIDs.
///
/// Vendored rather than taken as a dependency: it is forty lines, the crate's promise is a
/// single mandatory dependency, and version-5 UUIDs are defined in terms of SHA-1 and
/// nothing else. It is emphatically **not** offered as a hash function — SHA-1 is broken
/// for collision resistance, which does not matter for deriving a name-based identifier
/// and would matter for anything else.
struct Sha1 {
    state: [u32; 5],
    buf: [u8; 64],
    len: u64,
}

impl Sha1 {
    fn new() -> Sha1 {
        Sha1 {
            state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buf: [0u8; 64],
            len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        let mut fill = (self.len % 64) as usize;
        self.len += data.len() as u64;
        while !data.is_empty() {
            let n = data.len().min(64 - fill);
            self.buf[fill..fill + n].copy_from_slice(&data[..n]);
            fill += n;
            data = &data[n..];
            if fill == 64 {
                let block = self.buf;
                self.compress(&block);
                fill = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 20] {
        let bits = self.len * 8;
        self.update(&[0x80]);
        while self.len % 64 != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 20];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e]) {
            *s = s.wrapping_add(v);
        }
    }
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
    const COMPACT: &str = "70c4656cf7a0431998bb84fb5e2e9b37";

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
            assert!(m.is_conforming(), "{f} is written conformingly");
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

    /// Published CGMES 2.4.15 files write UUIDs without hyphens. They are still UUIDs.
    #[test]
    fn a_compact_uuid_is_the_same_identifier_as_its_hyphenated_spelling() {
        let compact = Mrid::parse("_1fa19c281c8f4e1eaad9e1cab70f923e");
        let hyphenated = Mrid::parse("#_1fa19c28-1c8f-4e1e-aad9-e1cab70f923e");

        // Same object: a boundary file and an equipment file referring to it must join.
        assert_eq!(compact, hyphenated);
        assert_eq!(
            std::collections::HashSet::from([compact.clone(), hyphenated.clone()]).len(),
            1
        );

        // Both denote a UUID, so both have a `urn:uuid:` IRI — which is what keeps them
        // out of the blank-node path in the RDF writer.
        assert!(compact.is_uuid());
        assert_eq!(compact.to_urn(), hyphenated.to_urn());
        assert_eq!(compact.canonical(), hyphenated.canonical());

        // Only the spelling differs, and only the spelling is preserved on export.
        assert_eq!(compact.form(), MridForm::Compact);
        assert_eq!(hyphenated.form(), MridForm::Uuid);
        assert!(!compact.is_conforming());
        assert!(hyphenated.is_conforming());
        assert_eq!(compact.to_rdf_id(), "_1fa19c281c8f4e1eaad9e1cab70f923e");
        assert_eq!(
            hyphenated.to_rdf_id(),
            "_1fa19c28-1c8f-4e1e-aad9-e1cab70f923e"
        );
    }

    /// The published CGMES 2.4.15 boundary set writes UUIDs in upper-case hex —
    /// `_1C405140-0F45-4534-9C90-F38F4905362C`. Lower-casing it on export changes every
    /// identifier in that file, which no check inside this crate could see: the census
    /// records the identity *style* and never the text, and the round-trip compares the
    /// model, where identity is the sixteen bytes. PowSyBl saw it immediately, because an
    /// IIDM identifier is the mRID string.
    #[test]
    fn an_uppercase_uuid_keeps_its_case_on_export_but_not_its_identity() {
        const UPPER: &str = "1C405140-0F45-4534-9C90-F38F4905362C";
        const LOWER: &str = "1c405140-0f45-4534-9c90-f38f4905362c";
        let u = Mrid::parse(UPPER);
        let l = Mrid::parse(LOWER);

        // One object, whichever case it arrived in.
        assert_eq!(u, l);
        assert_eq!(
            std::collections::HashSet::from([u.clone(), l.clone()]).len(),
            1
        );

        // Written back as it came in, so re-export reproduces the document.
        assert_eq!(u.as_written(), UPPER);
        assert_eq!(u.to_rdf_id(), format!("_{UPPER}"));
        assert_eq!(l.as_written(), LOWER);

        // But the canonical name is always lower case, because RFC 4122 defines no other.
        assert_eq!(u.canonical(), LOWER);
        assert_eq!(u.to_urn(), format!("urn:uuid:{LOWER}"));
        assert_eq!(u.to_urn(), l.to_urn());

        // Upper case survives the compact spelling and the fragment path too.
        let compact = Mrid::parse("_1C4051400F4545349C90F38F4905362C");
        assert_eq!(compact, u);
        assert_eq!(compact.as_written(), "1C4051400F4545349C90F38F4905362C");
        assert_eq!(
            Mrid::parse(&format!("http://host/EQ.xml#_{UPPER}")).as_written(),
            UPPER
        );

        // Mixed case is not hypothetical: this identifier is from the published CGMES
        // 2.4.15 boundary set, and the `497f` inside it is lower case while the rest is
        // not. Per-digit fidelity is what this test exists for — a single upper/lower flag
        // rewrites it, and rewriting it is what PowSyBl noticed.
        const MIXED: &str = "24C12434-E42B-497f-928F-119C6AE92079";
        let m = Mrid::parse(MIXED);
        assert_eq!(m.as_written(), MIXED);
        assert_eq!(m.canonical(), MIXED.to_ascii_lowercase());
        assert_eq!(Mrid::parse(&m.to_rdf_id()), m);
        assert_eq!(m, Mrid::parse(&MIXED.to_ascii_lowercase()));
    }

    /// The spelling rides in padding the pointer variant already forced, so remembering it
    /// costs nothing per identifier — and a model holds one per object plus one per
    /// reference, which is where a byte would have been felt.
    #[test]
    fn remembering_the_spelling_does_not_grow_an_identifier() {
        assert!(
            std::mem::size_of::<Mrid>() <= 24,
            "Mrid grew to {} bytes",
            std::mem::size_of::<Mrid>()
        );
    }

    #[test]
    fn compact_and_hyphenated_spellings_survive_a_round_trip_each() {
        for raw in [CANON, COMPACT] {
            let m = Mrid::parse(raw);
            assert!(m.is_uuid(), "{raw}");
            assert_eq!(m.as_written(), raw, "{raw}");
            assert_eq!(Mrid::parse(&m.to_rdf_id()), m, "{raw}");
            assert_eq!(Mrid::parse(&m.to_rdf_resource()), m, "{raw}");
        }
        assert_eq!(Mrid::parse(CANON), Mrid::parse(COMPACT));
    }

    /// Producers write cross-document references as absolute IRIs. The fragment is the
    /// object; reading the whole IRI as opaque text dangles a reference against an object
    /// that is present in the model.
    #[test]
    fn an_absolute_iri_denotes_the_uuid_in_its_fragment() {
        for raw in [
            "http://example.com/model#_70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "http://example.com/model#70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
            "file:///models/EQ.xml#_70c4656c-f7a0-4319-98bb-84fb5e2e9b37",
        ] {
            let m = Mrid::parse(raw);
            assert!(m.is_uuid(), "{raw}");
            assert_eq!(m, Mrid::parse(CANON), "{raw}");
        }
        // The compact spelling survives the fragment path too, and stays compact.
        let compact = Mrid::parse(&format!("http://example.com/m#_{COMPACT}"));
        assert_eq!(compact.form(), MridForm::Compact);
        assert_eq!(compact, Mrid::parse(CANON));

        // Nothing is invented: a fragment that is not a UUID leaves the identifier opaque,
        // and so does a document IRI with no fragment at all.
        for raw in [
            "http://example.com/model#Substation",
            "http://example.com/model",
            "http://example.com/a#b#c",
        ] {
            assert!(!Mrid::parse(raw).is_uuid(), "{raw}");
        }
    }

    #[test]
    fn non_conforming_identifiers_keep_their_value_but_lose_decoration() {
        for raw in ["not-a-uuid", "12345", "garbage"] {
            let m = Mrid::parse(raw);
            assert!(!m.is_uuid(), "{raw:?}");
            assert_eq!(m.form(), MridForm::Opaque);
            assert_eq!(m.canonical(), raw);
            // Output uses the conforming form; re-reading gives the same identifier.
            assert_eq!(m.to_rdf_id(), format!("_{raw}"));
            assert_eq!(Mrid::parse(&m.to_rdf_id()), m);
            assert_eq!(Mrid::parse(&m.to_rdf_resource()), m);
        }
    }

    #[test]
    fn non_ascii_identifiers_do_not_panic() {
        // Decoration stripping must not slice a `str` at a fixed byte index: an
        // identifier is arbitrary text from a possibly malformed document, and byte 9
        // of these falls inside a multi-byte character.
        for raw in [
            "12345678é",
            "urn:uuiδ:x",
            "########é",
            "é",
            "ééééé",
            "#_ééééé",
            "urn:uuid:é",
            // Exactly 32 bytes, but not 32 hex digits: the compact form's length test is
            // in bytes, so what follows it must check each byte rather than assume.
            "éééééééééééééééé",
        ] {
            let m = Mrid::parse(raw);
            assert!(!m.is_uuid(), "{raw:?}");
            // Whatever survives stripping must still round-trip through the wire forms.
            assert_eq!(Mrid::parse(&m.to_rdf_id()), m, "{raw:?}");
        }
    }

    #[test]
    fn malformed_uuids_do_not_parse_as_uuid() {
        for raw in [
            "70c4656c-f7a0-4319-98bb-84fb5e2e9b3",   // too short
            "70c4656c-f7a0-4319-98bb-84fb5e2e9b377", // too long
            "70c4656cxf7a0-4319-98bb-84fb5e2e9b37",  // wrong separator
            "70c4656c-f7a0-4319-98bb-84fb5e2e9bzz",  // non-hex
            "70c4656cf7a0431998bb84fb5e2e9b3",       // 31 hex digits
            "70c4656cf7a0431998bb84fb5e2e9bzz",      // 32 characters, not all hex
        ] {
            assert!(!Mrid::parse(raw).is_uuid(), "{raw}");
        }
    }

    /// RFC 4122 §Appendix B publishes this vector for the DNS namespace and "www.example.org".
    #[test]
    fn version_5_uuids_match_the_published_test_vector() {
        let dns = Mrid::parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8");
        let m = Mrid::new_v5(&dns, b"www.example.org");
        assert_eq!(m.canonical(), "74738ff5-5367-5958-9aee-98fffdcd1876");
        assert!(m.is_conforming());
        // Deterministic, and distinct for distinct names.
        assert_eq!(m, Mrid::new_v5(&dns, b"www.example.org"));
        assert_ne!(m, Mrid::new_v5(&dns, b"www.example.com"));
    }

    #[test]
    fn sha1_matches_its_published_vectors() {
        let hex = |d: [u8; 20]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut h = Sha1::new();
        assert_eq!(hex(h.finish()), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        h = Sha1::new();
        h.update(b"abc");
        assert_eq!(hex(h.finish()), "a9993e364706816aba3e25717850c26c9cd0d89d");
        // Multi-block, exercising the buffering path.
        h = Sha1::new();
        for _ in 0..1_000_000 {
            h.update(b"a");
        }
        assert_eq!(hex(h.finish()), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }
}
