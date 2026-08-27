//! Structural validation derived from the schema.
//!
//! These checks are exactly the ones the RDFS vocabulary can justify: cardinality,
//! datatype, class membership, reference targets, profile membership and identifier
//! conformance. Semantic rules beyond the vocabulary — the ones ENTSO-E publishes as
//! SHACL shapes — are deliberately out of scope; run those with a SHACL engine against
//! the same data.

use std::collections::BTreeMap;

use crate::dataset::Dataset;
use crate::error::{Diagnostic, Report, Rule, Severity};
use crate::header::ModelHeader;
use crate::mrid::{Mrid, MridForm};
use crate::object::Object;
use crate::schema::{AttrId, AttrKind, ProfileId, ProfileMask, Schema};
use crate::value::Value;

/// Which checks to run.
#[derive(Clone, Debug)]
pub struct ValidateOptions {
    /// Check that required attributes are present.
    pub required_attributes: bool,
    /// Check that multiplicities are respected.
    pub cardinality: bool,
    /// Check that references resolve within the dataset.
    pub references: bool,
    /// Check that a reference points at an object of the declared target class.
    pub reference_targets: bool,
    /// Check that every stored value's attribute belongs to the object's class.
    ///
    /// The one check about what the *store* holds rather than about what a document said.
    /// Everything else walks the class's attribute list, so a value filed under an
    /// attribute the class does not have is examined by nothing and dropped on export.
    pub class_membership: bool,
    /// Check that each stored value has the shape its attribute declares.
    ///
    /// CIM/XML carries no datatype information, so this is only decidable against the
    /// RDFS — which is exactly why ENTSO-E's own guidance tells importers to enrich the
    /// data graph from the profile before validating it.
    pub datatypes: bool,
    /// Check attributes the schema pins with `cims:isFixed`.
    pub fixed_values: bool,
    /// Check that identifiers conform to IEC 61970-552.
    pub mrid_conformance: bool,
    /// Report use of deprecated classes and attributes.
    pub deprecation: bool,
    /// Check that headers declare dependencies that were actually loaded.
    pub dependencies: bool,
    /// Report attributes whose data belongs to a profile none of the loaded files
    /// declares — a header that under-declares what its file contains.
    pub profile_reach: bool,
    /// Check each loaded file's header against the structural requirements of
    /// IEC 61970-552.
    pub headers: bool,
    /// Report single-valued attributes two loaded files gave different values for.
    ///
    /// Recorded by [`Dataset`] as the files are merged, because by the time validation
    /// runs one of the two values is gone.
    pub merge_conflicts: bool,
    /// Check that the model can actually be serialized as CIM/XML.
    ///
    /// Two constraints of the output syntax that the object model cannot express, so
    /// nothing else enforces them: a text value must hold only characters XML 1.0 can
    /// represent, and an identifier must have an `rdf:ID` or `rdf:about` form. Both are
    /// satisfied by every conforming model — every IEC 61970-552 identifier is a UUID —
    /// and both are reachable from a mis-encoded file or from
    /// [`Object::set`](crate::Object::set). Neither is visible to a well-formedness check
    /// of the output: the first because `quick-xml` does not enforce the `Char`
    /// production, the second because the document is well-formed XML and merely invalid
    /// RDF/XML.
    pub serializable: bool,
    /// Restrict checks to these profiles. Empty means "the profiles the headers declare",
    /// falling back to every profile the schema knows.
    pub profiles: ProfileMask,
    /// Stop after this many diagnostics. Prevents a broken file producing millions.
    pub max_diagnostics: usize,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        ValidateOptions {
            required_attributes: true,
            cardinality: true,
            class_membership: true,
            references: true,
            reference_targets: true,
            datatypes: true,
            fixed_values: true,
            mrid_conformance: true,
            deprecation: false,
            dependencies: true,
            profile_reach: true,
            headers: true,
            merge_conflicts: true,
            serializable: true,
            profiles: 0,
            max_diagnostics: 10_000,
        }
    }
}

