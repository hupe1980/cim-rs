//! Structural validation derived from the schema.
//!
//! These checks are exactly the ones the RDFS vocabulary can justify: cardinality,
//! datatype, class membership, reference targets, profile membership and identifier
//! conformance. Semantic rules beyond the vocabulary — the ones ENTSO-E publishes as
//! SHACL shapes — are deliberately out of scope; run those with a SHACL engine against
//! the same data.

use crate::dataset::Dataset;
use crate::error::{Diagnostic, Report, Rule, Severity};
use crate::header::ModelHeader;
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::{AttrKind, ProfileId, ProfileMask, Schema};
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
    /// Check that identifiers conform to IEC 61970-552.
    pub mrid_conformance: bool,
    /// Report use of deprecated classes and attributes.
    pub deprecation: bool,
    /// Check that headers declare dependencies that were actually loaded.
    pub dependencies: bool,
    /// Check each loaded file's header against the structural requirements of
    /// IEC 61970-552.
    pub headers: bool,
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
            references: true,
            reference_targets: true,
            mrid_conformance: true,
            deprecation: false,
            dependencies: true,
            headers: true,
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
            references: true,
            reference_targets: true,
            mrid_conformance: false,
            deprecation: false,
            dependencies: false,
            headers: false,
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

    if options.mrid_conformance && !mrid.is_uuid() {
        report.push(
            Diagnostic::warning(
                Rule::NonConformingMrid,
                format!(
                    "identifier {:?} is not a UUID, which IEC 61970-552 requires",
                    mrid.canonical()
                ),
            )
            .with_class(class.name)
            .with_object(mrid.canonical()),
        );
    }

    if !class.concrete {
        report.push(
            Diagnostic::error(
                Rule::AbstractInstantiated,
                format!("abstract class {} has an instance", class.name),
            )
            .with_class(class.name)
            .with_object(mrid.canonical()),
        );
    }

    if options.deprecation && class.deprecated {
        report.push(
            Diagnostic::info(
                Rule::Deprecated,
                format!("class {} is deprecated", class.name),
            )
            .with_class(class.name)
            .with_object(mrid.canonical()),
        );
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
                // Only report a missing mandatory attribute if the object actually
                // participates in a profile that declares it; otherwise the data simply
                // has not been loaded yet.
                let object_in_profile = obj.profiles() == 0 || obj.profiles() & def.profiles != 0;
                if object_in_profile {
                    report.push(
                        Diagnostic::error(
                            Rule::MissingRequired,
                            format!("required attribute {} is missing", def.name),
                        )
                        .with_class(class.name)
                        .with_object(mrid.canonical())
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
                .with_object(mrid.canonical())
                .with_attribute(def.name),
            );
        }

        // Report only data that would genuinely be lost: a value no profile in scope
        // would write. A value carries the profile of the file it came from, so
        // ShortCircuit attributes inside an Equipment file are correctly not flagged.
        if scope != ProfileMask::MAX {
            let orphaned = obj.get_all(*attr_id).iter().all(|slot| {
                let effective = if slot.profiles != 0 {
                    slot.profiles
                } else {
                    def.used_in
                };
                effective & scope == 0
            });
            if orphaned {
                report.push(
                    Diagnostic::warning(
                        Rule::AttributeNotInProfile,
                        format!(
                            "{} is set but no loaded profile would write it, so it would \
                             be lost on export",
                            def.name
                        ),
                    )
                    .with_class(class.name)
                    .with_object(mrid.canonical())
                    .with_attribute(def.name),
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
                .with_object(mrid.canonical())
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
                                .with_object(mrid.canonical())
                                .with_attribute(def.name),
                            );
                        }
                    }
                    Some(t) => {
                        if options.reference_targets && !schema.is_a(t.class(), target) {
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
                                .with_object(mrid.canonical())
                                .with_attribute(def.name),
                            );
                        }
                    }
                }
            }
        }
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
        && !id.is_uuid()
    {
        report.push(
            Diagnostic::warning(
                Rule::MalformedHeader,
                format!("header identifier {:?} is not a UUID", id.canonical()),
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
pub fn profile_coverage(dataset: &Dataset) -> Vec<ProfileCoverage> {
    let schema = dataset.schema();
    let mut out = Vec::new();
    for (i, p) in schema.profiles.iter().enumerate() {
        let profile = ProfileId(i as u16);
        let mask = profile.mask();
        let mut objects = 0;
        let mut attributes = 0;
        let mut missing_required = 0;

        for (_, obj) in dataset.iter() {
            let class = schema.class(obj.class());
            if class.profiles & mask == 0 {
                continue;
            }
            let mut any = false;
            for attr_id in class.all_attrs {
                let def = schema.attr(*attr_id);
                if !def.is_serialized_in(profile) {
                    continue;
                }
                let n = obj.count(*attr_id);
                if n > 0 {
                    any = true;
                    attributes += n;
                } else if def.mult.is_required() {
                    missing_required += 1;
                }
            }
            if any {
                objects += 1;
            }
        }
        out.push(ProfileCoverage {
            profile,
            keyword: p.keyword,
            objects,
            attributes,
            missing_required,
        });
    }
    out
}

/// Convenience: does this report block a conforming export?
pub fn is_conforming(report: &Report) -> bool {
    report.count(Severity::Error) == 0
}
