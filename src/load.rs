//! Loading and saving whole models.
//!
//! A CGMES model is a set of profile files, so the convenient unit of work is "load
//! these files into one dataset" rather than "parse this document". This module provides
//! that, plus difference-model application and per-profile export.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::dataset::Dataset;
use crate::error::{Diagnostic, Report, Result, Rule};
use crate::header::{DifferenceModel, ModelHeader, ModelKind, Statement, StatementValue};
use crate::mrid::Mrid;
use crate::object::Object;
use crate::reader::{ReadOptions, read_header, read_into};
use crate::schema::{AttrKind, Primitive, ProfileId, Schema};
use crate::value::Value;
use crate::writer::{WriteOptions, write_profile};

/// Outcome of loading one or more files.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub report: Report,
    /// Files loaded, in order, with the header each carried.
    pub files: Vec<(PathBuf, Option<ModelHeader>)>,
    pub objects_read: usize,
}

impl LoadReport {
    pub fn has_errors(&self) -> bool {
        self.report.has_errors()
    }
}

impl Dataset {
    /// Load a set of instance files into a new dataset.
    ///
    /// Files may be given in any order: objects are merged by mRID, so an SSH file read
    /// before its EQ file produces the same result.
    pub fn load<I, P>(schema: &'static Schema, paths: I) -> Result<Dataset>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut ds = Dataset::new(schema);
        ds.load_files(paths, &ReadOptions::lenient())?;
        Ok(ds)
    }

