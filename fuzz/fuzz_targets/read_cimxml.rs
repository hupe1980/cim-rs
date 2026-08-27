//! The reader must never panic, whatever bytes it is given.
//!
//! Lenient mode is the documented contract for real-world files: malformed input yields
//! an error or a model plus diagnostics, never a crash.
#![no_main]

use cim_rs::cgmes3::SCHEMA;
use cim_rs::prelude::*;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for options in [ReadOptions::lenient(), ReadOptions::strict()] {
        let mut ds = Dataset::new(SCHEMA);
        if cim_rs::reader::read_into(&mut ds, data, Some("fuzz.xml"), &options).is_ok() {
            // Whatever was read must survive the operations the API promises.
            let _ = cim_rs::validate::validate(&ds);
            let _ = ds.dangling_references();
            let _ = cim_rs::InverseIndex::build(&ds);
            for (_, o) in ds.iter() {
                for slot in o.slots() {
                    let _ = SCHEMA.attr(slot.attr).name;
                }
            }
        }
    }
});
