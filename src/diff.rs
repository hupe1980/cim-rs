//! Computing a difference model from two model states.
//!
//! # Why this exists
//!
//! IEC 61970-552 defines `dm:DifferenceModel` as the incremental form of exchange: rather
//! than resending a whole grid model, a producer sends the statements to retract and the
//! statements to assert. Reading, applying and writing one is
//! [`reader::read_difference`](crate::reader::read_difference),
//! [`Dataset::apply_difference`] and [`writer::write_difference`](crate::writer::write_difference).
//! This module supplies the piece those three imply and none of them provides:
//! **producing** a change set from a before and an after.
//!
//! It is the operation an EMS performs every time it publishes an update, and it is
//! decidable from what the crate already holds — objects keyed by mRID, values sorted by
//! attribute, profile provenance per value — so it costs a walk of the two models and no
//! new machinery.
//!
// An example has to name a vintage, and a vintage is a feature. See `Dataset::view`.
#![cfg_attr(
    feature = "cgmes3",
    doc = r#"
```no_run
use cim_rs::prelude::*;
use cim_rs::cgmes3::SCHEMA;
use cim_rs::diff::DiffOptions;

# fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
let base = Dataset::load(SCHEMA, ["base_EQ.xml", "base_SSH.xml"])?;
let updated = Dataset::load(SCHEMA, ["updated_EQ.xml", "updated_SSH.xml"])?;

let change = base.difference_to(&updated, &DiffOptions::default());
let out = std::fs::File::create("change_DIFF.xml")?;
cim_rs::writer::write_difference(SCHEMA, &change.model, out, &Default::default())?;
# Ok(())
# }
```
"#
)]
//!
//! # What the result guarantees
//!
//! Applying the result to the base reproduces the target:
//! `base.apply_difference(&base.difference_to(&target, o).model)` leaves `base` holding
//! every value `target` holds, and none it does not. That round trip is a test, not a
//! claim — see `tests/difference.rs`.
//!
//! # What a statement-level difference cannot say
//!
//! Two limits are the standard's, not this implementation's, and both are reported rather
//! than papered over:
//!
//! * **An object cannot be deleted, only emptied.** `rdf:parseType="Statements"` retracts
//!   statements; there is no statement that says "this identifier no longer denotes
//!   anything". An object present in the base and absent from the target therefore becomes
//!   a subject with no properties. [`Dataset::prune_empty`] removes such shells where a
//!   consumer wants them gone.
//! * **A compound has no statement form.** `Location.mainAddress` holds a `StreetAddress`
//!   inline, and IEC 61970-552's statement syntax has nowhere to put one. A changed
//!   compound is reported as [`Rule::Structure`] and left out,
//!   because emitting it as text would fabricate a value.

use crate::dataset::{Dataset, ObjectId};
use crate::error::{Diagnostic, Report, Rule};
use crate::header::{
    DifferenceModel, ModelHeader, ModelKind, QualifiedName, Statement, StatementValue,
};
use crate::mrid::Mrid;
use crate::object::{Object, Slot};
use crate::schema::{AttrId, AttrKind, ClassId, Primitive, ProfileMask, Schema};
use crate::value::Value;

/// Options controlling how a difference is computed.
#[derive(Clone, Debug, Default)]
pub struct DiffOptions {
    /// Compare only values belonging to these profiles; zero means "everything".
    ///
    /// Filtering follows the same rule as the CIM/XML writer
    /// ([`Schema::effective_profiles`]), so a change set restricted to Steady State
    /// Hypothesis contains exactly the values an SSH export would, and no equipment
    /// changes that belong in a different file.
    pub profiles: ProfileMask,
    /// Header to give the change set. Without one, a minimal `dm:DifferenceModel` header
    /// is derived from the target's own headers.
    pub header: Option<ModelHeader>,
}

impl DiffOptions {
    /// Restrict the comparison to a set of profiles.
    pub fn profiles(mut self, profiles: ProfileMask) -> Self {
        self.profiles = profiles;
        self
    }

    pub fn with_header(mut self, header: ModelHeader) -> Self {
        self.header = Some(header);
        self
    }
}

/// A computed change set, together with what could not be expressed.
#[derive(Debug)]
pub struct DiffReport {
    pub model: DifferenceModel,
    /// Differences the statement syntax cannot carry, and mismatches that prevented one.
    pub report: Report,
    /// Objects present only in the target.
    pub added: usize,
    /// Objects present only in the base.
    pub removed: usize,
    /// Objects present in both whose description changed.
    pub changed: usize,
}

impl DiffReport {
    /// Whether the two models describe the same thing.
    pub fn is_empty(&self) -> bool {
        self.model.forward.is_empty() && self.model.reverse.is_empty()
    }
}

