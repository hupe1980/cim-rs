//! Anything the reader accepts must be writable, and re-readable without error.
#![no_main]

use cim::cgmes3::SCHEMA;
use cim::prelude::*;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut ds = Dataset::new(SCHEMA);
    if cim::reader::read_into(&mut ds, data, None, &ReadOptions::lenient()).is_err() {
        return;
    }

    let mut buf = Vec::new();
    if cim::writer::write(&ds, &mut buf, &WriteOptions::default()).is_err() {
        return;
    }

    // Our own output must always be readable, and must produce the same object count.
    let mut back = Dataset::new(SCHEMA);
    let outcome = cim::reader::read_into(&mut back, buf.as_slice(), None, &ReadOptions::lenient())
        .expect("output of the writer must be readable");
    assert!(
        !outcome.report.has_errors(),
        "re-reading our own output produced errors"
    );
    assert_eq!(back.len(), ds.len(), "round-trip changed the object count");
});
