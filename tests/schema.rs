//! Invariants every shipped schema vintage must satisfy.
//!
//! These hold the promises the architecture makes about generated tables, so that adding a
//! vintage or a third-party profile fails here rather than somewhere far away.

use cim_rs::schema::{ProfileId, Schema};

/// Every vintage this build has.
///
/// Written as a chain of conditional pushes rather than a literal because which vintages
/// exist is a feature-matrix question, and `cargo test --no-default-features` must still
/// compile this file down to an empty list.
#[allow(clippy::vec_init_then_push)]
fn schemas() -> Vec<&'static Schema> {
    #[allow(unused_mut)]
    let mut out: Vec<&'static Schema> = Vec::new();
    #[cfg(feature = "cgmes3")]
    out.push(cim_rs::cgmes3::SCHEMA);
    #[cfg(feature = "cgmes2")]
    out.push(cim_rs::cgmes2::SCHEMA);
    out
}

/// A profile set is data, and `ProfileMask` is 64 bits — so the design's promise that
/// twenty-nine published profiles "still leave room for a private one" is a claim about a
/// bound that nothing else checks.
///
/// It matters more than an off-by-one usually would. Rust masks an over-wide shift in
/// release builds, so a sixty-fifth profile would not fail: it would take profile 0's bit
/// and silently merge two profiles' data everywhere provenance is consulted — the writer's
/// file selection, the differ's scope, the RDF export's per-profile slice. `ProfileId::mask`
/// asserts, and this makes the assertion unreachable rather than merely present.
#[test]
fn schemas_fit_the_profile_mask() {
    for s in schemas() {
        assert!(
            s.profiles.len() <= ProfileId::MAX_PROFILES,
            "{} declares {} profiles, more than the {} bits of ProfileMask; \
             widen the type and regenerate",
            s.vintage,
            s.profiles.len(),
            ProfileId::MAX_PROFILES,
        );
        // And the masks are genuinely distinct, which is the property the bound exists for.
        let mut seen = 0u64;
        for i in 0..s.profiles.len() {
            let m = ProfileId(i as u16).mask();
            assert_eq!(
                seen & m,
                0,
                "{}: profile {i} aliases an earlier one",
                s.vintage
            );
            seen |= m;
        }
    }
}

/// Vintages are told apart by `Schema::vintage`, which `Dataset::merge` and
/// `Dataset::difference_to` compare to refuse mixing tables that mean different things.
#[test]
fn vintages_are_distinguishable() {
    let all = schemas();
    for (i, a) in all.iter().enumerate() {
        assert!(!a.vintage.is_empty());
        for b in &all[i + 1..] {
            assert_ne!(a.vintage, b.vintage, "two vintages share a name");
        }
    }
}
