//! Objects: the storage unit of a CIM model.

use crate::mrid::Mrid;
use crate::schema::{AttrId, ClassId, ProfileMask};
use crate::value::Value;

/// One stored attribute occurrence, with the profiles it came from.
///
/// Provenance matters because CGMES declares common attributes such as
/// `IdentifiedObject.mRID` in almost every profile. Deciding what to write from
/// declarations alone would repeat every object in every profile file; recording where a
/// value actually came from reproduces the original file set exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub attr: AttrId,
    /// Profiles whose instance files carried this value.
    ///
    /// Zero means the provenance is unknown — typically a value set programmatically —
    /// in which case serialization falls back to the attribute's declared profiles.
    pub profiles: ProfileMask,
    pub value: Value,
}

impl Slot {
    pub fn new(attr: AttrId, value: Value) -> Slot {
        Slot {
            attr,
            profiles: 0,
            value,
        }
    }

    pub fn in_profiles(attr: AttrId, profiles: ProfileMask, value: Value) -> Slot {
        Slot {
            attr,
            profiles,
            value,
        }
    }
}

/// A structured value with no identity of its own.
///
/// IEC 61970-552 serializes a `Compound` inline — `rdf:parseType="Resource"` inside the
/// property element that holds it — rather than as a reference, because it has no mRID to
/// point at. CGMES 3.0 uses this for postal addresses in the Geographical Location
/// profile (`Location.mainAddress` → `StreetAddress` → `StreetDetail`), and compounds
/// nest.
///
/// Storage mirrors [`Object`]: a vector sorted by [`AttrId`], with repeats adjacent.
/// A compound carries no profile provenance of its own; it belongs to whichever
/// [`Slot`] holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Compound {
    class: ClassId,
    values: Vec<(AttrId, Value)>,
}

impl Compound {
    pub fn new(class: ClassId) -> Compound {
        Compound {
            class,
            values: Vec::new(),
        }
    }

    /// The compound type, e.g. `StreetAddress`.
    #[inline]
    pub fn class(&self) -> ClassId {
        self.class
    }

    /// All stored `(attribute, value)` pairs, in attribute order.
    #[inline]
    pub fn values(&self) -> &[(AttrId, Value)] {
        &self.values
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    fn range(&self, attr: AttrId) -> std::ops::Range<usize> {
        let start = self.values.partition_point(|(a, _)| *a < attr);
        let end = self.values[start..].partition_point(|(a, _)| *a == attr) + start;
        start..end
    }

    /// First value of `attr`, if present.
    #[inline]
    pub fn get(&self, attr: AttrId) -> Option<&Value> {
        let r = self.range(attr);
        self.values
            .get(r.start)
            .filter(|(a, _)| *a == attr)
            .map(|(_, v)| v)
    }

    /// All values of `attr`, for many-valued fields.
    #[inline]
    pub fn get_all(&self, attr: AttrId) -> &[(AttrId, Value)] {
        &self.values[self.range(attr)]
    }

    /// Every value of `attr` — the uniform accessor generated views use.
    #[inline]
    pub fn all(&self, attr: AttrId) -> impl Iterator<Item = &Value> {
        self.get_all(attr).iter().map(|(_, v)| v)
    }

    pub fn has(&self, attr: AttrId) -> bool {
        !self.range(attr).is_empty()
    }

    pub fn count(&self, attr: AttrId) -> usize {
        self.range(attr).len()
    }

    /// Replace every occurrence of `attr` with a single value.
    pub fn set(&mut self, attr: AttrId, value: Value) {
        let r = self.range(attr);
        self.values.splice(r, std::iter::once((attr, value)));
    }

    /// Append another occurrence of `attr`, keeping storage sorted.
    pub fn push(&mut self, attr: AttrId, value: Value) {
        let r = self.range(attr);
        self.values.insert(r.end, (attr, value));
    }
}

/// A single CIM object.
///
/// Storage is sparse — only attributes actually present are stored — which matches how
/// CIM data behaves: a `Terminal` carries roughly thirty possible attributes but a
/// Steady State Hypothesis file sets exactly one of them. Slots are kept sorted by
/// [`AttrId`], with repeats adjacent for many-valued attributes.
#[derive(Clone, Debug)]
pub struct Object {
    pub(crate) class: ClassId,
    pub(crate) mrid: Mrid,
    /// Sorted by `AttrId`; duplicates are permitted and adjacent.
    pub(crate) values: Vec<Slot>,
    /// Profiles whose instance files contributed data to this object.
    pub(crate) profiles: ProfileMask,
    /// Whether this object came from a file, rather than being built programmatically.
    ///
    /// Which file is recorded by the dataset rather than here — see
    /// [`Dataset::objects_from`](crate::Dataset::objects_from) — because an object appears
    /// in as many files as there are profiles describing it, and a per-object set would
    /// cost more than the index does.
    pub(crate) from_file: bool,
}

impl Object {
    pub fn new(class: ClassId, mrid: Mrid) -> Object {
        Object {
            class,
            mrid,
            values: Vec::new(),
            profiles: 0,
            from_file: false,
        }
    }

