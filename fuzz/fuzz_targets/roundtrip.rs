//! Anything the reader accepts must be writable, re-readable, and *well-formed XML*.
//!
//! The last of those is the one worth spelling out. Re-reading the writer's output with the
//! crate's own reader is a weak check by construction: that reader is deliberately
//! tolerant, so it accepts documents no other parser will — which is exactly how every file
//! the writer produced once carried `xmlns:md` twice without anything noticing. A fuzzer is
//! the place that blind spot costs most, because it is the one tool here that manufactures
//! untidy input on purpose, and a writer defect that only untidy input can reach has
//! nowhere else to show up.
#![no_main]

use cim_rs::cgmes3::SCHEMA;
use cim_rs::prelude::*;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut ds = Dataset::new(SCHEMA);
    if cim_rs::reader::read_into(&mut ds, data, None, &ReadOptions::lenient()).is_err() {
        return;
    }

    let mut buf = Vec::new();
    if cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).is_err() {
        return;
    }

    let text = String::from_utf8(buf.clone()).expect("the writer must produce UTF-8");
    assert_well_formed(&text);

    // Our own output must always be readable, and must produce the same object count.
    let mut back = Dataset::new(SCHEMA);
    let outcome =
        cim_rs::reader::read_into(&mut back, buf.as_slice(), None, &ReadOptions::lenient())
            .expect("output of the writer must be readable");
    assert!(
        !outcome.report.has_errors(),
        "re-reading our own output produced errors"
    );
    assert_eq!(back.len(), ds.len(), "round-trip changed the object count");
});

/// The constraints a CIM consumer's parser applies, checked with them turned on.
///
/// Deliberately a second implementation of what `tests/common/mod.rs` does rather than a
/// shared one: a `cargo-fuzz` target is its own crate and cannot reach a test helper, and
/// the alternative — exposing the checker from the library — would put a testing concern in
/// the published API. The duplication is thirty lines and both copies are pinned by the
/// same property.
fn assert_well_formed(text: &str) {
    use quick_xml::NsReader;
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;

    // XML 1.0 §2.2. Checked first and separately because `quick-xml` does not enforce the
    // `Char` production in either direction — the reader accepts a raw control character
    // and the writer would put it back out, producing a document every conforming parser
    // refuses at that byte.
    assert!(
        cim_rs::xml::find_illegal(text).is_none(),
        "output holds a character XML 1.0 cannot represent"
    );

    let mut reader = NsReader::from_str(text);
    reader.config_mut().check_end_names = true;
    reader.config_mut().expand_empty_elements = false;

    loop {
        let (ns, event) = reader.read_resolved_event().expect("output must parse");
        let start = match &event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => e.clone(),
            _ => continue,
        };
        assert!(
            !matches!(ns, ResolveResult::Unknown(_)),
            "output uses an undeclared element prefix"
        );

        // `with_checks(true)` is what reports a duplicate attribute; it is off on the
        // reader's hot path, which is precisely why that defect survived so long.
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for attr in start.attributes().with_checks(true) {
            let attr = attr.expect("output must have a well-formed attribute list");
            let key = attr.key.as_ref().to_vec();
            assert!(
                !seen.contains(&key),
                "output declares an attribute twice on one element"
            );
            if let Some(i) = key.iter().position(|&b| b == b':')
                && !matches!(&key[..i], b"xmlns" | b"xml")
            {
                assert!(
                    !matches!(
                        reader.resolver_mut().resolve_attribute(attr.key).0,
                        ResolveResult::Unknown(_)
                    ),
                    "output uses an undeclared attribute prefix"
                );
            }
            seen.push(key);
        }
    }
}
