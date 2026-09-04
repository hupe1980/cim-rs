//! The dataset: a set of CIM objects assembled from one or more instance files.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::header::{DifferenceModel, ModelHeader};
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::{AttrId, AttrKind, ClassId, ProfileMask, Schema};
use crate::value::Value;

/// Handle to an object inside a [`Dataset`].
///
/// Stable for the lifetime of the dataset unless [`Dataset::remove`] is called.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ObjectId(pub u32);

impl ObjectId {
    #[inline]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// How to treat references that do not resolve within the dataset.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LinkPolicy {
    /// Unresolved references are an error. Appropriate for a fully assembled model.
    Strict,
    /// Unresolved references are reported as diagnostics and otherwise tolerated.
    ///
    /// This is the default because real exchanges are routinely partial: a Steady State
    /// Hypothesis file references equipment defined in a separate Equipment file, and a
    /// model may legitimately be read before its boundary set is loaded.
    #[default]
    Lenient,
}

/// A set of CIM objects together with the headers of the files that produced them.
///
/// A dataset is the unit of real-world exchange: CGMES splits one grid model across
/// several profile files (EQ, SSH, TP, SV, ...) that describe the *same* objects. Loading
/// them into one dataset merges each object's attributes across profiles.
#[derive(Debug)]
pub struct Dataset {
    schema: &'static Schema,
    /// `None` marks a removed slot, keeping [`ObjectId`]s stable.
    objects: Vec<Option<Object>>,
    by_mrid: HashMap<Mrid, ObjectId>,
    /// Objects of exactly each class, indexed by [`ClassId`].
    ///
    /// A dense vector rather than a map: a schema has a few hundred classes, so the
    /// per-class bucket is a direct index instead of a hash lookup on every iteration.
    by_class: Vec<Vec<ObjectId>>,
    /// Where each object sits inside its class bucket, indexed by [`ObjectId`].
    ///
    /// This is what keeps removing an object and moving it between classes O(1). Without
    /// it both had to scan the bucket, so deleting every object of a class — an ordinary
    /// way to filter a model — cost the square of their number, and so did loading a
    /// Steady State Hypothesis file before its Equipment file, since every object it
    /// carries as a generic `<cim:Equipment>` is then promoted to its real class.
    class_slot: Vec<u32>,
    headers: Vec<ModelHeader>,
    /// Objects each loaded file contributed, indexed by header slot.
    by_source: Vec<Vec<ObjectId>>,
    differences: Vec<DifferenceModel>,
    link_policy: LinkPolicy,
    /// Single-valued attributes two files gave different values for.
    conflicts: Vec<MergeConflict>,
    /// Conflicts beyond [`Dataset::MAX_CONFLICTS`], counted but not kept.
    conflicts_dropped: usize,
}

/// Two files disagreeing about a single-valued attribute of one object.
///
/// Merging by mRID is what makes multi-profile assembly work, and for almost everything it
/// is unambiguous: each file contributes attributes the others do not have, or repeats one
/// with the same value. A *different* value for a single-valued attribute is neither — it
/// is a defect in the model set, and one that has to be visible, because resolving it means
/// discarding one of the two readings.
///
/// The last file read wins, so that a change file applied over a base model has the effect
/// its author intended. Which one that is depends on load order; that the conflict happened
/// does not, and is what [`Dataset::merge_conflicts`] reports.
#[derive(Clone, Debug)]
pub struct MergeConflict {
    pub object: Mrid,
    pub attr: AttrId,
    /// The value now stored — the one the later file supplied.
    pub kept: Value,
    /// The value the earlier file supplied, which is no longer in the model.
    pub discarded: Value,
}

impl Dataset {
    pub fn new(schema: &'static Schema) -> Dataset {
        Dataset {
            schema,
            objects: Vec::new(),
            by_mrid: HashMap::new(),
            by_class: vec![Vec::new(); schema.classes.len()],
            class_slot: Vec::new(),
            headers: Vec::new(),
            by_source: Vec::new(),
            differences: Vec::new(),
            link_policy: LinkPolicy::default(),
            conflicts: Vec::new(),
            conflicts_dropped: 0,
        }
    }