    #[inline]
    pub fn class(&self) -> ClassId {
        self.class
    }

    #[inline]
    pub fn mrid(&self) -> &Mrid {
        &self.mrid
    }

    /// Profiles that contributed data to this object.
    #[inline]
    pub fn profiles(&self) -> ProfileMask {
        self.profiles
    }

    /// Whether this object was read from a file rather than built programmatically.
    #[inline]
    pub fn from_file(&self) -> bool {
        self.from_file
    }

    /// Change the object's class, e.g. when a later profile reveals a more specific type.
    pub(crate) fn set_class(&mut self, class: ClassId) {
        self.class = class;
    }

    pub(crate) fn mark_profile(&mut self, mask: ProfileMask) {
        self.profiles |= mask;
    }

    /// All stored slots, in attribute order.
    #[inline]
    pub fn slots(&self) -> &[Slot] {
        &self.values
    }

    /// All stored `(attribute, value)` pairs, in attribute order.
    pub fn values(&self) -> impl Iterator<Item = (AttrId, &Value)> + '_ {
        self.values.iter().map(|s| (s.attr, &s.value))
    }

    /// Number of stored attribute occurrences.
    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Half-open range of `values` holding occurrences of `attr`.
    fn range(&self, attr: AttrId) -> std::ops::Range<usize> {
        let start = self.values.partition_point(|s| s.attr < attr);
        let end = self.values[start..].partition_point(|s| s.attr == attr) + start;
        start..end
    }

    /// First value of `attr`, if present.
    #[inline]
    pub fn get(&self, attr: AttrId) -> Option<&Value> {
        self.get_slot(attr).map(|s| &s.value)
    }

    /// First slot of `attr`, including its provenance.
    #[inline]
    pub fn get_slot(&self, attr: AttrId) -> Option<&Slot> {
        let r = self.range(attr);
        // An empty range still points at the next attribute, so the identity of the
        // slot found must be checked rather than assumed.
        self.values.get(r.start).filter(|s| s.attr == attr)
    }

    /// All slots of `attr`, for many-valued attributes.
    #[inline]
    pub fn get_all(&self, attr: AttrId) -> &[Slot] {
        &self.values[self.range(attr)]
    }

    /// Every value of `attr` — the uniform accessor generated views use.
    #[inline]
    pub fn all(&self, attr: AttrId) -> impl Iterator<Item = &Value> {
        self.get_all(attr).iter().map(|s| &s.value)
    }

    /// Number of stored occurrences of `attr`.
    pub fn count(&self, attr: AttrId) -> usize {
        self.range(attr).len()
    }

    pub fn has(&self, attr: AttrId) -> bool {
        !self.range(attr).is_empty()
    }

