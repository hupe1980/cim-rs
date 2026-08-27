//! Locating the ENTSO-E conformity test corpus.
//!
//! The models are fetched by `cargo xtask fetch-specs` into the gitignored `specs/`
//! directory. They are licensed CC BY-SA 4.0 and owned by ENTSO-E, so they serve as a
//! local test corpus only and are never redistributed with this crate. Tests that need
//! them skip when the corpus is absent, keeping `cargo test` green on a fresh clone.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The repository root, which is also the crate's own manifest directory.
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn specs() -> Option<PathBuf> {
    let p = workspace_root().join("specs");
    p.is_dir().then_some(p)
}

/// Root of the CGMES 3.0 conformity assessment configurations.
pub fn cgmes3_models() -> Option<PathBuf> {
    let p = specs()?
        .join("test-models/cas-3.0.3")
        .join("CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3/v3.0");
    p.is_dir().then_some(p)
}

/// A named model directory inside the CGMES 3.0 corpus.
pub fn cgmes3_model(rel: &str) -> Option<PathBuf> {
    let p = cgmes3_models()?.join(rel);
    p.is_dir().then_some(p)
}

/// Every `.xml` instance file under `dir`, sorted for reproducibility.
pub fn xml_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
            out.push(p);
        }
    }
}

/// Skip a test with an explanatory note when the corpus is unavailable.
#[macro_export]
macro_rules! require_corpus {
    ($e:expr) => {
        match $e {
            Some(v) => v,
            None => {
                eprintln!(
                    "skipping: ENTSO-E conformity models not found; run `cargo xtask fetch-specs`"
                );
                return;
            }
        }
    };
}

/// How many objects of each class a document holds, and how each is identified.
///
/// The unit of comparison for "did this export reproduce its input". A projection of the
/// assembled model can see neither the difference between `rdf:ID` and `rdf:about` nor an
/// object landing in the wrong file.
pub fn element_census(text: &str) -> std::collections::BTreeMap<(String, &'static str), usize> {
    let mut out = std::collections::BTreeMap::new();
    for (i, _) in text.match_indices('<') {
        let rest = &text[i + 1..];
        let Some(end) = rest.find(|c: char| c.is_whitespace() || c == '>' || c == '/') else {
            continue;
        };
        let name = &rest[..end];
        // Object elements are `prefix:ClassName`; properties carry a dot, and neither the
        // header nor a difference statement group is an object.
        if name.contains('.') || !name.contains(':') || name.ends_with("FullModel") {
            continue;
        }
        let tail = &rest[end..];
        let Some(close) = tail.find('>') else {
            continue;
        };
        let attrs = &tail[..close];
        let style = if attrs.contains("rdf:ID=") {
            "rdf:ID"
        } else if attrs.contains("rdf:about=") {
            "rdf:about"
        } else {
            continue;
        };
        *out.entry((name.to_owned(), style)).or_default() += 1;
    }
    out
}

/// Every object identifier in a document, exactly as it is spelled.
///
/// The census above counts objects by class and by *identity style* — `rdf:ID` versus
/// `rdf:about` — and never looks at the identifier text, which makes it structurally blind
/// to an identifier being *rewritten*: the count and the style are unchanged when
/// `_1C405140-0F45-…` comes back as `_1c405140-0f45-…`. So is the round-trip, where identity
/// is the sixteen bytes by construction. The published CGMES 2.4.15 boundary set writes
/// mixed-case hex, so this is not hypothetical.
///
/// Header identifiers are excluded deliberately: `md:FullModel rdf:about="urn:uuid:_<uuid>"`
/// appears in the published 2.4.15 files and is not a valid URN — RFC 8141 gives the uuid
/// namespace no underscore — so writing the conforming form back is a repair rather than a
/// loss, and the only place this crate knowingly does not reproduce its input.
pub fn identifier_census(text: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for (attr, prefix) in [("rdf:ID=\"", ""), ("rdf:about=\"", "#")] {
        for (i, _) in text.match_indices(attr) {
            let rest = &text[i + attr.len()..];
            let Some(end) = rest.find('"') else { continue };
            let value = &rest[..end];
            // A header identifier is a `urn:uuid:`; an object's `rdf:about` is a fragment.
            if value.starts_with("urn:uuid:") {
                continue;
            }
            if prefix.is_empty() || value.starts_with('#') {
                out.insert(value.trim_start_matches('#').to_owned());
            }
        }
    }
    out
}

