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
use crate::writer::{WriteOptions, conventional_id_style, write_profile};

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
        match outcome.header {
            Some(h) => self.push_header(h),
            None => report.report.push(
                Diagnostic::warning(Rule::MalformedHeader, "file has no md:FullModel header")
                    .with_source(name.unwrap_or_default()),
            ),
        }
        Ok(report)
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
        for s in &diff.reverse {
            self.apply_statement(s, false, &mut report);
        }
        for s in &diff.forward {
            self.apply_statement(s, true, &mut report);
        }
        report
    }

    fn apply_statement(&mut self, s: &Statement, assert: bool, report: &mut Report) {
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
                .with_object(s.subject.canonical()),
            );
            return;
        };
        let def = schema.attr(attr);

        let value = match &s.value {
            StatementValue::Resource(iri) => match def.kind {
                AttrKind::Enumeration(_) => match iri.rsplit_once('#') {
                    Some((ns, local)) => match schema.find_enum_value(&format!("{ns}#"), local) {
                        Some(v) => Value::Enum(v),
                        None => {
                            report.push(
                                Diagnostic::error(
                                    Rule::InvalidValue,
                                    format!("unknown enumeration literal {iri}"),
                                )
                                .with_attribute(def.name)
                                .with_object(s.subject.canonical()),
                            );
                            return;
                        }
                    },
                    None => {
                        report.push(
                            Diagnostic::error(
                                Rule::InvalidValue,
                                format!("enumeration value {iri} is not an IRI"),
                            )
                            .with_attribute(def.name),
                        );
                        return;
                    }
                },
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
                                .with_object(s.subject.canonical()),
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
                    .as_deref()
                    .and_then(|c| schema.find_class(&s.predicate_ns, c))
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

        let Some(obj) = self.get_mut(id) else { return };
        if assert {
            if def.mult.is_many() {
                obj.push(attr, value);
            } else {
                obj.set(attr, value);
            }
        } else if !obj.remove_value(attr, &value) {
            report.push(
                Diagnostic::info(
                    Rule::Structure,
                    format!("difference removes {} which was not present", def.name),
                )
                .with_object(s.subject.canonical()),
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
        let options = WriteOptions {
            id_style: conventional_id_style(schema, profile),
            ..Default::default()
        };
        let header = header.or_else(|| Some(default_header(schema, profile)));
        let file = File::create(path)?;
        write_profile(
            self,
            profile,
            BufWriter::with_capacity(1 << 16, file),
            header,
            &options,
        )
    }

    /// Write every profile the dataset carries data for, into `dir`.
    ///
    /// Files are named `<stem>_<KEYWORD>.xml`. Returns the paths written.
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
}

/// A minimal conforming header for a profile export.
fn default_header(schema: &Schema, profile: ProfileId) -> ModelHeader {
    ModelHeader {
        kind: ModelKind::Full,
        id: None,
        profiles: vec![schema.profile(profile).version_iri.to_owned()],
        version: Some("001".to_owned()),
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
            if let Some(h) = outcome.header {
                self.push_header(h);
            }
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

        for cov in crate::validate::profile_coverage(self) {
            if cov.objects == 0 {
                continue;
            }
            let name = format!("{stem}_{}.xml", cov.keyword);
            zw.start_file(&name, SimpleFileOptions::default())
                .map_err(|e| Error::Zip(e.to_string()))?;
            let options = WriteOptions {
                id_style: conventional_id_style(schema, cov.profile),
                ..Default::default()
            };
            write_profile(
                self,
                cov.profile,
                &mut zw,
                Some(default_header(schema, cov.profile)),
                &options,
            )?;
            written.push(name);
        }
        zw.finish().map_err(|e| Error::Zip(e.to_string()))?;
        Ok(written)
    }
}