    /// Replace every occurrence of `attr` with a single value of unknown provenance.
    pub fn set(&mut self, attr: AttrId, value: Value) {
        self.set_slot(Slot::new(attr, value));
    }

    /// Replace every occurrence of `attr`, recording which profile the value belongs to.
    pub fn set_in(&mut self, profiles: ProfileMask, attr: AttrId, value: Value) {
        self.set_slot(Slot::in_profiles(attr, profiles, value));
    }

    pub fn set_slot(&mut self, slot: Slot) {
        let r = self.range(slot.attr);
        self.values.splice(r, std::iter::once(slot));
    }

    /// Append another occurrence of `attr`, keeping storage sorted.
    pub fn push(&mut self, attr: AttrId, value: Value) {
        self.push_slot(Slot::new(attr, value));
    }

    /// Append another occurrence, recording which profile the value belongs to.
    pub fn push_in(&mut self, profiles: ProfileMask, attr: AttrId, value: Value) {
        self.push_slot(Slot::in_profiles(attr, profiles, value));
    }

    pub fn push_slot(&mut self, slot: Slot) {
        let r = self.range(slot.attr);
        self.values.insert(r.end, slot);
    }

    /// Remove every occurrence of `attr`, returning how many were removed.
    pub fn remove(&mut self, attr: AttrId) -> usize {
        let r = self.range(attr);
        let n = r.len();
        self.values.drain(r);
        n
    }

    /// Remove one specific `(attr, value)` occurrence, e.g. when applying a
    /// difference model's reverse statements. Returns whether one was removed.
    pub fn remove_value(&mut self, attr: AttrId, value: &Value) -> bool {
        let r = self.range(attr);
        match self.values[r.clone()]
            .iter()
            .position(|s| s.value == *value)
        {
            Some(i) => {
                self.values.remove(r.start + i);
                true
            }
            None => false,
        }
    }

    /// Keep the slots `keep` accepts, returning the rest in storage order.
    ///
    /// The removed slots are handed back rather than dropped so that a caller thinning an
    /// object — reclassifying it, say — can say what went.
    pub(crate) fn retain_slots(&mut self, keep: impl Fn(&Slot) -> bool) -> Vec<Slot> {
        let mut shed = Vec::new();
        self.values.retain(|s| {
            let keeping = keep(s);
            if !keeping {
                shed.push(s.clone());
            }
            keeping
        });
        shed
    }

    /// Record that `attr`'s existing value also came from `profiles`.
    pub(crate) fn merge_provenance(&mut self, attr: AttrId, value: &Value, profiles: ProfileMask) {
        let r = self.range(attr);
        if let Some(s) = self.values[r].iter_mut().find(|s| s.value == *value) {
            s.profiles |= profiles;
        }
    }

    /// Shrink storage after bulk loading.
    pub(crate) fn shrink(&mut self) {
        self.values.shrink_to_fit();
    }