impl Dataset {
    /// The change set that turns this model into `target`.
    ///
    /// See the [module documentation](self) for what the result guarantees and for the two
    /// things a statement-level difference cannot express.
    pub fn difference_to(&self, target: &Dataset, options: &DiffOptions) -> DiffReport {
        let schema = self.schema();
        let mut out = DiffReport {
            model: DifferenceModel {
                header: options
                    .header
                    .clone()
                    .unwrap_or_else(|| derived_header(target)),
                reverse: Vec::new(),
                forward: Vec::new(),
            },
            report: Report::default(),
            added: 0,
            removed: 0,
            changed: 0,
        };
        out.model.header.kind = ModelKind::Difference;

        // Comparing models built from different vocabularies would compare `AttrId`s that
        // mean different things, which is worse than refusing.
        if !std::ptr::eq(schema, target.schema()) {
            out.report.push(Diagnostic::error(
                Rule::Structure,
                format!(
                    "cannot diff a {} model against a {} model",
                    schema.vintage,
                    target.schema().vintage
                ),
            ));
            return out;
        }

        // mRID order, so a change set is reproducible and diffable between runs — the same
        // reason the CIM/XML writer sorts.
        for mrid in union_of_identifiers(self, target) {
            let before = self.by_mrid(&mrid);
            let after = target.by_mrid(&mrid);
            match (before, after) {
                (None, Some(new)) => {
                    let n = out.model.forward.len();
                    self.emit_all(new, true, options, &mut out.model.forward, &mut out.report);
                    if out.model.forward.len() > n {
                        out.added += 1;
                    }
                }
                (Some(old), None) => {
                    let n = out.model.reverse.len();
                    self.emit_all(old, false, options, &mut out.model.reverse, &mut out.report);
                    if out.model.reverse.len() > n {
                        out.removed += 1;
                    }
                }
                (Some(old), Some(new)) => {
                    // Most objects are untouched even in a large update, and comparing
                    // their slot vectors directly is a linear scan that allocates nothing
                    // — where the per-attribute path builds a vector per attribute. A
                    // provenance-only difference falls through to the slow path and
                    // correctly produces no statements, so this is a fast path and not a
                    // second definition of equality.
                    if old.class() == new.class() && old.slots() == new.slots() {
                        continue;
                    }
                    let (r, f) = (out.model.reverse.len(), out.model.forward.len());
                    self.emit_changes(old, new, options, &mut out);
                    if out.model.reverse.len() > r || out.model.forward.len() > f {
                        out.changed += 1;
                    }
                }
                (None, None) => {}
            }
        }

        // The other thing a statement-level difference cannot express. A changed compound
        // is already reported; deletion was only *counted*, which left the two limits of
        // the same syntax documented to different standards — and a caller who applies this
        // change set and compares content identifiers finds an object that is empty rather
        // than absent, with nothing in the report to explain it.
        //
        // One diagnostic rather than one per object: the count is the fact, and a report
        // that grows with the size of the change is the thing `max_diagnostics` exists to
        // prevent elsewhere.
        if out.removed > 0 {
            out.report.push(Diagnostic::warning(
                Rule::Structure,
                format!(
                    "{} object(s) are absent from the target and a difference model cannot \
                     say so: applying this retracts everything they said and leaves the \
                     identifiers behind. Dataset::prune_empty removes such shells",
                    out.removed
                ),
            ));
        }
        out
    }

    /// Every value of one object as statements, for an object that is wholly new or gone.
    fn emit_all(
        &self,
        obj: &Object,
        introduces: bool,
        options: &DiffOptions,
        into: &mut Vec<Statement>,
        report: &mut Report,
    ) {
        let schema = self.schema();
        let class = qualified(schema, obj.class());
        for slot in obj.slots() {
            if !in_scope(schema, slot, options.profiles) {
                continue;
            }
            match statement(schema, slot.attr, &slot.value) {
                Some(value) => into.push(Statement {
                    subject: obj.mrid().clone(),
                    class: Some(class.clone()),
                    // A change set that introduces an object identifies it with `rdf:ID`;
                    // one that retracts an existing object's statements refers to it with
                    // `rdf:about`, because it is not the file that defined it.
                    defines_subject: introduces,
                    predicate_ns: schema.namespace(schema.attr(slot.attr).ns).iri.to_owned(),
                    predicate: schema.attr(slot.attr).name.to_owned(),
                    value,
                }),
                None => report.push(inexpressible(schema, obj.mrid(), slot.attr)),
            }
        }
    }