impl ValidateOptions {
    /// Only the checks that indicate genuinely broken data.
    pub fn essential() -> ValidateOptions {
        ValidateOptions {
            required_attributes: false,
            cardinality: true,
            // A value nothing will ever look at is broken data by any definition.
            class_membership: true,
            references: true,
            reference_targets: true,
            datatypes: true,
            fixed_values: false,
            mrid_conformance: false,
            deprecation: false,
            dependencies: false,
            profile_reach: false,
            headers: false,
            // A model set whose files contradict each other is broken data by any
            // definition, so this stays on even in the reduced set.
            merge_conflicts: true,
            // So is a model that cannot be written back out.
            serializable: true,
            ..Default::default()
        }
    }

    /// Every check, including advisory ones.
    pub fn thorough() -> ValidateOptions {
        ValidateOptions {
            deprecation: true,
            ..Default::default()
        }
    }

    pub fn for_profiles(mut self, profiles: ProfileMask) -> Self {
        self.profiles = profiles;
        self
    }
}

/// Validate a dataset with default options.
pub fn validate(dataset: &Dataset) -> Report {
    validate_with(dataset, &ValidateOptions::default())
}

/// Validate a dataset.
pub fn validate_with(dataset: &Dataset, options: &ValidateOptions) -> Report {
    let schema = dataset.schema();
    let mut report = Report::default();

    // Which profiles to judge required-ness against. An attribute that is mandatory in
    // Equipment must not be reported as missing when only a Topology file is loaded.
    let scope = if options.profiles != 0 {
        options.profiles
    } else {
        let declared = dataset.profiles();
        if declared != 0 {
            declared
        } else {
            ProfileMask::MAX
        }
    };

    if options.headers {
        for h in dataset.headers() {
            report.extend(validate_header(h));
        }
    }
    if options.dependencies {
        check_dependencies(dataset, &mut report);
    }
    if options.profile_reach {
        check_profile_reach(dataset, &mut report);
    }
    if options.merge_conflicts {
        check_merge_conflicts(dataset, &mut report);
    }

    for (_, obj) in dataset.iter() {
        if report.len() >= options.max_diagnostics {
            report.push(Diagnostic::warning(
                Rule::Structure,
                format!(
                    "validation stopped after {} diagnostics",
                    options.max_diagnostics
                ),
            ));
            break;
        }
        check_object(schema, dataset, obj, scope, options, &mut report);
    }

    report
}