    /// How many merge conflicts are retained before only their number is kept.
    ///
    /// A model set assembled from mismatched files can disagree about every object; the
    /// cap keeps that from turning into memory proportional to the model.
    pub const MAX_CONFLICTS: usize = 1000;

    pub fn with_link_policy(mut self, policy: LinkPolicy) -> Dataset {
        self.link_policy = policy;
        self
    }

    #[inline]
    pub fn schema(&self) -> &'static Schema {
        self.schema
    }

    #[inline]
    pub fn link_policy(&self) -> LinkPolicy {
        self.link_policy
    }

    /// Headers of every file loaded into this dataset, in load order.
    #[inline]
    pub fn headers(&self) -> &[ModelHeader] {
        &self.headers
    }

    /// Register a file's header and return the source slot its objects belong to.
    ///
    /// The reader calls this before storing a file's objects, so each object is recorded
    /// against the file it came from and an export can reproduce that file set.
    pub fn push_header(&mut self, header: ModelHeader) -> usize {
        self.headers.push(header);
        self.by_source.push(Vec::new());
        self.headers.len() - 1
    }

    /// Replace the header in slot `index`, keeping the objects already recorded against it.
    ///
    /// A document that names itself only *after* its first object — legal XML, and not what
    /// IEC 61970-552's layout suggests — claims a slot before its `md:FullModel` is read.
    /// Pushing the real header afterwards would leave the file described twice and its
    /// objects attached to the placeholder.
    pub fn set_header(&mut self, index: usize, header: ModelHeader) {
        if let Some(slot) = self.headers.get_mut(index) {
            *slot = header;
        }
    }

    /// Record that the file in header slot `source` contained `id`.
    pub fn record_source(&mut self, source: usize, id: ObjectId) {
        let Some(list) = self.by_source.get_mut(source) else {
            return;
        };
        // One object element per file, so a repeat means the file described it twice.
        if list.last() != Some(&id) {
            list.push(id);
        }
        if let Some(Some(o)) = self.objects.get_mut(id.index()) {
            o.from_file = true;
        }
    }

    /// Objects the file in header slot `source` contributed, in the order it listed them.
    ///
    /// Profile provenance alone cannot say which *file* a value came from, and in a merged
    /// common grid model that matters: two modelling authorities each contribute an
    /// Equipment file, and writing the model back must put each authority's equipment in
    /// its own file rather than the union in both.
    ///
    /// Objects since removed with [`Dataset::remove`] are skipped. The index is not
    /// rebuilt on removal — an object appears in as many files as there are profiles
    /// describing it, so cleaning it out would mean either a per-object source set or a
    /// scan of every file's list — so the liveness check happens here instead, where it
    /// costs one lookup per entry and cannot be forgotten by a caller.
    pub fn objects_from(&self, source: usize) -> impl Iterator<Item = ObjectId> + '_ {
        self.by_source
            .get(source)
            .map_or(&[][..], |v| v.as_slice())
            .iter()
            .copied()
            .filter(move |&id| self.get(id).is_some())
    }

    /// Difference models read into this dataset, in load order.
    ///
    /// A difference is not applied on load — [`Dataset::apply_difference`] does that — but
    /// it is kept so that a model set containing change files can be written back whole.
    pub fn differences(&self) -> &[DifferenceModel] {
        &self.differences
    }

    pub fn push_difference(&mut self, diff: DifferenceModel) {
        self.differences.push(diff);
    }

    /// Profiles present across all loaded headers.
    pub fn profiles(&self) -> ProfileMask {
        self.headers.iter().fold(0, |acc, h| {
            acc | h
                .profiles
                .iter()
                .filter_map(|iri| self.schema.profile_by_iri(iri))
                .fold(0, |a, p| a | p.mask())
        })
    }

    /// Number of live objects.
    pub fn len(&self) -> usize {
        self.by_mrid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_mrid.is_empty()
    }

    #[inline]
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.index())?.as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(id.index())?.as_mut()
    }

    pub fn find(&self, mrid: &Mrid) -> Option<ObjectId> {
        self.by_mrid.get(mrid).copied()
    }

    pub fn by_mrid(&self, mrid: &Mrid) -> Option<&Object> {
        self.find(mrid).and_then(|id| self.get(id))
    }

    /// Insert an object, or merge into the existing one with the same mRID.
    ///
    /// Merging is what makes multi-profile assembly work: when an SSH file describes an
    /// object already known from EQ, its attributes are added rather than replacing the
    /// object. A more specific class wins, since profiles may describe the same object
    /// through a base class.
    pub fn insert(&mut self, object: Object) -> ObjectId {
        if let Some(&existing) = self.by_mrid.get(&object.mrid) {
            self.merge_into(existing, object);
            return existing;
        }
        let id = ObjectId(self.objects.len() as u32);
        let class = object.class;
        self.by_mrid.insert(object.mrid.clone(), id);
        self.objects.push(Some(object));
        self.class_slot.push(0);
        self.attach_to_class(id, class);
        id
    }

    /// Absorb another dataset: its objects, its file headers, which file each object came
    /// from, and any change files it carries.
    ///
    /// The point is assembly that is not sequential. `load_files` reads one document after
    /// another into one dataset because the merge has to happen somewhere, but reading is
    /// the expensive half and it is embarrassingly parallel — a model set is a handful of
    /// independent files. With this, each can be parsed into its own dataset on its own
    /// thread and the results combined afterwards, and the outcome is the same model:
    /// merging by mRID is what makes multi-profile assembly work, and it does not care
    /// which dataset an object arrived in.
    ///
    // An example has to name a vintage, and a vintage is a feature. See `Dataset::view`.
    #[cfg_attr(
        feature = "cgmes3",
        doc = r#"