    /// Release the parser's over-allocation as soon as an object is complete.
    ///
    /// Objects are sparse, so a growth-doubled `Vec` typically holds far more capacity
    /// than it needs; over a million objects that adds up.
    pub(crate) fn shrink_for_load(&mut self) {
        if self.values.capacity() > self.values.len() * 2 {
            self.values.shrink_to_fit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj() -> Object {
        Object::new(
            ClassId(0),
            Mrid::parse("70c4656c-f7a0-4319-98bb-84fb5e2e9b37"),
        )
    }

    #[test]
    fn values_stay_sorted_regardless_of_insertion_order() {
        let mut o = obj();
        o.push(AttrId(5), Value::Integer(5));
        o.push(AttrId(1), Value::Integer(1));
        o.push(AttrId(3), Value::Integer(3));
        let ids: Vec<u16> = o.slots().iter().map(|s| s.attr.0).collect();
        assert_eq!(ids, [1, 3, 5]);
    }

    #[test]
    fn many_valued_attributes_keep_every_occurrence_in_order() {
        let mut o = obj();
        o.push(AttrId(2), Value::Integer(1));
        o.push(AttrId(9), Value::Integer(99));
        o.push(AttrId(2), Value::Integer(2));
        assert_eq!(o.count(AttrId(2)), 2);
        let got: Vec<i64> = o
            .get_all(AttrId(2))
            .iter()
            .map(|s| s.value.as_i64().unwrap())
            .collect();
        assert_eq!(got, [1, 2]);
        assert_eq!(o.get(AttrId(2)).unwrap().as_i64(), Some(1));
    }

    #[test]
    fn set_replaces_all_occurrences() {
        let mut o = obj();
        o.push(AttrId(2), Value::Integer(1));
        o.push(AttrId(2), Value::Integer(2));
        o.push(AttrId(4), Value::Integer(4));
        o.set(AttrId(2), Value::Integer(9));
        assert_eq!(o.count(AttrId(2)), 1);
        assert_eq!(o.get(AttrId(2)).unwrap().as_i64(), Some(9));
        // Neighbouring attributes are untouched.
        assert_eq!(o.get(AttrId(4)).unwrap().as_i64(), Some(4));
    }

    #[test]
    fn remove_value_removes_exactly_one_matching_occurrence() {
        let mut o = obj();
        o.push(AttrId(2), Value::Integer(1));
        o.push(AttrId(2), Value::Integer(2));
        assert!(o.remove_value(AttrId(2), &Value::Integer(1)));
        assert_eq!(o.count(AttrId(2)), 1);
        assert_eq!(o.get(AttrId(2)).unwrap().as_i64(), Some(2));
        assert!(!o.remove_value(AttrId(2), &Value::Integer(7)));
    }

    #[test]
    fn missing_attributes_report_absent() {
        let o = obj();
        assert!(!o.has(AttrId(1)));
        assert_eq!(o.get(AttrId(1)), None);
        assert!(o.get_all(AttrId(1)).is_empty());
    }

    #[test]
    fn absent_attribute_does_not_return_its_neighbour() {
        // Sorted storage means the lookup position for a missing attribute is occupied
        // by the next one; the identity of what is found must be checked.
        let mut o = obj();
        o.push(AttrId(1), Value::Integer(10));
        o.push(AttrId(3), Value::Integer(30));
        for missing in [AttrId(0), AttrId(2), AttrId(4)] {
            assert_eq!(o.get(missing), None, "{missing:?}");
            assert_eq!(o.get_slot(missing), None, "{missing:?}");
            assert!(!o.has(missing), "{missing:?}");
            assert_eq!(o.count(missing), 0, "{missing:?}");
        }
        assert_eq!(o.get(AttrId(1)).unwrap().as_i64(), Some(10));
        assert_eq!(o.get(AttrId(3)).unwrap().as_i64(), Some(30));
    }

    #[test]
    fn provenance_is_recorded_and_merged() {
        let mut o = obj();
        o.push_in(0b0100, AttrId(1), Value::Integer(1));
        assert_eq!(o.get_slot(AttrId(1)).unwrap().profiles, 0b0100);
        // The same value seen again in another profile widens its provenance.
        o.merge_provenance(AttrId(1), &Value::Integer(1), 0b1000);
        assert_eq!(o.get_slot(AttrId(1)).unwrap().profiles, 0b1100);
        // A different value is left alone.
        o.merge_provenance(AttrId(1), &Value::Integer(2), 0b0001);
        assert_eq!(o.get_slot(AttrId(1)).unwrap().profiles, 0b1100);
    }

    #[test]
    fn provenance_defaults_to_unknown() {
        let mut o = obj();
        o.set(AttrId(1), Value::Integer(1));
        assert_eq!(o.get_slot(AttrId(1)).unwrap().profiles, 0);
    }
}
