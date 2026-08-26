//! The dataset: a set of CIM objects assembled from one or more instance files.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::header::ModelHeader;
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::{AttrId, AttrKind, ClassId, ProfileId, ProfileMask, Schema};
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
    objects: Vec<Object>,
    /// `None` marks a removed slot, keeping [`ObjectId`]s stable.
    alive: Vec<bool>,
    by_mrid: HashMap<Mrid, ObjectId>,
    by_class: HashMap<ClassId, Vec<ObjectId>>,
    headers: Vec<ModelHeader>,
    link_policy: LinkPolicy,
}

impl Dataset {
    pub fn new(schema: &'static Schema) -> Dataset {
        Dataset {
            schema,
            objects: Vec::new(),
            alive: Vec::new(),
            by_mrid: HashMap::new(),
            by_class: HashMap::new(),
            headers: Vec::new(),
            link_policy: LinkPolicy::default(),
        }
    }

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

    pub fn push_header(&mut self, header: ModelHeader) {
        self.headers.push(header);
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
        match self.alive.get(id.index()) {
            Some(true) => self.objects.get(id.index()),
            _ => None,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        match self.alive.get(id.index()) {
            Some(true) => self.objects.get_mut(id.index()),
            _ => None,
        }
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
        self.by_class.entry(object.class).or_default().push(id);
        self.by_mrid.insert(object.mrid.clone(), id);
        self.objects.push(object);
        self.alive.push(true);
        id
    }

    fn merge_into(&mut self, target: ObjectId, incoming: Object) {
        let schema = self.schema;
        let old_class = self.objects[target.index()].class;

        // Prefer the most specific class: a profile may describe an object through a
        // base class (e.g. SSH writing to `Terminal` where EQ knew a subclass).
        let new_class = if schema.is_a(incoming.class, old_class) {
            incoming.class
        } else {
            old_class
        };
        if new_class != old_class {
            if let Some(v) = self.by_class.get_mut(&old_class) {
                v.retain(|&x| x != target);
            }
            self.by_class.entry(new_class).or_default().push(target);
            self.objects[target.index()].set_class(new_class);
        }

        let obj = &mut self.objects[target.index()];
        obj.mark_profile(incoming.profiles);
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
                // Two profiles disagreeing on a single-valued attribute is a data error.
                // Take the newer value, but keep the union of provenance so that both
                // files still carry the attribute on export instead of one losing it.
                let inherited = obj.get_slot(slot.attr).map_or(0, |s| s.profiles);
                let mut slot = slot;
                slot.profiles |= inherited;
                obj.set_slot(slot);
            }
        }
    }

    /// Change an object's class, keeping the per-class index consistent.
    ///
    /// Used when a difference model reclassifies an object.
    pub fn reclassify(&mut self, id: ObjectId, class: ClassId) {
        let Some(old) = self.get(id).map(|o| o.class()) else {
            return;
        };
        if old == class {
            return;
        }
        if let Some(v) = self.by_class.get_mut(&old) {
            v.retain(|&x| x != id);
        }
        self.by_class.entry(class).or_default().push(id);
        self.objects[id.index()].set_class(class);
    }

    /// Remove an object. Its [`ObjectId`] is never reused.
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        if !matches!(self.alive.get(id.index()), Some(true)) {
            return None;
        }
        self.alive[id.index()] = false;
        let obj = &self.objects[id.index()];
        self.by_mrid.remove(&obj.mrid);
        if let Some(v) = self.by_class.get_mut(&obj.class) {
            v.retain(|&x| x != id);
        }
        Some(std::mem::replace(
            &mut self.objects[id.index()],
            Object::new(ClassId(0), Mrid::parse("")),
        ))
    }

    /// Every live object.
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &Object)> + '_ {
        self.objects
            .iter()
            .enumerate()
            .filter(|(i, _)| self.alive[*i])
            .map(|(i, o)| (ObjectId(i as u32), o))
    }

    /// Objects of exactly `class`, excluding subclasses.
    pub fn iter_exact(&self, class: ClassId) -> impl Iterator<Item = (ObjectId, &Object)> + '_ {
        self.by_class
            .get(&class)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(move |&id| self.get(id).map(|o| (id, o)))
    }

    /// Objects of `class` or any subclass — usually what callers want.
    pub fn iter_class(&self, class: ClassId) -> impl Iterator<Item = (ObjectId, &Object)> + '_ {
        self.iter()
            .filter(move |(_, o)| self.schema.is_a(o.class, class))
    }

    pub fn count_class(&self, class: ClassId) -> usize {
        self.iter_class(class).count()
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

    /// Release unused capacity after bulk loading.
    pub fn shrink_to_fit(&mut self) {
        for o in &mut self.objects {
            o.shrink();
        }
        self.objects.shrink_to_fit();
        self.by_mrid.shrink_to_fit();
        for v in self.by_class.values_mut() {
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

/// A precomputed index from referenced object to the objects that reference it.
///
/// Building this once turns repeated [`Dataset::referrers`] scans into hash lookups,
/// which matters for traversals such as "all terminals of this equipment".
#[derive(Debug)]
pub struct InverseIndex {
    map: HashMap<(AttrId, Mrid), Vec<ObjectId>>,
}

impl InverseIndex {
    /// Index every association in the dataset.
    pub fn build(dataset: &Dataset) -> InverseIndex {
        let mut map: HashMap<(AttrId, Mrid), Vec<ObjectId>> = HashMap::new();
        for (id, obj) in dataset.iter() {
            for slot in obj.slots() {
                if matches!(
                    dataset.schema().attr(slot.attr).kind,
                    AttrKind::Association { .. }
                ) && let Value::Reference(m) = &slot.value
                {
                    map.entry((slot.attr, m.clone())).or_default().push(id);
                }
            }
        }
        InverseIndex { map }
    }

    /// Objects whose `attr` points at `target`.
    pub fn referrers(&self, attr: AttrId, target: &Mrid) -> &[ObjectId] {
        self.map
            .get(&(attr, target.clone()))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
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

/// Which profile a set of objects came from, used when loading a file.
#[derive(Clone, Copy, Debug)]
pub struct LoadContext {
    pub profile: Option<ProfileId>,
}

impl LoadContext {
    pub fn mask(&self) -> ProfileMask {
        self.profile.map(|p| p.mask()).unwrap_or(0)
    }
}