/// Check a document against the XML and XML Namespaces recommendations.
///
/// Returns `Err` with the first violation found.
///
/// The crate's own reader is deliberately tolerant, so re-reading what the writer produced
/// says only that this crate can read it back — nothing about whether anyone else's parser
/// will. The checks here are the ones a CIM consumer's parser applies: balanced and
/// correctly nested elements, no duplicate attribute, every prefix bound before use, and
/// every character inside XML 1.0's `Char` production.
///
/// The last is tested by this function directly rather than delegated to `quick-xml`, which
/// the reader also uses and which enforces the `Char` production in neither direction. A
/// checker built on the library it is checking inherits that library's permissions.
pub fn check_well_formed(text: &str) -> Result<(), String> {
    use quick_xml::NsReader;
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;

    // XML 1.0 §2.2. Checked over the whole document rather than per event: it constrains
    // markup and content alike, and there is no legal position for such a character.
    if let Some((offset, c)) = cim_rs::xml::find_illegal(text) {
        return Err(format!(
            "byte {offset} is U+{:04X}, which XML 1.0's Char production excludes; \
             no escape or numeric reference can represent it",
            c as u32
        ));
    }

    let mut reader = NsReader::from_str(text);
    reader.config_mut().check_end_names = true;
    reader.config_mut().expand_empty_elements = false;

    let describe = |e: &quick_xml::events::BytesStart<'_>| {
        String::from_utf8_lossy(e.name().as_ref()).into_owned()
    };

    loop {
        let (ns, event) = match reader.read_resolved_event() {
            Ok(v) => v,
            Err(e) => return Err(format!("not well-formed: {e}")),
        };
        let start = match &event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => e.clone(),
            _ => continue,
        };

        if let ResolveResult::Unknown(p) = &ns {
            return Err(format!(
                "element <{}> uses undeclared prefix {:?} at byte {}",
                describe(&start),
                String::from_utf8_lossy(p),
                reader.buffer_position()
            ));
        }

        // `with_checks(true)` is what reports a duplicate attribute; it is off on the
        // reader's hot path, so only this sees one.
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for attr in start.attributes().with_checks(true) {
            let attr = attr.map_err(|e| {
                format!(
                    "element <{}> has a malformed attribute list: {e}",
                    describe(&start)
                )
            })?;
            let key = attr.key.as_ref().to_vec();
            if seen.contains(&key) {
                return Err(format!(
                    "element <{}> declares attribute {:?} twice",
                    describe(&start),
                    String::from_utf8_lossy(&key)
                ));
            }
            // An attribute's prefix must be bound too, `xmlns` and `xml` aside.
            // The bindings live on the reader's resolver, which is reachable only through
            // `resolver_mut`; `start` is an owned clone, so borrowing the reader here does
            // not conflict with the iteration.
            if let Some(i) = key.iter().position(|&b| b == b':')
                && !matches!(&key[..i], b"xmlns" | b"xml")
                && matches!(
                    reader.resolver_mut().resolve_attribute(attr.key).0,
                    ResolveResult::Unknown(_)
                )
            {
                return Err(format!(
                    "element <{}> attribute {:?} uses an undeclared prefix",
                    describe(&start),
                    String::from_utf8_lossy(&key)
                ));
            }
            seen.push(key);
        }
    }
    Ok(())
}

/// Assert well-formedness, naming the document in the failure.
pub fn assert_well_formed(label: &str, text: &str) {
    if let Err(e) = check_well_formed(text) {
        panic!("{label} is not well-formed XML: {e}");
    }
}

/// Check a document against the N-Triples grammar, returning the number of triples.
///
/// The same reasoning as [`check_well_formed`]: output that only this crate can read is
/// not interchange. N-Triples is small enough to check exactly, so it is, and any
/// escaping mistake in an identifier, an IRI or a literal shows up here rather than in
/// somebody else's parser.
pub fn check_ntriples(text: &str) -> Result<usize, String> {
    let mut count = 0;
    for (n, line) in text.lines().enumerate() {
        let at = |m: &str| format!("line {}: {m}: {line}", n + 1);
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let rest = line
            .strip_suffix('.')
            .ok_or_else(|| at("no terminating '.'"))?
            .trim_end();
        let rest = take_subject(rest).ok_or_else(|| at("bad subject"))?;
        let rest = take_iri(rest.trim_start()).ok_or_else(|| at("bad predicate"))?;
        let rest = take_object(rest.trim_start()).ok_or_else(|| at("bad object"))?;
        if !rest.trim().is_empty() {
            return Err(at("trailing content after the object"));
        }
        count += 1;
    }
    Ok(count)
}

/// An `IRIREF`: angle brackets with none of the characters the grammar excludes.
fn take_iri(s: &str) -> Option<&str> {
    let body = s.strip_prefix('<')?;
    let end = body.find('>')?;
    let iri = &body[..end];
    let forbidden = iri
        .chars()
        .any(|c| (c as u32) <= 0x20 || matches!(c, '<' | '"' | '{' | '}' | '|' | '^' | '`'));
    // A backslash is only legal as the start of a \u escape.
    let bad_escape = iri
        .match_indices('\\')
        .any(|(i, _)| !matches!(iri[i + 1..].chars().next(), Some('u') | Some('U')));
    (!forbidden && !bad_escape).then(|| &body[end + 1..])
}

fn take_blank(s: &str) -> Option<&str> {
    let body = s.strip_prefix("_:")?;
    let end = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .unwrap_or(body.len());
    (end > 0).then(|| &body[end..])
}

fn take_subject(s: &str) -> Option<&str> {
    take_iri(s).or_else(|| take_blank(s))
}

fn take_object(s: &str) -> Option<&str> {
    if let Some(rest) = take_iri(s).or_else(|| take_blank(s)) {
        return Some(rest);
    }
    // A literal: a quoted string, optionally with a datatype IRI.
    let body = s.strip_prefix('"')?;
    let mut chars = body.char_indices();
    let end = loop {
        let (i, c) = chars.next()?;
        match c {
            // Every escape is two characters, so the next one is consumed unread.
            '\\' => {
                chars.next()?;
            }
            '"' => break i,
            // A raw newline, carriage return or quote must have been escaped.
            '\n' | '\r' => return None,
            _ => {}
        }
    };
    let rest = &body[end + 1..];
    match rest.strip_prefix("^^") {
        Some(dt) => take_iri(dt),
        // A language tag is legal but this crate never writes one.
        None => Some(rest),
    }
}

/// Describe the first few differences between two censuses.
pub fn census_diff(
    a: &std::collections::BTreeMap<(String, &'static str), usize>,
    b: &std::collections::BTreeMap<(String, &'static str), usize>,
) -> String {
    let mut keys: Vec<_> = a.keys().chain(b.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| a.get(k) != b.get(k))
        .take(4)
        .map(|k| {
            format!(
                "{} {} {} -> {}",
                k.0,
                k.1,
                a.get(&k).copied().unwrap_or(0),
                b.get(&k).copied().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
