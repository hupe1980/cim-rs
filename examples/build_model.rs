//! Build a CGMES Equipment model in memory and write it out — no input files needed.
//!
//! ```text
//! cargo run --example build_model --features cgmes3
//! ```
//!
//! Everything else in this repository reads a model somebody else produced. This is the
//! other direction, and it has two subtleties worth showing.
//!
//! A model built in memory has no `md:FullModel` identifier, and IEC 61970-552 requires
//! one. A random UUID would make every export of an unchanged model a different document;
//! [`Dataset::content_id`] derives one from what the model *contains*, so an unchanged
//! model exports identically and a changed one does not.
//!
//! And an instance file may only name a class whose mandatory attributes the data actually
//! supplies — otherwise the document claims more than the model supports. Leave `bch` off
//! the line below and the writer emits `<cim:Equipment>` rather than `<cim:ACLineSegment>`,
//! which is the same rule that makes published Steady State Hypothesis files write
//! `<cim:Equipment>` for a breaker.

use cim_rs::cgmes3::{SCHEMA, attributes as attrs, classes};
use cim_rs::prelude::*;
use cim_rs::schema::ClassId;

/// Mint a stable identifier, so running this twice produces the same document.
fn id(name: &str) -> Mrid {
    Mrid::new_v5(&Dataset::DERIVED_NS, name.as_bytes())
}

fn main() -> cim_rs::Result<()> {
    let mut ds = Dataset::new(SCHEMA);
    let eq = SCHEMA.profile_by_keyword("EQ").expect("the EQ profile");

    // Every CIM object is an `IdentifiedObject`: an mRID and a name, at minimum. Naming
    // the profile on each value is what lets the writer put it back in the right file —
    // see `Schema::effective_profiles`.
    let new = |class: ClassId, key: &str, name: &str| {
        let mrid = id(key);
        let mut o = Object::new(class, mrid.clone());
        o.set_in(
            eq.mask(),
            attrs::identified_object::mRID,
            mrid.canonical().into(),
        );
        o.set_in(eq.mask(), attrs::identified_object::name, name.into());
        (mrid, o)
    };

    // A geographical region and one sub-region, because `Substation.Region` is mandatory
    // in the Equipment profile and a substation without one is not a conforming document.
    let (region, o) = new(classes::GeographicalRegion, "demo/Region", "Benelux");
    ds.insert(o);
    let (sub_region, mut o) = new(classes::SubGeographicalRegion, "demo/SubRegion", "BE-NL");
    // An association is stored as the target's identifier, because IEC 61970-501
    // serializes exactly one side of each — the other is derived by inversion.
    o.set_in(
        eq.mask(),
        attrs::sub_geographical_region::Region,
        Value::Reference(region),
    );
    ds.insert(o);

    let (base_voltage, mut o) = new(classes::BaseVoltage, "demo/380kV", "380 kV");
    // A CIM datatype serializes as its primitive; the unit and multiplier are schema
    // constants, recorded on `DatatypeDef` and repeated in each accessor's rustdoc.
    o.set_in(eq.mask(), attrs::base_voltage::nominalVoltage, 380.0.into());
    ds.insert(o);

    for town in ["Brussels", "Amsterdam"] {
        let (_, mut o) = new(classes::Substation, &format!("demo/{town}"), town);
        o.set_in(
            eq.mask(),
            attrs::substation::Region,
            Value::Reference(sub_region.clone()),
        );
        ds.insert(o);
    }

    let (_, mut o) = new(classes::ACLineSegment, "demo/BE-NL-1", "BE-NL 1");
    for (attr, v) in [
        (attrs::ac_line_segment::r, 2.2),
        (attrs::ac_line_segment::x, 68.2),
        (attrs::ac_line_segment::bch, 0.0001),
    ] {
        o.set_in(eq.mask(), attr, v.into());
    }
    o.set_in(
        eq.mask(),
        attrs::conducting_equipment::BaseVoltage,
        Value::Reference(base_voltage),
    );
    ds.insert(o);

    // Structural checks, against the profile rather than against a guess. Scoped to EQ:
    // an attribute the Steady State Hypothesis profile requires is not missing from a
    // model that is only claiming to be an Equipment file.
    let report = ds.validate_with(&cim_rs::ValidateOptions::default().for_profiles(eq.mask()));
    eprintln!("{} object(s), {} finding(s)", ds.len(), report.len());
    for d in report.iter().take(5) {
        eprintln!("  {d}");
    }
    eprintln!("content id: {}", ds.content_id());

    // Typed views work on a model built this way exactly as on one that was read.
    for line in ds.view::<cim_rs::cgmes3::views::ACLineSegment>() {
        let kv = line
            .base_voltage_in(&ds)
            .and_then(|bv| bv.nominal_voltage());
        eprintln!("{:?}: r={:?} at {kv:?} kV", line.name(), line.r());
    }

    // No header supplied, so a conforming one is derived — identifier included.
    cim_rs::writer::write_profile(&ds, eq, std::io::stdout().lock(), &Default::default())
}