    /// The statements that turn `old`'s description into `new`'s.
    fn emit_changes(
        &self,
        old: &Object,
        new: &Object,
        options: &DiffOptions,
        out: &mut DiffReport,
    ) {
        let schema = self.schema();
        // A difference names an object's class only when it is saying something about it.
        // IEC 61970-552 treats the type as a statement like any other, and the published
        // conformity change sets use exactly this to replace a `LinearShuntCompensator`
        // with a `NonlinearShuntCompensator` under one identifier.
        let reclassified = old.class() != new.class();
        let forward_class = reclassified.then(|| qualified(schema, new.class()));
        let reverse_class = reclassified.then(|| qualified(schema, old.class()));
        let forward_before = out.model.forward.len();

        for attr in attributes_of_either(old, new) {
            fn scoped<'o>(
                schema: &Schema,
                o: &'o Object,
                attr: AttrId,
                profiles: ProfileMask,
            ) -> Vec<&'o Value> {
                o.get_all(attr)
                    .iter()
                    .filter(|s| in_scope(schema, s, profiles))
                    .map(|s| &s.value)
                    .collect()
            }
            let before = scoped(schema, old, attr, options.profiles);
            let after = scoped(schema, new, attr, options.profiles);
            if before == after {
                continue;
            }
            // Many-valued attributes are compared as multisets: two occurrences of the
            // same value are two statements, and retracting one must not retract both.
            let mut removed = before.clone();
            let mut added: Vec<&Value> = Vec::new();
            for v in after {
                match removed.iter().position(|x| *x == v) {
                    Some(i) => {
                        removed.remove(i);
                    }
                    None => added.push(v),
                }
            }

            for (values, class, defines, into) in [
                (
                    removed,
                    reverse_class.clone(),
                    false,
                    &mut out.model.reverse,
                ),
                (added, forward_class.clone(), false, &mut out.model.forward),
            ] {
                for v in values {
                    match statement(schema, attr, v) {
                        Some(value) => into.push(Statement {
                            subject: new.mrid().clone(),
                            class: class.clone(),
                            defines_subject: defines,
                            predicate_ns: schema.namespace(schema.attr(attr).ns).iri.to_owned(),
                            predicate: schema.attr(attr).name.to_owned(),
                            value,
                        }),
                        None => out.report.push(inexpressible(schema, new.mrid(), attr)),
                    }
                }
            }
        }

        // A reclassification with no attribute change still has to be said, or applying
        // the difference would leave the object as the class it used to be.
        //
        // "No attribute change" is decided from the mark taken before this object was
        // handled, not by searching the accumulated statements: a change set can run to
        // hundreds of thousands of statements, and re-scanning them once per object turns
        // the whole comparison quadratic.
        if reclassified && out.model.forward.len() == forward_before {
            match mrid_attribute(schema, new) {
                Some(attr) => out.model.forward.push(Statement {
                    subject: new.mrid().clone(),
                    class: forward_class,
                    defines_subject: false,
                    predicate_ns: schema.namespace(schema.attr(attr).ns).iri.to_owned(),
                    predicate: schema.attr(attr).name.to_owned(),
                    value: StatementValue::Literal(new.mrid().canonical()),
                }),
                // A statement group names its class on a property element, so a class
                // change with no property to hang it on cannot be said at all. Every CGMES
                // class descends from `IdentifiedObject` and so has `mRID`, which is why
                // this is unreachable there — but "a profile is data", and a vocabulary
                // that declares a class outside that hierarchy would otherwise lose the
                // reclassification without a word.
                None => out.report.push(
                    Diagnostic::warning(
                        Rule::Structure,
                        format!(
                            "{} changed class from {} to {}, but {} has no attribute the \
                             statement could carry, so the change is not in the change set",
                            new.mrid().canonical(),
                            schema.class(old.class()).name,
                            schema.class(new.class()).name,
                            schema.class(new.class()).name,
                        ),
                    )
                    .with_class(schema.class(new.class()).name)
                    .with_object(new.mrid().clone()),
                ),
            }
        }
    }

    /// Remove objects that carry no values at all.
    ///
    /// Applying a difference that retracts everything an object said leaves the identifier
    /// behind with nothing attached, because IEC 61970-552's statement syntax has no way to
    /// say "this object is gone". Returns how many were removed.
    ///
    /// Deliberately separate from [`Dataset::apply_difference`]: a model legitimately
    /// contains objects described entirely by files that are not loaded, and silently
    /// deleting those would be worse than keeping a shell.
    pub fn prune_empty(&mut self) -> usize {
        let empty: Vec<ObjectId> = self
            .iter()
            .filter(|(_, o)| o.is_empty())
            .map(|(id, _)| id)
            .collect();
        let n = empty.len();
        for id in empty {
            self.remove(id);
        }
        n
    }
}