```no_run
# use cim_rs::prelude::*;
# use cim_rs::cgmes3::SCHEMA;
# fn main() -> cim_rs::Result<()> {
let mut model = Dataset::new(SCHEMA);
for part in ["EQ.xml", "SSH.xml", "TP.xml", "SV.xml"]
    .map(|f| Dataset::load(SCHEMA, [f]))
{
    model.merge(part?)?;
}
# Ok(()) }
```
"#
    )]
    ///
    /// One order-dependence survives, and it is the same one `load_files` has: where two
    /// files give a single-valued attribute two different values, the later merge wins and
    /// the discarded reading is recorded as a [`MergeConflict`]. Conflicts `other`
    /// recorded on its own are carried over rather than lost.
    ///
    /// Fails if the two datasets were built from different schema vintages, because their
    /// [`AttrId`]s index different tables.
    pub fn merge(&mut self, other: Dataset) -> Result<()> {
        if !std::ptr::eq(self.schema, other.schema) {
            return Err(Error::SchemaMismatch {
                left: self.schema.vintage,
                right: other.schema.vintage,
            });
        }
        let base = self.headers.len();
        for h in other.headers {
            self.push_header(h);
        }
        for d in other.differences {
            self.push_difference(d);
        }

        // `other`'s handles do not survive the move — its objects merge into whatever this
        // dataset already holds under the same identifier — so the per-file index is
        // rebuilt through a translation table rather than copied.
        let mut moved: Vec<Option<ObjectId>> = vec![None; other.objects.len()];
        for (i, slot) in other.objects.into_iter().enumerate() {
            if let Some(o) = slot {
                moved[i] = Some(self.insert(o));
            }
        }
        for (i, ids) in other.by_source.into_iter().enumerate() {
            for id in ids {
                if let Some(new) = moved.get(id.index()).copied().flatten() {
                    self.record_source(base + i, new);
                }
            }
        }

        for c in other.conflicts {
            self.record_conflict(c);
        }
        self.conflicts_dropped += other.conflicts_dropped;
        Ok(())
    }

    /// Record `id` in `class`'s bucket, remembering where it landed.
    fn attach_to_class(&mut self, id: ObjectId, class: ClassId) {
        let Some(bucket) = self.by_class.get_mut(class.index()) else {
            return;
        };
        self.class_slot[id.index()] = bucket.len() as u32;
        bucket.push(id);
    }

    /// Take `id` out of `class`'s bucket in constant time.
    ///
    /// The last entry moves into the vacated slot, so its own recorded position has to be
    /// corrected — the whole point of keeping the position rather than searching for it.
    fn detach_from_class(&mut self, id: ObjectId, class: ClassId) {
        let slot = self.class_slot[id.index()] as usize;
        let Some(bucket) = self.by_class.get_mut(class.index()) else {
            return;
        };
        debug_assert_eq!(bucket.get(slot), Some(&id), "class index out of step");
        bucket.swap_remove(slot);
        if let Some(&moved) = bucket.get(slot) {
            self.class_slot[moved.index()] = slot as u32;
        }
    }

    fn merge_into(&mut self, target: ObjectId, incoming: Object) {
        let schema = self.schema;
        let Some(old_class) = self.get(target).map(|o| o.class) else {
            return;
        };

        // Prefer the most specific class: a profile may describe an object through a
        // base class (e.g. SSH writing to `Terminal` where EQ knew a subclass).
        let new_class = if schema.is_a(incoming.class, old_class) {
            incoming.class
        } else {
            old_class
        };
        if new_class != old_class {
            self.detach_from_class(target, old_class);
            self.attach_to_class(target, new_class);
        }

        let mut conflicts: Vec<MergeConflict> = Vec::new();
        let Some(obj) = self.objects[target.index()].as_mut() else {
            return;
        };
        obj.set_class(new_class);
        obj.mark_profile(incoming.profiles);
        obj.from_file |= incoming.from_file;
        for slot in incoming.values {
            let many = schema.attr(slot.attr).mult.is_many();
            let duplicate = obj
                .get_all(slot.attr)
                .iter()
                .any(|existing| existing.value == slot.value);
            if duplicate {
                // The same value in two profiles: widen its provenance rather than
                // storing it twice, so an export reproduces both files.
                obj.merge_provenance(slot.attr, &slot.value, slot.profiles);
            } else if many {
                obj.push_slot(slot);
            } else {
                // Two files disagreeing on a single-valued attribute is a data error, and
                // resolving it means throwing one of the two readings away — so it is
                // recorded rather than swallowed. The newer value wins, which is what
                // makes a change file applied over a base model do what its author meant.
                // Provenance is unioned so both files still carry the attribute on export
                // instead of one of them losing it.
                let existing = obj.get_slot(slot.attr);
                let inherited = existing.map_or(0, |s| s.profiles);
                if let Some(old) = existing {
                    conflicts.push(MergeConflict {
                        object: obj.mrid().clone(),
                        attr: slot.attr,
                        kept: slot.value.clone(),
                        discarded: old.value.clone(),
                    });
                }
                let mut slot = slot;
                slot.profiles |= inherited;
                obj.set_slot(slot);
            }
        }
        for c in conflicts {
            self.record_conflict(c);
        }
    }

    fn record_conflict(&mut self, conflict: MergeConflict) {
        if self.conflicts.len() < Self::MAX_CONFLICTS {
            self.conflicts.push(conflict);
        } else {
            self.conflicts_dropped += 1;
        }
    }

    /// Single-valued attributes two loaded files gave different values for.
    ///
    /// Empty for a consistent model set. A non-empty result means the model this dataset
    /// holds is not the union of its files: for each entry, one file's value was discarded.
    /// [`validate`](mod@crate::validate) turns these into
    /// [`Rule::ConflictingValue`](crate::Rule::ConflictingValue) diagnostics.
    pub fn merge_conflicts(&self) -> &[MergeConflict] {
        &self.conflicts
    }

    /// Conflicts that occurred beyond [`Dataset::MAX_CONFLICTS`] and were only counted.
    pub fn merge_conflicts_dropped(&self) -> usize {
        self.conflicts_dropped
    }

    /// Change an object's class, keeping the per-class index consistent, and return any
    /// values the new class cannot carry.
    ///
    /// Used when a difference model reclassifies an object — IEC 61970-552 treats an
    /// object's type as a statement like any other, and the published conformity change
    /// sets replace a `LinearShuntCompensator` with a `NonlinearShuntCompensator` under one
    /// identifier.
    ///
    /// **Values are shed, not kept.** Narrowing to a subclass sheds nothing, because a
    /// subclass has every attribute its parent has. Moving sideways does:
    /// `LinearShuntCompensator.bPerSection` has no meaning on a `NonlinearShuntCompensator`
    /// and no place in one's instance file. Keeping such a value looks harmless and is not
    /// — nothing afterwards would ever look at it, because the writer emits a class's own
    /// attributes and validation checks a class's own attributes, so it would vanish
    /// without a word on the next export. Handing it back makes the loss the caller's to
    /// report, which is what [`Dataset::apply_difference`] does.
    pub fn reclassify(&mut self, id: ObjectId, class: ClassId) -> Vec<crate::object::Slot> {
        let schema = self.schema;
        let Some(old) = self.get(id).map(|o| o.class()) else {
            return Vec::new();
        };
        if old == class {
            return Vec::new();
        }
        self.detach_from_class(id, old);
        self.attach_to_class(id, class);
        let Some(o) = self.objects[id.index()].as_mut() else {
            return Vec::new();
        };
        o.set_class(class);
        o.retain_slots(|slot| schema.is_a(class, schema.attr(slot.attr).owner))
    }

    /// Remove an object. Its [`ObjectId`] is never reused.
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        let obj = self.objects.get_mut(id.index())?.take()?;
        self.by_mrid.remove(&obj.mrid);
        self.detach_from_class(id, obj.class);
        Some(obj)
    }

    /// Every live object.
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &Object)> + '_ {
        self.objects
            .iter()
            .enumerate()
            .filter_map(|(i, o)| Some((ObjectId(i as u32), o.as_ref()?)))
    }

    /// Objects of exactly `class`, excluding subclasses.
    pub fn iter_exact(&self, class: ClassId) -> impl Iterator<Item = (ObjectId, &Object)> + '_ {
        self.by_class
            .get(class.index())
            .map_or(&[][..], |v| v.as_slice())
            .iter()
            .filter_map(move |&id| self.get(id).map(|o| (id, o)))
    }

    /// Objects of `class` or any subclass — usually what callers want.
    ///
    /// Walks the per-class buckets of `class` and its descendants rather than scanning
    /// the dataset, so asking for every `ACLineSegment` costs their number rather than
    /// the model's. Order is unspecified but stable for a given load.
    pub fn iter_class(&self, class: ClassId) -> impl Iterator<Item = (ObjectId, &Object)> + '_ {
        self.schema
            .class(class)
            .descendants
            .iter()
            .flat_map(move |&c| self.iter_exact(c))
    }

    pub fn count_class(&self, class: ClassId) -> usize {
        self.schema
            .class(class)
            .descendants
            .iter()
            .map(|c| self.by_class.get(c.index()).map_or(0, |v| v.len()))
            .sum()
    }

    /// Resolve a single-valued association to the referenced object.
    pub fn follow(&self, object: &Object, attr: AttrId) -> Option<&Object> {
        let mrid = object.get(attr)?.as_reference()?;
        self.by_mrid(mrid)
    }

    /// Resolve every value of a many-valued association.
    ///
    /// References that do not resolve are skipped; use
    /// [`validate`](crate::validate::validate) to report them.
    pub fn follow_all<'a>(
        &'a self,
        object: &'a Object,
        attr: AttrId,
    ) -> impl Iterator<Item = &'a Object> + 'a {
        object
            .get_all(attr)
            .iter()
            .filter_map(|s| s.value.as_reference())
            .filter_map(move |m| self.by_mrid(m))
    }

    /// Objects whose association `attr` points at `target`.
    ///
    /// This walks the inverse of a stored association. Inverse roles are deliberately not
    /// stored on both sides — IEC 61970-501 marks exactly one side of each association as
    /// serialized — so this is computed. Build an [`InverseIndex`] when querying repeatedly.
    pub fn referrers<'a>(
        &'a self,
        target: &'a Mrid,
        attr: AttrId,
    ) -> impl Iterator<Item = (ObjectId, &'a Object)> + 'a {
        self.iter().filter(move |(_, o)| {
            o.get_all(attr)
                .iter()
                .any(|s| s.value.as_reference() == Some(target))
        })
    }

    /// The namespace under which this crate derives identifiers.
    ///
    /// A fixed version-4 UUID, so that a derived mRID cannot collide with one derived for
    /// a different purpose, here or anywhere else.
    pub const DERIVED_NS: Mrid = Mrid::from_uuid_bytes([
        0x2b, 0x0f, 0x9a, 0x71, 0x2c, 0x8e, 0x4a, 0x2f, 0x9c, 0x1a, 0x3f, 0x7f, 0x9d, 0x4b, 0x6e,
        0x10,
    ]);

    /// A UUID that identifies *what this model contains*.
    ///
    /// Exporting a model needs a `md:FullModel rdf:about`, and a model built
    /// programmatically has none. A random one would make every export of an unchanged
    /// model differ from the last, which defeats a deterministic writer; a fixed one would
    /// give two unrelated models the same identifier, which is worse. Deriving it from the
    /// content gives both properties: unchanged model, unchanged identifier; changed
    /// model, changed identifier.
    ///
    /// The fingerprint covers the vintage and, for every object in mRID order, its
    /// identifier, its class and every stored value. What it deliberately does not cover
    /// is anything about the *files* — headers, per-file provenance, load order — because
    /// two ways of packaging one model are still that model.
    ///
    /// It is not a cryptographic commitment and is not offered as one: it is derived with
    /// SHA-1, which is broken for collision resistance, and identifies a model against
    /// accident rather than against an adversary.
    pub fn content_id(&self) -> Mrid {
        // Objects in mRID order, so the result does not depend on load order, into one
        // buffer built from raw bytes rather than rendered text — a nation-scale model is
        // a quarter of a million objects and this should not allocate per object.
        // Separators keep two different models from hashing alike by running together at
        // the seams.
        let mut ids: Vec<(&Mrid, ObjectId)> = self.iter().map(|(id, o)| (o.mrid(), id)).collect();
        ids.sort_by(|a, b| a.0.cmp(b.0));

        let mut buf: Vec<u8> = Vec::with_capacity(self.len() * 24 + 32);
        buf.extend_from_slice(self.schema.vintage.as_bytes());
        for (mrid, id) in ids {
            buf.push(0x1e);
            match mrid.as_uuid_bytes() {
                Some(b) => buf.extend_from_slice(b),
                None => buf.extend_from_slice(mrid.canonical().as_bytes()),
            }
            let Some(o) = self.get(id) else { continue };
            buf.push(0x1f);
            buf.extend_from_slice(&o.class().0.to_le_bytes());
            for slot in o.slots() {
                buf.extend_from_slice(&slot.attr.0.to_le_bytes());
                fold_value(&mut buf, &slot.value);
            }
        }
        Mrid::new_v5(&Self::DERIVED_NS, &buf)
    }

    /// Release unused capacity after bulk loading.
    pub fn shrink_to_fit(&mut self) {
        for o in self.objects.iter_mut().flatten() {
            o.shrink();
        }
        self.objects.shrink_to_fit();
        self.class_slot.shrink_to_fit();
        self.by_mrid.shrink_to_fit();
        for v in self.by_class.iter_mut() {
            v.shrink_to_fit();
        }
    }

    /// References that point at objects not present in the dataset.
    pub fn dangling_references(&self) -> Vec<(ObjectId, AttrId, Mrid)> {
        let mut out = Vec::new();
        for (id, obj) in self.iter() {
            for slot in obj.slots() {
                if let Value::Reference(m) = &slot.value
                    && !self.by_mrid.contains_key(m)
                {
                    out.push((id, slot.attr, m.clone()));
                }
            }
        }
        out
    }

    /// Fail if the link policy is [`LinkPolicy::Strict`] and references are unresolved.
    pub fn check_links(&self) -> Result<()> {
        if self.link_policy == LinkPolicy::Strict {
            let dangling = self.dangling_references();
            if !dangling.is_empty() {
                let (id, attr, mrid) = &dangling[0];
                let owner = self
                    .get(*id)
                    .map(|o| o.mrid().canonical())
                    .unwrap_or_default();
                return Err(Error::DanglingReference {
                    object: owner,
                    attribute: self.schema.attr(*attr).name,
                    target: mrid.canonical(),
                    total: dangling.len(),
                });
            }
        }
        Ok(())
    }
}