    /// Load files into an existing dataset.
    pub fn load_files<I, P>(&mut self, paths: I, options: &ReadOptions) -> Result<LoadReport>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut out = LoadReport::default();
        for path in paths {
            let path = path.as_ref();
            let one = self.load_file(path, options)?;
            out.report.extend(one.report);
            out.files.extend(one.files);
            out.objects_read += one.objects_read;
        }
        self.shrink_to_fit();
        Ok(out)
    }

    /// Load one instance file, or every instance file inside a zip archive.
    pub fn load_file(&mut self, path: &Path, options: &ReadOptions) -> Result<LoadReport> {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            #[cfg(feature = "zip")]
            {
                return self.load_zip(path, options);
            }
            #[cfg(not(feature = "zip"))]
            {
                return Err(crate::error::Error::NotCimXml(format!(
                    "{} is a zip archive; enable the `zip` feature to read it",
                    path.display()
                )));
            }
        }

        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let file = File::open(path)?;
        let outcome = read_into(
            self,
            BufReader::with_capacity(1 << 16, file),
            name.as_deref(),
            options,
        )?;

        let mut report = LoadReport {
            report: outcome.report,
            objects_read: outcome.objects_read,
            files: vec![(path.to_path_buf(), outcome.header.clone())],
        };
        // The reader registers the header itself, so that each object records the file it
        // came from while it is being read.
        if outcome.header.is_none() {
            report.report.push(
                Diagnostic::warning(Rule::MalformedHeader, "file has no md:FullModel header")
                    .with_source(name.unwrap_or_default()),
            );
        }
        Ok(report)
    }

    /// Load every instance file under a directory, in sorted order.
    ///
    /// A CGMES model set is a directory, not a list — that is how the conformity models,
    /// the QoCDC models and every real exchange are shipped — so this is the call most
    /// programs actually want. Naming the files individually is [`Dataset::load`].
    pub fn load_dir(schema: &'static Schema, dir: impl AsRef<Path>) -> Result<Dataset> {
        let mut ds = Dataset::new(schema);
        ds.load_files(instance_files(dir.as_ref()), &ReadOptions::lenient())?;
        Ok(ds)
    }

    /// Load a model set, reading its vintage out of the files themselves.
    ///
    /// The call most programs want. `path` is a directory, a zip archive or a single
    /// instance file; the schema comes from the first document's namespace declarations, so
    /// the caller does not have to know — or hard-code — whether a model is CGMES 3.0 or
    /// 2.4.15.
    ///
    /// Use [`Dataset::load_dir`] or [`Dataset::load`] to state the vintage explicitly, which
    /// is what a program that only ever handles one of them should do.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownVintage`](crate::Error::UnknownVintage) when no file under `path`
    /// declares a vocabulary this build
    /// has a feature for — including when `path` holds no CIM input at all.
    ///
    // An example has to name a vintage only when it names a schema; this one does not, so
    // it compiles under any feature set.
    /// ```no_run
    /// # fn main() -> cim_rs::Result<()> {
    /// let grid = cim_rs::Dataset::open("MicroGrid-BE")?;
    /// println!("{} objects", grid.len());
    /// # Ok(()) }
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Dataset> {
        let path = path.as_ref();
        let files = instance_files(path);
        let schema = files.iter().find_map(|f| detect_file(f)).ok_or_else(|| {
            crate::error::Error::UnknownVintage {
                path: path.display().to_string(),
                known: crate::VINTAGES.iter().map(|s| s.vintage).collect(),
            }
        })?;
        let mut ds = Dataset::new(schema);
        ds.load_files(&files, &ReadOptions::lenient())?;
        Ok(ds)
    }

    /// Read only the headers of a set of files, without loading their objects.
    ///
    /// Useful for deciding what to load: headers declare profiles and dependencies.
    pub fn peek_headers<I, P>(
        schema: &'static Schema,
        paths: I,
    ) -> Result<Vec<(PathBuf, Option<ModelHeader>)>>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut out = Vec::new();
        for path in paths {
            let path = path.as_ref();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let file = File::open(path)?;
            let header = read_header(schema, BufReader::new(file), name.as_deref())?;
            out.push((path.to_path_buf(), header));
        }
        Ok(out)
    }

    /// Apply a difference model (IEC 61970-552): remove reverse statements, then assert
    /// forward statements.
    ///
    /// Statements naming objects or properties the schema does not know are reported and
    /// skipped rather than aborting the whole application.
    pub fn apply_difference(&mut self, diff: &DifferenceModel) -> Report {
        let mut report = Report::default();
        // Values a difference asserts belong to the profiles its own header declares,
        // exactly as if they had arrived in an instance file. Without that, an export
        // would fall back to the attribute's declaration and could file the change under
        // a profile the change set never claimed.
        let schema = self.schema();
        let profiles = diff
            .header
            .profiles
            .iter()
            .filter_map(|iri| schema.profile_by_iri(iri))
            .fold(0, |acc, p| acc | p.mask());

        // Retract, then re-type, then assert. The order matters and is the standard's own:
        // a reverse statement talks about the object as it *was*, so it has to be applied
        // while the object still has its old class, and a forward statement talks about
        // what it becomes. The published conformity change sets replace a
        // `LinearShuntCompensator` with a `NonlinearShuntCompensator` under one identifier
        // and then set attributes only the new class has — which is exactly the case that
        // re-typing last would get wrong.
        for s in &diff.reverse {
            self.apply_statement(s, false, profiles, &mut report);
        }
        self.apply_reclassifications(diff, &mut report);
        for s in &diff.forward {
            self.apply_statement(s, true, profiles, &mut report);
        }
        report
    }

    /// Apply class changes named by a difference's forward statements.
    ///
    /// IEC 61970-552 treats an object's type as a statement like any other, so a
    /// difference may replace an object's class. The published CGMES conformity tests do
    /// exactly this, turning a `LinearShuntCompensator` into a `NonlinearShuntCompensator`
    /// under the same identifier; the reverse statements retract the old class's
    /// attributes and the forward ones assert the new class's.
    fn apply_reclassifications(&mut self, diff: &DifferenceModel, report: &mut Report) {
        let schema = self.schema();
        let mut seen: Vec<Mrid> = Vec::new();

        for s in &diff.forward {
            let Some(qname) = &s.class else { continue };
            if seen.contains(&s.subject) {
                continue;
            }
            seen.push(s.subject.clone());

            let Some(target) = schema.find_class(&qname.ns, &qname.local) else {
                report.push(
                    Diagnostic::warning(
                        Rule::UnknownClass,
                        format!("difference names unknown class {}", qname.local),
                    )
                    .with_object(s.subject.clone()),
                );
                continue;
            };
            let Some(id) = self.find(&s.subject) else {
                continue;
            };
            let Some(current) = self.get(id).map(|o| o.class()) else {
                continue;
            };
            if current == target {
                continue;
            }
            // Only a genuine change is applied. Narrowing to a subclass is a refinement
            // and is accepted; anything else is a reclassification worth recording.
            if !schema.is_a(target, current) {
                report.push(
                    Diagnostic::info(
                        Rule::Structure,
                        format!(
                            "difference reclassifies {} from {} to {}",
                            s.subject.canonical(),
                            schema.class(current).name,
                            schema.class(target).name
                        ),
                    )
                    .with_object(s.subject.clone())
                    .with_class(schema.class(target).name),
                );
            }
            // A sideways reclassification leaves the old class's own attributes with
            // nowhere to live. The dataset sheds them so the model stays consistent; what
            // went is reported here, because it is data the change set's author chose to
            // discard and a consumer should be able to see that it happened.
            for slot in self.reclassify(id, target) {
                let def = schema.attr(slot.attr);
                report.push(
                    Diagnostic::warning(
                        Rule::Structure,
                        format!(
                            "{} was dropped: {} does not have it, and the difference \
                             reclassified this object",
                            def.name,
                            schema.class(target).name
                        ),
                    )
                    .with_class(schema.class(target).name)
                    .with_object(s.subject.clone())
                    .with_attribute(def.name),
                );
            }
        }
    }

    fn apply_statement(
        &mut self,
        s: &Statement,
        assert: bool,
        profiles: crate::schema::ProfileMask,
        report: &mut Report,
    ) {
        let schema = self.schema();
        let Some(attr) = schema.find_attr(&s.predicate_ns, &s.predicate) else {
            report.push(
                Diagnostic::warning(
                    Rule::UnknownAttribute,
                    format!(
                        "difference statement names unknown property {}",
                        s.predicate
                    ),
                )
                .with_object(s.subject.clone()),
            );
            return;
        };
        let def = schema.attr(attr);

        let value = match &s.value {
            StatementValue::Resource(iri) => match def.kind {
                AttrKind::Enumeration(_) => {
                    let Some((ns, local)) = iri.rsplit_once('#') else {
                        report.push(
                            Diagnostic::error(
                                Rule::InvalidValue,
                                format!("enumeration value {iri} is not an IRI"),
                            )
                            .with_attribute(def.name),
                        );
                        return;
                    };
                    match schema
                        .find_enum_value(&format!("{ns}#"), local)
                        // As in the reader: a misplaced namespace is recoverable.
                        .or_else(|| schema.find_enum_value_any_ns(local))
                    {
                        Some(v) => Value::Enum(v),
                        None => {
                            report.push(
                                Diagnostic::error(
                                    Rule::InvalidValue,
                                    format!("unknown enumeration literal {iri}"),
                                )
                                .with_attribute(def.name)
                                .with_object(s.subject.clone()),
                            );
                            return;
                        }
                    }
                }
                _ => Value::Reference(Mrid::parse(iri)),
            },
            StatementValue::Literal(text) => {
                let prim = match def.kind {
                    AttrKind::Primitive(p) => p,
                    AttrKind::Datatype(d) => schema.datatype(d).value,
                    _ => Primitive::String,
                };
                match Value::parse_primitive(prim, text) {
                    Ok(v) => v,
                    Err(e) => {
                        report.push(
                            Diagnostic::error(Rule::InvalidValue, e.to_string())
                                .with_attribute(def.name)
                                .with_object(s.subject.clone()),
                        );
                        return;
                    }
                }
            }
        };

        // Asserting into an absent object creates it, which a forward difference may
        // legitimately do when it introduces a new object.
        let id = match self.find(&s.subject) {
            Some(id) => id,
            None if assert => {
                let class = s
                    .class
                    .as_ref()
                    .and_then(|c| schema.find_class(&c.ns, &c.local))
                    .unwrap_or(def.owner);
                self.insert(Object::new(class, s.subject.clone()))
            }
            None => {
                report.push(
                    Diagnostic::warning(
                        Rule::DanglingReference,
                        format!(
                            "difference removes from {} which is not in the dataset",
                            s.subject.canonical()
                        ),
                    )
                    .with_attribute(def.name),
                );
                return;
            }
        };

        // The same guard the reader applies to a property element, for the same reason: a
        // value stored under an attribute the object's class does not have is invisible
        // afterwards — the writer emits a class's own attributes and validation checks
        // them, so neither would ever look at it — and it would be dropped without a word
        // on the next export. A difference statement is not more trustworthy than a
        // document just because it arrived as a change set.
        //
        // Assertions only. A *retraction* of such a value is exactly how the model gets
        // rid of one, and refusing it would strand the value it was sent to remove: the
        // published CGMES 2.4.15 Type 4 archive has an object its Equipment file calls a
        // `LinearShuntCompensator` and its Steady State Hypothesis file a
        // `NonlinearShuntCompensator`, and the change set that reconciles them opens by
        // retracting the linear attributes from what is, by then, a nonlinear object.
        let class = self.get(id).map(|o| o.class());
        if assert
            && let Some(class) = class
            && !schema.is_a(class, def.owner)
        {
            report.push(
                Diagnostic::warning(
                    Rule::UnknownAttribute,
                    format!(
                        "difference statement sets {} on {}, which does not have it",
                        def.name,
                        schema.class(class).name
                    ),
                )
                .with_class(schema.class(class).name)
                .with_object(s.subject.clone())
                .with_attribute(def.name),
            );
            return;
        }

        let Some(obj) = self.get_mut(id) else { return };
        if assert {
            if def.mult.is_many() {
                obj.push_in(profiles, attr, value);
            } else {
                obj.set_in(profiles, attr, value);
            }
            obj.mark_profile(profiles);
        } else if !obj.remove_value(attr, &value) {
            report.push(
                Diagnostic::info(
                    Rule::Structure,
                    format!("difference removes {} which was not present", def.name),
                )
                .with_object(s.subject.clone()),
            );
        }
    }

    /// Write one profile of this dataset to a file.
    pub fn save_profile(
        &self,
        path: &Path,
        profile: ProfileId,
        header: Option<ModelHeader>,
    ) -> Result<()> {
        let schema = self.schema();
        // `IdStyle::Auto` decides per class whether this profile defines the object or
        // only adds to it, which is what IEC 61970-552 means by rdf:ID versus rdf:about.
        let options = WriteOptions::default();
        let header = header.or_else(|| Some(default_header(schema, profile, &self.content_id())));
        let file = File::create(path)?;
        write_profile(
            self,
            profile,
            BufWriter::with_capacity(1 << 16, file),
            header,
            &options,
        )
    }

    /// Write every profile the dataset carries data for, into `dir`, one file each.
    ///
    /// Files are named `<stem>_<KEYWORD>.xml`. Use [`Dataset::save_as_loaded`] instead
    /// when the goal is to reproduce the file set the model was read from.
    pub fn save_all_profiles(&self, dir: &Path, stem: &str) -> Result<Vec<PathBuf>> {
        std::fs::create_dir_all(dir)?;
        let mut written = Vec::new();
        for cov in crate::validate::profile_coverage(self) {
            if cov.objects == 0 {
                continue;
            }
            let path = dir.join(format!("{stem}_{}.xml", cov.keyword));
            self.save_profile(&path, cov.profile, None)?;
            written.push(path);
        }
        Ok(written)
    }

    /// Write the model back out as the file set it was read from.
    ///
    /// One output file per loaded header, carrying that header, the profiles it declared
    /// and the objects that came from it. CGMES normally exchanges Equipment, Operation
    /// and ShortCircuit in a single file; splitting them apart would be legal but would
    /// not reproduce the input, so this is the faithful export.
    ///
    /// Selecting by profile alone is not enough for a merged common grid model, where two
    /// modelling authorities each contribute an Equipment file: every object would then
    /// land in both. The dataset therefore records which file contributed each object —
    /// see [`Dataset::objects_from`] — and each header writes its own. Objects built
    /// programmatically belong to no file and go with the first header whose profiles can
    /// carry them, rather than being dropped.
    ///
    /// Returns the paths written. Headers without a resolvable profile are skipped and
    /// named in the returned report.
    pub fn save_as_loaded(&self, dir: &Path) -> Result<SaveReport> {
        std::fs::create_dir_all(dir)?;
        let schema = self.schema();
        let mut out = SaveReport::default();

        // Objects built programmatically belong to no file; they go with the first file
        // whose profiles can carry them, rather than being dropped.
        let unsourced: Vec<crate::dataset::ObjectId> = self
            .iter()
            .filter(|(_, o)| !o.from_file())
            .map(|(id, _)| id)
            .collect();
        let mut unsourced_placed = false;
        let mut used_names: Vec<String> = Vec::new();

        for (i, header) in self.headers().iter().enumerate() {
            let profiles = header
                .profiles
                .iter()
                .filter_map(|iri| schema.profile_by_iri(iri))
                .fold(0, |acc, p| acc | p.mask());
            if profiles == 0 {
                out.skipped.push(
                    header
                        .source
                        .clone()
                        .unwrap_or_else(|| format!("header {i}")),
                );
                continue;
            }

            // Reuse the source file name so a directory can be regenerated in place —
            // unless another header already claimed it. Two headers can want the same
            // name: `save_as_loaded` keeps only the final path component (a zip entry
            // name is chosen by whoever built the archive), so a merged model assembled
            // from `BE/EQ.xml` and `NL/EQ.xml` would otherwise write one file, silently
            // lose an authority's equipment, and report the same path twice as written.
            let name = unique_name(
                header
                    .source
                    .as_deref()
                    .and_then(output_file_name)
                    .unwrap_or_else(|| format!("model_{i}.xml")),
                &mut used_names,
            );
            let path = dir.join(&name);
            let file = File::create(&path)?;
            let sink = BufWriter::with_capacity(1 << 16, file);
            let options = WriteOptions {
                profiles,
                header: Some(header.clone()),
                ..Default::default()
            };

            if header.kind == ModelKind::Difference {
                // A change file holds statements, not objects, so the statements have to be
                // matched to the header: by identifier, or failing that by the document both
                // were read from — a `dm:DifferenceModel` with no `rdf:about` has no
                // identifier to match on.
                let diff = self
                    .differences()
                    .iter()
                    .find(|d| d.header.id.is_some() && d.header.id == header.id)
                    .or_else(|| {
                        self.differences()
                            .iter()
                            .find(|d| d.header.source.is_some() && d.header.source == header.source)
                    });
                match diff {
                    Some(d) => crate::writer::write_difference(schema, d, sink, &options)?,
                    None => {
                        out.skipped
                            .push(format!("{name}: difference statements were not retained"));
                        continue;
                    }
                }
                out.written.push(path);
                continue;
            }

            let mut ids: Vec<_> = self
                .objects_from(i)
                .filter(|&id| {
                    self.get(id)
                        .is_some_and(|o| crate::writer::object_has_content_in(schema, o, profiles))
                })
                .collect();
            if !unsourced_placed && !unsourced.is_empty() {
                let before = ids.len();
                ids.extend(unsourced.iter().copied().filter(|&id| {
                    self.get(id)
                        .is_some_and(|o| crate::writer::object_has_content_in(schema, o, profiles))
                }));
                unsourced_placed = ids.len() > before;
            }

            crate::writer::write_objects(self, ids.into_iter(), sink, &options)?;
            out.written.push(path);
        }
        Ok(out)
    }
}