/// Every identifier either model knows, in a stable order.
fn union_of_identifiers(a: &Dataset, b: &Dataset) -> Vec<Mrid> {
    let mut out: Vec<Mrid> = a
        .iter()
        .chain(b.iter())
        .map(|(_, o)| o.mrid().clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Attributes either object carries, in schema order.
///
/// Both slot vectors are already sorted by [`AttrId`], so this is a merge rather than a
/// sort — and it must consider attributes the *class* does not declare, since a document
/// may set one the profile puts elsewhere.
fn attributes_of_either(a: &Object, b: &Object) -> Vec<AttrId> {
    let mut out: Vec<AttrId> = a
        .slots()
        .iter()
        .chain(b.slots())
        .map(|s: &Slot| s.attr)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether a stored value takes part in a comparison restricted to `profiles`.
///
/// The same rule the CIM/XML writer applies — [`Schema::effective_profiles`] — so a change
/// set restricted to a profile holds exactly what that profile's file would.
fn in_scope(schema: &Schema, slot: &Slot, profiles: ProfileMask) -> bool {
    profiles == 0 || schema.effective_profiles(slot.attr, slot.profiles) & profiles != 0
}

fn qualified(schema: &Schema, class: ClassId) -> QualifiedName {
    let def = schema.class(class);
    QualifiedName {
        ns: schema.namespace(def.ns).iri.to_owned(),
        local: def.name.to_owned(),
    }
}

/// `IdentifiedObject.mRID` as visible on this object, for a statement that carries no
/// other payload.
fn mrid_attribute(schema: &Schema, obj: &Object) -> Option<AttrId> {
    schema.attr_by_label(obj.class(), "mRID")
}

/// One stored value as a difference statement's value, or `None` if it has no such form.
fn statement(schema: &Schema, attr: AttrId, value: &Value) -> Option<StatementValue> {
    let kind = schema.attr(attr).kind;
    Some(match value {
        // A reference is written the way an instance file writes one, so a consumer that
        // reads the change set resolves it against the same model.
        Value::Reference(m) => StatementValue::Resource(m.to_rdf_reference()),
        Value::Enum(e) => {
            let ev = schema.enum_value(*e);
            StatementValue::Resource(format!("{}{}", schema.namespace(ev.ns).iri, ev.name))
        }
        // A compound is held inline, and `rdf:parseType="Statements"` has nowhere to put
        // one. Reporting that is the only honest answer.
        Value::Compound(_) => return None,
        other => StatementValue::Literal(other.to_lexical_as(lexical(schema, kind))?),
    })
}

fn lexical(schema: &Schema, kind: AttrKind) -> Primitive {
    match kind {
        AttrKind::Primitive(p) => p,
        AttrKind::Datatype(d) => schema.datatype(d).value,
        _ => Primitive::String,
    }
}

fn inexpressible(schema: &Schema, mrid: &Mrid, attr: AttrId) -> Diagnostic {
    let def = schema.attr(attr);
    Diagnostic::warning(
        Rule::Structure,
        format!(
            "{} changed, but IEC 61970-552 difference statements cannot carry a compound \
             value; the change is not in the change set",
            def.name
        ),
    )
    .with_class(schema.class(def.owner).name)
    .with_object(mrid.clone())
    .with_attribute(def.name)
}

/// A minimal change-set header, taking what it can from the target's own headers.
///
/// The identifier is derived rather than random: the same pair of models must produce the
/// same change file every time, or a change set stops being comparable with the last one.
/// `md:Model.Supersedes` names the models being replaced, which is what tells a consumer
/// what this change applies to.
fn derived_header(target: &Dataset) -> ModelHeader {
    let profiles: Vec<String> = target
        .headers()
        .iter()
        .flat_map(|h| h.profiles.iter().cloned())
        .collect();
    let supersedes: Vec<Mrid> = target
        .headers()
        .iter()
        .filter_map(|h| h.id.clone())
        .collect();

    // What the change set replaces is what names it. A model loaded from files has header
    // identifiers and they say it exactly; one built in memory has none, and would then
    // give every change set the same identifier — so that case falls back to what the
    // target actually contains, which costs a walk of the model but only where there is no
    // cheaper answer.
    let name: String = if supersedes.is_empty() {
        target.content_id().canonical()
    } else {
        supersedes
            .iter()
            .map(Mrid::canonical)
            .collect::<Vec<_>>()
            .join(" ")
    };

    let mut header = ModelHeader::new(ModelKind::Difference);
    header.id = Some(Mrid::new_v5(&Dataset::DERIVED_NS, name.as_bytes()));
    header.version = Some("1".to_owned());
    header.profiles = dedup(profiles);
    header.supersedes = supersedes;
    header
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    v.retain(|s| {
        let new = !seen.contains(s);
        if new {
            seen.push(s.clone());
        }
        new
    });
    v
}