/// Append one value to a content fingerprint, in its stored rather than its printed form.
///
/// Each variant is tagged, so a text value cannot fold to the same bytes as a reference
/// that happens to spell the same thing; floats go in by bit pattern, which is exact where
/// a rendered form would depend on the formatter.
fn fold_value(buf: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Boolean(b) => buf.extend_from_slice(&[1, *b as u8]),
        Value::Integer(i) => {
            buf.push(2);
            buf.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            buf.push(3);
            // The number, not its spelling: two models that differ only in how a producer
            // formatted a float are the same model, and `Real`'s equality says so too.
            buf.extend_from_slice(&f.get().to_bits().to_le_bytes());
        }
        Value::Text(s) => {
            buf.push(4);
            buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        }
        Value::Enum(e) => {
            buf.push(5);
            buf.extend_from_slice(&e.0.to_le_bytes());
        }
        Value::Reference(m) => {
            buf.push(6);
            match m.as_uuid_bytes() {
                Some(b) => buf.extend_from_slice(b),
                None => buf.extend_from_slice(m.canonical().as_bytes()),
            }
        }
        Value::Compound(c) => {
            buf.push(7);
            buf.extend_from_slice(&c.class().0.to_le_bytes());
            for (a, v) in c.values() {
                buf.extend_from_slice(&a.0.to_le_bytes());
                fold_value(buf, v);
            }
        }
    }
    buf.push(0x1d);
}