/// The schema vintage a file declares, or `None` if it cannot be read or is unrecognised.
///
/// A zip archive is opened for its first CIM/XML entry, since a model set often arrives as
/// one. Unreadable is not an error here: [`Dataset::open`] moves on to the next file.
pub fn detect_file(path: &Path) -> Option<&'static Schema> {
    #[cfg(feature = "zip")]
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        let file = File::open(path).ok()?;
        let mut zip = zip::ZipArchive::new(BufReader::new(file)).ok()?;
        let mut names: Vec<String> = (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_owned()))
            .filter(|n| n.to_ascii_lowercase().ends_with(".xml"))
            .collect();
        names.sort();
        let entry = zip.by_name(names.first()?).ok()?;
        return crate::reader::sniff(BufReader::new(entry)).ok().flatten();
    }
    let file = File::open(path).ok()?;
    crate::reader::sniff(BufReader::new(file)).ok().flatten()
}

/// Every CIM input under `path`, sorted so that a load is reproducible.
///
/// A plain file is returned as itself whatever its name; a directory is walked recursively
/// and the files that look like CIM inputs are kept — `.xml`, and `.zip` when the `zip`
/// feature is on, since without it an archive can only produce an error.
///
/// Exposed because every program that touches a model set needs it and the answer is not
/// quite obvious: models arrive as directories, the directories nest, and the order files
/// are read in has to be fixed for a load to be reproducible even though merging makes it
/// otherwise immaterial.
pub fn instance_files(path: &Path) -> Vec<PathBuf> {
    fn looks_like_input(path: &Path) -> bool {
        let Some(ext) = path.extension() else {
            return false;
        };
        ext.eq_ignore_ascii_case("xml")
            || (cfg!(feature = "zip") && ext.eq_ignore_ascii_case("zip"))
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if looks_like_input(&p) {
                out.push(p);
            }
        }
    }

    let mut out = Vec::new();
    if path.is_dir() {
        walk(path, &mut out);
        out.sort();
    } else if path.is_file() {
        out.push(path.to_path_buf());
    }
    out
}