fn check_object(
    schema: &'static Schema,
    dataset: &Dataset,
    obj: &Object,
    scope: ProfileMask,
    options: &ValidateOptions,
    report: &mut Report,
) {
    let class = schema.class(obj.class());
    let mrid = obj.mrid();

    if options.mrid_conformance && !mrid.is_conforming() {
        report.push(
            Diagnostic::warning(Rule::NonConformingMrid, mrid_complaint(mrid))
                .with_class(class.name)
                .with_object(mrid.clone()),
        );
    }

    if options.serializable {
        check_serializable(schema, obj, class.name, mrid, report);
    }

    if !class.concrete {
        report.push(
            Diagnostic::error(
                Rule::AbstractInstantiated,
                format!("abstract class {} has an instance", class.name),
            )
            .with_class(class.name)
            .with_object(mrid.clone()),
        );
    }

    if options.deprecation && class.deprecated {
        report.push(
            Diagnostic::info(
                Rule::Deprecated,
                format!("class {} is deprecated", class.name),
            )
            .with_class(class.name)
            .with_object(mrid.clone()),
        );
    }

    // A value stored under an attribute this class does not have is otherwise *invisible*:
    // every other check below walks `class.all_attrs`, and so does the writer, so such a
    // value is never examined and is silently dropped on the next export. The reader
    // refuses to store one and so does `apply_difference`, but `Object::set` is public and
    // a class can be reclassified out from under its values, so the finished model is
    // checked rather than only the paths into it.
    if options.class_membership {
        for slot in obj.slots() {
            let def = schema.attr(slot.attr);
            if !schema.is_a(obj.class(), def.owner) {
                report.push(
                    Diagnostic::error(
                        Rule::UnknownAttribute,
                        format!(
                            "{} is declared on {} and cannot be stored on a {}; it would be \
                             dropped on export",
                            def.name,
                            schema.class(def.owner).name,
                            class.name
                        ),
                    )
                    .with_class(class.name)
                    .with_object(mrid.clone())
                    .with_attribute(def.name),
                );
            }
        }
    }

    for attr_id in class.all_attrs {
        let def = schema.attr(*attr_id);
        let in_scope = def.profiles & scope != 0;
        let count = obj.count(*attr_id) as u32;

        if count == 0 {
            // An association side marked `AssociationUsed = No` is never written; it is
            // derived by inverting the other side, so its absence is not a defect.
            let derived = matches!(def.kind, AttrKind::Association { .. }) && def.used_in == 0;
            if options.required_attributes && in_scope && def.mult.is_required() && !derived {
                // Two conditions, both about having enough of the model to judge.
                //
                // The object must participate in a profile that declares the attribute —
                // otherwise the data simply has not been loaded yet. And some loaded file
                // must *define* the object's class: a Topology file that refers to a
                // `ConnectivityNode` the Equipment file introduces carries none of its
                // mandatory attributes, and is not supposed to.
                let object_in_profile = obj.profiles() == 0 || obj.profiles() & def.profiles != 0;
                //
                // A class that no profile both introduces and declares concrete is never
                // the subject of a definition at all — `Equipment` is abstract in the
                // Equipment profile and concrete only in Steady State Hypothesis, where it
                // is the generic carrier for `inService` — so there is no file whose
                // absence or presence would settle the question.
                let defining = class.defined_in & class.concrete_in;
                let definition_loaded = defining & scope != 0;
                if object_in_profile && definition_loaded {
                    report.push(
                        Diagnostic::error(
                            Rule::MissingRequired,
                            format!("required attribute {} is missing", def.name),
                        )
                        .with_class(class.name)
                        .with_object(mrid.clone())
                        .with_attribute(def.name),
                    );
                }
            }
            continue;
        }

        if options.cardinality && count > def.mult.max() {
            report.push(
                Diagnostic::error(
                    Rule::CardinalityExceeded,
                    format!(
                        "{} occurs {} times but permits at most {}",
                        def.name,
                        count,
                        def.mult.max()
                    ),
                )
                .with_class(class.name)
                .with_object(mrid.clone())
                .with_attribute(def.name),
            );
        }
        // A bounded lower limit above one — `M:2..2` and friends — is not covered by the
        // "required attribute is missing" check, which only sees absence.
        if options.cardinality && in_scope && count < def.mult.min() && count > 0 {
            report.push(
                Diagnostic::error(
                    Rule::MissingRequired,
                    format!(
                        "{} occurs {} times but requires at least {}",
                        def.name,
                        count,
                        def.mult.min()
                    ),
                )
                .with_class(class.name)
                .with_object(mrid.clone())
                .with_attribute(def.name),
            );
        }

        if options.datatypes || options.fixed_values {
            for slot in obj.get_all(*attr_id) {
                check_value(
                    schema,
                    &slot.value,
                    *attr_id,
                    class.name,
                    mrid,
                    options,
                    report,
                );
            }
        }

        if options.deprecation && def.deprecated {
            report.push(
                Diagnostic::info(
                    Rule::Deprecated,
                    format!("attribute {} is deprecated", def.name),
                )
                .with_class(class.name)
                .with_object(mrid.clone())
                .with_attribute(def.name),
            );
        }

        // Reference checks.
        if let AttrKind::Association { target, .. } = def.kind {
            for slot in obj.get_all(*attr_id) {
                let Value::Reference(m) = &slot.value else {
                    continue;
                };
                match dataset.by_mrid(m) {
                    None => {
                        if options.references {
                            report.push(
                                Diagnostic::warning(
                                    Rule::DanglingReference,
                                    format!(
                                        "{} points at {} which is not in the dataset",
                                        def.name,
                                        m.canonical()
                                    ),
                                )
                                .with_class(class.name)
                                .with_object(mrid.clone())
                                .with_attribute(def.name),
                            );
                        }
                    }
                    Some(t) => {
                        // An object may be known only through a base class: a Steady State
                        // Hypothesis file writes `<cim:Equipment rdf:about="…">` for a
                        // breaker whose own class only the Equipment file declares. Until
                        // that file is loaded the object *is* an `Equipment`, and that is
                        // not evidence it is the wrong kind of thing — only two classes on
                        // different branches are.
                        let compatible =
                            schema.is_a(t.class(), target) || schema.is_a(target, t.class());
                        if options.reference_targets && !compatible {
                            report.push(
                                Diagnostic::error(
                                    Rule::WrongReferenceTarget,
                                    format!(
                                        "{} must point at {} but {} is a {}",
                                        def.name,
                                        schema.class(target).name,
                                        m.canonical(),
                                        schema.class(t.class()).name
                                    ),
                                )
                                .with_class(class.name)
                                .with_object(mrid.clone())
                                .with_attribute(def.name),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Check that an object can be written back out as CIM/XML.
///
/// The two constraints of the output syntax that the object model cannot express. Both
/// hold for every conforming model, and both are reachable — the first from a mis-encoded
/// source file, the second from an identifier a producer chose freely, which this crate
/// keeps verbatim on purpose.
///
/// This is the same shape of check as class membership: the writer already defends itself,
/// stripping what XML cannot hold and choosing the identifier form the syntax permits, but
/// a defence at the exit tells the caller nothing about *why* their data changed. Here it
/// is named, with the object it belongs to, before an export quietly differs from what was
/// put in.
fn check_serializable(
    schema: &'static Schema,
    obj: &Object,
    class: &'static str,
    mrid: &Mrid,
    report: &mut Report,
) {
    use crate::xml::{IdentifierForm, describe_char, find_illegal};

    if mrid.form_in_xml() == IdentifierForm::Unwritable {
        report.push(
            Diagnostic::error(
                Rule::UnserializableIdentifier,
                format!(
                    "identifier {:?} is neither an XML NCName nor an absolute IRI, so it has \
                     no valid rdf:ID or rdf:about form; the document written will be \
                     well-formed XML but not valid RDF/XML",
                    mrid.as_written()
                ),
            )
            .with_class(class)
            .with_object(mrid.clone()),
        );
    }

    // Only text carries characters; enumerations, references and compounds serialize
    // through forms of their own — except that a compound's fields are text again.
    fn scan(
        schema: &'static Schema,
        value: &Value,
        attr: AttrId,
        class: &'static str,
        mrid: &Mrid,
        report: &mut Report,
    ) {
        match value {
            Value::Text(s) => {
                if let Some((offset, c)) = find_illegal(s) {
                    let def = schema.attr(attr);
                    report.push(
                        Diagnostic::error(
                            Rule::IllegalXmlCharacter,
                            format!(
                                "{} holds {}, which XML 1.0 cannot represent in any form; it \
                                 will be dropped on export (first at byte {offset} of the value)",
                                def.name,
                                describe_char(c)
                            ),
                        )
                        .with_class(class)
                        .with_object(mrid.clone())
                        .with_attribute(def.name),
                    );
                }
            }
            Value::Compound(c) => {
                for (a, v) in c.values() {
                    scan(schema, v, *a, class, mrid, report);
                }
            }
            _ => {}
        }
    }

    for slot in obj.slots() {
        scan(schema, &slot.value, slot.attr, class, mrid, report);
    }
}

/// Check one stored value against what its attribute declares, recursing into compounds.
///
/// This is the check CIM/XML cannot make for itself. IEC 61970-552 exchanges no datatype
/// information, so a parser without the profile reads every value as a string; only the
/// RDFS says that `ACLineSegment.r` is a float or that `Terminal.phases` must be a
/// `PhaseCode`, and only here can a value that is neither be caught.
fn check_value(
    schema: &'static Schema,
    value: &Value,
    attr: AttrId,
    class: &'static str,
    mrid: &Mrid,
    options: &ValidateOptions,
    report: &mut Report,
) {
    let def = schema.attr(attr);
    let mut mismatch = |expected: &str, found: &str| {
        report.push(
            Diagnostic::error(
                Rule::DatatypeMismatch,
                format!("{} expects {expected} but holds {found}", def.name),
            )
            .with_class(class)
            .with_object(mrid.clone())
            .with_attribute(def.name),
        );
    };

    if options.datatypes {
        match def.kind {
            AttrKind::Primitive(p) => {
                if let Some(found) = primitive_mismatch(p, value) {
                    mismatch(&format!("{p:?}"), found);
                }
            }
            AttrKind::Datatype(dt) => {
                let p = schema.datatype(dt).value;
                if let Some(found) = primitive_mismatch(p, value) {
                    mismatch(&format!("{} ({p:?})", schema.datatype(dt).name), found);
                }
            }
            AttrKind::Enumeration(want) => match value {
                // A literal recovered from the wrong namespace can belong to a different
                // enumeration; the qualified name alone cannot rule that out.
                Value::Enum(v) if schema.enum_value(*v).owner != want => mismatch(
                    schema.enumeration(want).name,
                    schema.enumeration(schema.enum_value(*v).owner).name,
                ),
                Value::Enum(_) => {}
                other => mismatch(schema.enumeration(want).name, shape_of(other)),
            },
            AttrKind::Association { .. } => {
                if !matches!(value, Value::Reference(_)) {
                    mismatch("a reference", shape_of(value));
                }
            }
            AttrKind::Compound(want) => match value {
                Value::Compound(c) if c.class() != want => {
                    mismatch(schema.class(want).name, schema.class(c.class()).name)
                }
                Value::Compound(c) => {
                    // Compounds nest, and their fields are ordinary attributes.
                    for (a, v) in c.values() {
                        check_value(schema, v, *a, class, mrid, options, report);
                    }
                }
                other => mismatch(schema.class(want).name, shape_of(other)),
            },
        }
    }

    if options.fixed_values
        && let Some(fixed) = def.fixed
    {
        let actual = crate::value::display_value(schema, value);
        // A fixed value is compared as text: the schema pins the lexical form.
        if actual != fixed && !actual.is_empty() {
            report.push(
                Diagnostic::warning(
                    Rule::FixedValueMismatch,
                    format!("{} is fixed to {fixed:?} but holds {actual:?}", def.name),
                )
                .with_class(class)
                .with_object(mrid.clone())
                .with_attribute(def.name),
            );
        }
    }
}

/// Whether `value` can serve as `p`, and what it is if not.
///
/// The compatibility rule itself lives on [`Value::fits`], because the RDF writer has to
/// answer the same question — whether to label a literal with the type the profile
/// declares — and the two must not drift apart.
fn primitive_mismatch(p: crate::schema::Primitive, value: &Value) -> Option<&'static str> {
    (!value.fits(p)).then(|| value.shape())
}

fn shape_of(v: &Value) -> &'static str {
    v.shape()
}

/// Report values whose own file declared no profile that declares the attribute.
///
/// This is a header/content mismatch: a CGMES 3.0 Equipment file that carries
/// ShortCircuit attributes should declare the ShortCircuit profile in its header. The
/// comparison is per value against *its own* file's declaration, not against the union of
/// everything loaded, so the finding survives having the missing profile loaded from
/// somewhere else.
///
/// Findings are aggregated **per attribute**, not per value — one such mismatch can
/// affect hundreds of thousands of values, and repeating the same finding for each is
/// noise rather than information.
fn check_profile_reach(dataset: &Dataset, report: &mut Report) {
    let schema = dataset.schema();
    // Attribute -> (number of values affected, an example object).
    let mut undeclared: BTreeMap<AttrId, (usize, Mrid)> = BTreeMap::new();

    for (_, obj) in dataset.iter() {
        for slot in obj.slots() {
            if !schema.declaration_covers(slot.attr, slot.profiles) {
                let entry = undeclared
                    .entry(slot.attr)
                    .or_insert_with(|| (0, obj.mrid().clone()));
                entry.0 += 1;
            }
        }
    }

    for (attr, (count, example)) in undeclared {
        let def = schema.attr(attr);
        let profiles: Vec<&str> = schema
            .profiles
            .iter()
            .enumerate()
            // `ProfileId::mask` rather than a bare shift, which Rust masks in release
            // builds: an over-wide shift would name the wrong profile instead of failing.
            .filter(|(i, _)| def.used_in & ProfileId(*i as u16).mask() != 0)
            .map(|(_, p)| p.keyword)
            .collect();
        report.push(
            Diagnostic::warning(
                Rule::AttributeNotInProfile,
                format!(
                    "{} is set on {count} object(s) by files whose header does not declare \
                     profile(s) {}, which is where the schema puts it",
                    def.name,
                    profiles.join("+")
                ),
            )
            .with_class(schema.class(def.owner).name)
            .with_object(example)
            .with_attribute(def.name),
        );
    }
}

/// Report the disagreements that assembling the model set had to resolve.
///
/// This is the one check that cannot be made from the finished dataset: merging keeps one
/// of two contradictory values, so by the time validation runs the other is gone.
/// [`Dataset`] therefore records the conflict as it happens and this reads it back.
///
/// A warning rather than an error: the model is usable and every published model set in
/// the conformity corpus is free of these, but a change file deliberately paired with the
/// *old* base model produces them by design, and that is a legitimate intermediate state.
fn check_merge_conflicts(dataset: &Dataset, report: &mut Report) {
    let schema = dataset.schema();
    for c in dataset.merge_conflicts() {
        let def = schema.attr(c.attr);
        report.push(
            Diagnostic::warning(
                Rule::ConflictingValue,
                format!(
                    "{} was given {} by one file and {} by another; the later value is kept",
                    def.name,
                    crate::value::display_value(schema, &c.discarded),
                    crate::value::display_value(schema, &c.kept),
                ),
            )
            .with_class(schema.class(def.owner).name)
            .with_object(c.object.clone())
            .with_attribute(def.name),
        );
    }
    let dropped = dataset.merge_conflicts_dropped();
    if dropped > 0 {
        report.push(Diagnostic::warning(
            Rule::ConflictingValue,
            format!("{dropped} further conflicting value(s) were counted but not recorded"),
        ));
    }
}

/// Check that every `md:Model.DependentOn` was satisfied by a loaded file.
fn check_dependencies(dataset: &Dataset, report: &mut Report) {
    let loaded: Vec<&Mrid> = dataset
        .headers()
        .iter()
        .filter_map(|h| h.id.as_ref())
        .collect();
    for h in dataset.headers() {
        for dep in &h.dependent_on {
            if !loaded.contains(&dep) {
                report.push(
                    Diagnostic::warning(
                        Rule::UnsatisfiedDependency,
                        format!("model depends on {} which was not loaded", dep.canonical()),
                    )
                    .with_source(h.source.clone().unwrap_or_else(|| "<header>".into())),
                );
            }
        }
    }
}

/// Say precisely what is wrong with an identifier IEC 61970-552 would reject.
///
/// The two cases call for different action and must not read alike: a compact UUID is the
/// right value in the wrong spelling — every tool can still resolve it, and re-hyphenating
/// it fixes the file — while an opaque identifier has no UUID behind it at all, so it has
/// no `urn:uuid:` IRI and no amount of reformatting will give it one.
fn mrid_complaint(mrid: &Mrid) -> String {
    match mrid.form() {
        MridForm::Uuid => format!("identifier {:?} conforms", mrid.canonical()),
        MridForm::Compact => format!(
            "identifier {:?} is a UUID written without hyphens; IEC 61970-552 requires \
             the {:?} form",
            mrid.as_written(),
            mrid.canonical()
        ),
        MridForm::Opaque => format!(
            "identifier {:?} is not a UUID, which IEC 61970-552 requires",
            mrid.canonical()
        ),
    }
}

/// Check a set of headers for the structural requirements of IEC 61970-552.
pub fn validate_header(header: &ModelHeader) -> Report {
    let mut report = Report::default();
    let src = header.source.clone().unwrap_or_else(|| "<header>".into());

    if header.id.is_none() {
        report.push(
            Diagnostic::error(Rule::MalformedHeader, "header has no rdf:about identifier")
                .with_source(src.clone()),
        );
    } else if let Some(id) = &header.id
        && !id.is_conforming()
    {
        report.push(
            Diagnostic::warning(
                Rule::MalformedHeader,
                format!("header identifier: {}", mrid_complaint(id)),
            )
            .with_source(src.clone()),
        );
    }
    if header.profiles.is_empty() {
        report.push(
            Diagnostic::error(Rule::MalformedHeader, "header declares no md:Model.profile")
                .with_source(src.clone()),
        );
    }
    if header.scenario_time.is_none() {
        report.push(
            Diagnostic::warning(Rule::MalformedHeader, "header has no md:Model.scenarioTime")
                .with_source(src.clone()),
        );
    }
    if header.created.is_none() {
        report.push(
            Diagnostic::warning(Rule::MalformedHeader, "header has no md:Model.created")
                .with_source(src),
        );
    }
    report
}

/// Summarize a dataset against one profile: what it would export, and what is missing.
#[derive(Clone, Debug)]
pub struct ProfileCoverage {
    pub profile: ProfileId,
    pub keyword: &'static str,
    /// Objects that would appear in an export of this profile.
    pub objects: usize,
    /// Attribute occurrences that would be written.
    pub attributes: usize,
    /// Required attributes of in-profile objects that are absent.
    pub missing_required: usize,
}

/// Report, per profile, how much of it the dataset actually carries.
///
/// One pass over the model rather than one per profile: on a nation-scale model the
/// difference is eleven walks of a quarter of a million objects versus one.
pub fn profile_coverage(dataset: &Dataset) -> Vec<ProfileCoverage> {
    let schema = dataset.schema();
    let n = schema.profiles.len();
    let mut objects = vec![0usize; n];
    let mut attributes = vec![0usize; n];
    let mut missing_required = vec![0usize; n];

    for (_, obj) in dataset.iter() {
        let class = schema.class(obj.class());
        // Profiles this object contributes at least one value to.
        let mut touched: ProfileMask = 0;
        for attr_id in class.all_attrs {
            let def = schema.attr(*attr_id);
            let slots = obj.get_all(*attr_id);
            if slots.is_empty() {
                if def.mult.is_required() {
                    for (i, missing) in missing_required.iter_mut().enumerate() {
                        let mask = ProfileId(i as u16).mask();
                        if class.profiles & mask != 0 && def.used_in & mask != 0 {
                            *missing += 1;
                        }
                    }
                }
                continue;
            }
            // Count exactly what the writer would emit, so the report cannot drift from
            // the export: both go through `effective_profiles`.
            for s in slots {
                let effective = schema.effective_profiles(s.attr, s.profiles);
                for (i, count) in attributes.iter_mut().enumerate() {
                    let mask = ProfileId(i as u16).mask();
                    if effective & mask != 0 && class.profiles & mask != 0 {
                        *count += 1;
                        touched |= mask;
                    }
                }
            }
        }
        for (i, count) in objects.iter_mut().enumerate() {
            if touched & ProfileId(i as u16).mask() != 0 {
                *count += 1;
            }
        }
    }

    schema
        .profiles
        .iter()
        .enumerate()
        .map(|(i, p)| ProfileCoverage {
            profile: ProfileId(i as u16),
            keyword: p.keyword,
            objects: objects[i],
            attributes: attributes[i],
            missing_required: missing_required[i],
        })
        .collect()
}

/// Convenience: does this report block a conforming export?
pub fn is_conforming(report: &Report) -> bool {
    report.count(Severity::Error) == 0
}