/// A precomputed index from referenced object to the objects that reference it.
///
/// Building this once turns repeated [`Dataset::referrers`] scans into hash lookups,
/// which matters for traversals such as "all terminals of this equipment".
#[derive(Debug)]
pub struct InverseIndex {
    /// Keyed by target first so a lookup borrows the identifier rather than cloning it.
    map: HashMap<Mrid, HashMap<AttrId, Vec<ObjectId>>>,
}

impl InverseIndex {
    /// Index every association in the dataset.
    pub fn build(dataset: &Dataset) -> InverseIndex {
        let mut map: HashMap<Mrid, HashMap<AttrId, Vec<ObjectId>>> = HashMap::new();
        for (id, obj) in dataset.iter() {
            for slot in obj.slots() {
                if matches!(
                    dataset.schema().attr(slot.attr).kind,
                    AttrKind::Association { .. }
                ) && let Value::Reference(m) = &slot.value
                {
                    map.entry(m.clone())
                        .or_default()
                        .entry(slot.attr)
                        .or_default()
                        .push(id);
                }
            }
        }
        InverseIndex { map }
    }

    /// Objects whose `attr` points at `target`.
    pub fn referrers(&self, attr: AttrId, target: &Mrid) -> &[ObjectId] {
        self.map
            .get(target)
            .and_then(|m| m.get(&attr))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Every object referring to `target`, whichever association it uses.
    pub fn all_referrers(&self, target: &Mrid) -> impl Iterator<Item = (AttrId, ObjectId)> + '_ {
        self.map.get(target).into_iter().flat_map(|m| {
            m.iter()
                .flat_map(|(a, v)| v.iter().map(move |id| (*a, *id)))
        })
    }

    /// Resolve the inverse of `attr` — the role declared by `cims:inverseRoleName` —
    /// for `target`, following whichever side the schema actually serializes.
    pub fn inverse_of<'a>(
        &'a self,
        schema: &Schema,
        attr: AttrId,
        target: &Mrid,
    ) -> &'a [ObjectId] {
        match schema.attr(attr).kind {
            AttrKind::Association {
                inverse: Some(inv), ..
            } => self.referrers(inv, target),
            _ => &[],
        }
    }
}

impl Dataset {
    /// Validate with the default checks.
    ///
    /// Shorthand for [`validate::validate`](crate::validate::validate).
    pub fn validate(&self) -> crate::error::Report {
        crate::validate::validate(self)
    }

    /// Validate with explicit options.
    pub fn validate_with(
        &self,
        options: &crate::validate::ValidateOptions,
    ) -> crate::error::Report {
        crate::validate::validate_with(self, options)
    }
}