/// Outcome of writing a model back to disk.
#[derive(Debug, Default)]
pub struct SaveReport {
    pub written: Vec<PathBuf>,
    /// Headers that declared no profile this schema recognises.
    pub skipped: Vec<String>,
}

/// The file name to write a header's model back to, confined to the output directory.
///
/// A header's `source` is whatever named the document it was read from, and that is not
/// always a plain file name: reading a zip archive records the *entry* name, which the
/// archive's author chose and which may contain directory separators or `..`. Joining that
/// onto an output directory would let an archive decide where this process writes — the
/// classic zip-slip. Only the final component is kept, and only when it is an ordinary
/// name.
fn output_file_name(source: &str) -> Option<String> {
    let candidate = Path::new(source).file_name()?.to_str()?;
    let plain = !candidate.is_empty()
        && candidate != "."
        && candidate != ".."
        && !candidate.contains(['/', '\\', '\0']);
    plain.then(|| candidate.to_owned())
}

/// Give `name` a suffix if `used` already holds it, and record the result.
///
/// Not a nicety: without it a second file wanting the same name overwrites the first, and
/// the caller is told both were written.
fn unique_name(name: String, used: &mut Vec<String>) -> String {
    let mut candidate = name.clone();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name.as_str(), String::new()),
    };
    let mut n = 1;
    while used.contains(&candidate) {
        n += 1;
        candidate = format!("{stem}_{n}{ext}");
    }
    used.push(candidate.clone());
    candidate
}

/// A minimal conforming header for a profile export.
///
/// `content` is the exporting dataset's [`Dataset::content_id`]. The model identifier is
/// derived from it and from the profile, rather than left empty: IEC 61970-552 requires
/// `md:FullModel rdf:about`, so a header without one — or with the nil UUID — is a document
/// this crate's own validator rejects. Deriving it keeps the export deterministic while
/// still giving two different models different identifiers.
///
/// `md:Model.created` and `md:Model.scenarioTime` are deliberately absent: both are facts
/// about the exchange that only the caller knows, and inventing a timestamp would make an
/// unchanged model export differently every time. [`validate`](mod@crate::validate)
/// reports them as warnings, which is the right nudge.
fn default_header(schema: &Schema, profile: ProfileId, content: &Mrid) -> ModelHeader {
    let def = schema.profile(profile);
    let name = format!(
        "{}\u{1e}{}\u{1e}{}",
        schema.vintage, def.version_iri, content
    );
    ModelHeader {
        kind: ModelKind::Full,
        id: Some(Mrid::new_v5(&Dataset::DERIVED_NS, name.as_bytes())),
        profiles: vec![def.version_iri.to_owned()],
        version: Some("1".to_owned()),
        ..Default::default()
    }
}

/// Write a dataset to any sink, as a single document.
pub fn write_to<W: Write>(dataset: &Dataset, out: W, options: &WriteOptions) -> Result<()> {
    crate::writer::write(dataset, out, options)
}

// ---------------------------------------------------------------------------
// Zip archives
// ---------------------------------------------------------------------------

#[cfg(feature = "zip")]
impl Dataset {
    /// Load every CIM/XML entry in a zip archive.
    ///
    /// CGMES model sets are routinely distributed as archives, sometimes with one
    /// instance file per archive and sometimes with a whole model set inside one.
    pub fn load_zip(&mut self, path: &Path, options: &ReadOptions) -> Result<LoadReport> {
        use crate::error::Error;

        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(BufReader::new(file))
            .map_err(|e| Error::Zip(format!("{}: {e}", path.display())))?;

        // Read entries in name order so a load is reproducible.
        let mut names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_owned()))
            .filter(|n| n.to_ascii_lowercase().ends_with(".xml"))
            .collect();
        names.sort();

        let mut out = LoadReport::default();
        for name in names {
            let entry = archive
                .by_name(&name)
                .map_err(|e| Error::Zip(format!("{name}: {e}")))?;
            let outcome = read_into(
                self,
                BufReader::with_capacity(1 << 16, entry),
                Some(&name),
                options,
            )?;
            out.report.extend(outcome.report);
            out.objects_read += outcome.objects_read;
            out.files
                .push((PathBuf::from(&name), outcome.header.clone()));
        }
        Ok(out)
    }

    /// Write every populated profile into one zip archive.
    pub fn save_zip(&self, path: &Path, stem: &str) -> Result<Vec<String>> {
        use crate::error::Error;
        use zip::write::SimpleFileOptions;

        let file = File::create(path)?;
        let mut zw = zip::ZipWriter::new(BufWriter::new(file));
        let schema = self.schema();
        let mut written = Vec::new();
        // Once for the archive, not once per profile: it walks the whole model.
        let content = self.content_id();

        for cov in crate::validate::profile_coverage(self) {
            if cov.objects == 0 {
                continue;
            }
            let name = format!("{stem}_{}.xml", cov.keyword);
            zw.start_file(&name, SimpleFileOptions::default())
                .map_err(|e| Error::Zip(e.to_string()))?;
            let options = WriteOptions::default();
            write_profile(
                self,
                cov.profile,
                &mut zw,
                Some(default_header(schema, cov.profile, &content)),
                &options,
            )?;
            written.push(name);
        }
        zw.finish().map_err(|e| Error::Zip(e.to_string()))?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::{output_file_name, unique_name};

    #[test]
    fn two_files_wanting_one_name_get_two_files() {
        // A merged common grid model assembled from `BE/EQ.xml` and `NL/EQ.xml`: only the
        // final component survives the traversal guard, so both headers ask for `EQ.xml`.
        let mut used = Vec::new();
        assert_eq!(unique_name("EQ.xml".into(), &mut used), "EQ.xml");
        assert_eq!(unique_name("EQ.xml".into(), &mut used), "EQ_2.xml");
        assert_eq!(unique_name("EQ.xml".into(), &mut used), "EQ_3.xml");
        assert_eq!(unique_name("SSH.xml".into(), &mut used), "SSH.xml");
        // A name without an extension still gets a distinct one.
        assert_eq!(unique_name("model".into(), &mut used), "model");
        assert_eq!(unique_name("model".into(), &mut used), "model_2");
        // And a collision with an already-suffixed name keeps counting rather than looping.
        assert_eq!(unique_name("EQ_2.xml".into(), &mut used), "EQ_2_2.xml");
    }

    #[test]
    fn an_export_never_escapes_its_output_directory() {
        // A header's source is the name of the document it was read from, and for a zip
        // archive that name is chosen by whoever built the archive.
        assert_eq!(output_file_name("EQ.xml").as_deref(), Some("EQ.xml"));
        assert_eq!(
            output_file_name("model/EQ.xml").as_deref(),
            Some("EQ.xml"),
            "a nested archive entry keeps only its file name"
        );
        assert_eq!(
            output_file_name("../../../etc/cron.d/pwn").as_deref(),
            Some("pwn")
        );
        assert_eq!(output_file_name("/etc/passwd").as_deref(), Some("passwd"));
        for hostile in ["..", ".", "", "/", "..\\..\\evil.xml", "a\0b"] {
            assert_eq!(output_file_name(hostile), None, "{hostile:?}");
        }
    }
}
