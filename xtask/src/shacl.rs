//! Validating the crate's RDF output against ENTSO-E's own SHACL shapes.
//!
//! This is the interoperability check the crate cannot make of itself. `cim-rs` writes the
//! model as ordinary RDF with every literal typed from the profile — the step the int:net
//! IOP report calls out as missing ("there are no open libraries to natively enhance the
//! data based on the profile definitions") — but running SHACL is deliberately **not** this
//! crate's job, so the proof that the output is usable has to come from a real engine.
//!
//! That engine is `pyshacl`, and it stays Python: there is no mature SHACL implementation
//! in Rust, and writing one would be a second project rather than a feature of this one.
//! What lives here is everything around it — selecting the shapes for each profile, driving
//! the export, reading the validation report, and deciding which findings are the published
//! models' own rather than ours.
//!
//! ```text
//! python3 -m venv .venv && .venv/bin/pip install pyshacl
//! cargo xtask shacl                       # MicroGrid Type 1, the default
//! cargo xtask shacl <model-directory>
//! ```
//!
//! Shapes are matched to profiles one for one, because a CGMES profile constrains a
//! reference's target to the classes *it* declares: validating a merged graph against one
//! profile's shapes reports hundreds of violations no instance file could have had. The
//! graph handed to the engine is that profile's slice, which is exactly what `cim rdf`
//! writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const SHACL_DIR: &str = "specs/application-profiles-library/CGMES/CurrentRelease/SHACL/TTL";
const DEFAULT_MODEL: &str = "specs/test-models/cas-3.0.3/\
    CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3/v3.0/MicroGrid/MicroGrid-Type1";

/// The "Simple" shape set is the one carrying the `sh:datatype` constraints — 3,137 of
/// them — which is what makes this a test of the datatype mapping and not only of the
/// structure.
const SHAPES: &[(&str, &str)] = &[
    ("EQ", "61970-600-2_Equipment-AP-Con-Simple-SHACL.ttl"),
    ("OP", "61970-600-2_Operation-AP-Con-Simple-SHACL.ttl"),
    ("SC", "61970-600-2_ShortCircuit-AP-Con-Simple-SHACL.ttl"),
    (
        "EQBD",
        "61970-600-2_EquipmentBoundary-AP-Con-Simple-SHACL.ttl",
    ),
    (
        "SSH",
        "61970-600-2_SteadyStateHypothesis-AP-Con-Simple-SHACL.ttl",
    ),
    ("TP", "61970-600-2_Topology-AP-Con-Simple-SHACL.ttl"),
    ("SV", "61970-600-2_StateVariables-AP-Con-Simple-SHACL.ttl"),
    ("DL", "61970-600-2_DiagramLayout-AP-Con-Simple-SHACL.ttl"),
    (
        "GL",
        "61970-600-2_GeographicalLocation-AP-Con-Simple-SHACL.ttl",
    ),
    ("DY", "61970-600-2_Dynamics-AP-Con-Simple-SHACL.ttl"),
];
const HEADER_SHAPES: &str = "61970-552-Header-AP-Con-Simple-SHACL.ttl";

/// Violations the published conformity models themselves carry.
///
/// Every Steady State Hypothesis file in the corpus writes `<cim:Equipment rdf:about="…">`
/// carrying nothing but `Equipment.inService`, and the SSH shapes require
/// `IdentifiedObject.mRID`. Reproducing that faithfully is the correct behaviour; hiding it
/// would not be.
const KNOWN: &[(&str, &str, &str)] = &[(
    "SSH",
    "IdentifiedObject.mRID",
    "MinCountConstraintComponent",
)];

pub fn check(root: &Path, model: Option<&str>) -> Result<()> {
    let shapes_dir = root.join(SHACL_DIR);
    if !shapes_dir.is_dir() {
        bail!(
            "SHACL shapes not found at {}\nRun `cargo xtask fetch-specs` first.",
            shapes_dir.display()
        );
    }
    let model_dir = match model {
        Some(m) => PathBuf::from(m),
        None => root.join(DEFAULT_MODEL),
    };
    if !model_dir.is_dir() {
        bail!("model set not found: {}", model_dir.display());
    }
    ensure_engine()?;

    let out = TempDir::new(root)?;
    export(root, &model_dir, out.path())?;
    println!("{}\n", model_dir.display());

    let mut failed = false;
    let mut validated = 0usize;
    for (keyword, shape_file) in SHAPES {
        let graph = out.path().join(format!("{keyword}.ttl"));
        // A profile this model carries no data for is not a pass, and telling the two apart
        // is `cim rdf`'s job rather than this harness's: it writes no graph for a profile
        // with no objects in it, so an absent file is the answer rather than a guess about
        // one. It used to be guessed from a line count, and the guess could not work — a
        // graph of nothing but the loaded files' headers runs to hundreds of lines, so the
        // threshold never fired and this harness reported empty profiles as validated.
        if !graph.is_file() {
            println!("  {keyword:<5} no data");
            continue;
        }
        let lines = std::fs::read_to_string(&graph)
            .map(|t| t.lines().count())
            .unwrap_or(0);

        let merged = out.path().join(format!("{keyword}-shapes.ttl"));
        concat_shapes(
            &[shapes_dir.join(shape_file), shapes_dir.join(HEADER_SHAPES)],
            &merged,
        )?;

        let found = validate(&graph, &merged, out.path())?;
        let unexpected: BTreeMap<_, _> = found
            .iter()
            .filter(|((path, component), _)| {
                !KNOWN
                    .iter()
                    .any(|(k, p, c)| k == keyword && p == path && c == component)
            })
            .collect();

        let note = if !found.is_empty() && unexpected.is_empty() {
            "  (only the published model's own violations)"
        } else {
            ""
        };
        let status = if unexpected.is_empty() { "OK" } else { "FAIL" };
        println!("  {keyword:<5} {lines:>6} lines  {status}{note}");
        for ((path, component), n) in &unexpected {
            println!("        {n:>5}x {component} on {path}");
        }
        failed |= !unexpected.is_empty();
        validated += 1;
    }

    println!();
    if failed {
        bail!("some profiles do not conform");
    }
    // "Every profile conforms" is worth nothing if every profile was skipped. A model set
    // this harness is pointed at has data for several profiles; none at all means the export
    // produced nothing, which is a failure that would otherwise read as a clean run.
    if validated == 0 {
        bail!(
            "no profile of {} produced a graph to validate",
            model_dir.display()
        );
    }
    println!("{validated} profiles validated, all conforming");
    Ok(())
}

/// Check that a SHACL engine is actually available, before doing minutes of export work.
fn ensure_engine() -> Result<()> {
    let ok = Command::new(engine())
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        bail!(
            "`{}` is not runnable.\nInstall a SHACL engine — running SHACL is not this \
             crate's job, so the check uses a real one:\n  python3 -m venv .venv && \
             .venv/bin/pip install pyshacl\n  PYSHACL=.venv/bin/pyshacl cargo xtask shacl",
            engine()
        );
    }
    Ok(())
}

/// The engine binary, overridable so a virtualenv does not have to be on `PATH`.
fn engine() -> String {
    std::env::var("PYSHACL").unwrap_or_else(|_| "pyshacl".to_owned())
}

/// Write one Turtle graph per profile, by running the crate's own command line.
///
/// A subprocess rather than a library call on purpose: `xtask` generates
/// `src/generated`, so making it depend on the crate that contains the generated sources
/// would mean a broken generated tree could not be regenerated.
///
/// Going through `cim rdf` rather than through a private copy of the export is the point:
/// what a SHACL engine judges here is what a user of the tool would actually get.
fn export(root: &Path, model: &Path, out: &Path) -> Result<()> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args([
            "run",
            "--release",
            "--quiet",
            "--bin",
            "cim",
            "--features",
            "cli,cgmes3",
            "--",
            "rdf",
            "--quiet",
        ])
        .arg(model)
        .arg("--out")
        .arg(out)
        .status()
        .context("running `cim rdf`")?;
    if !status.success() {
        bail!("exporting {} failed", model.display());
    }
    Ok(())
}

/// Join several shapes files into the single graph the engine takes.
///
/// Safe to concatenate because each published shapes file declares its own `@base` and
/// prefixes before using them, and Turtle allows both to be redeclared — so nothing in the
/// second file resolves against the first file's context.
fn concat_shapes(files: &[PathBuf], dest: &Path) -> Result<()> {
    let mut merged = String::new();
    for f in files {
        let text = std::fs::read_to_string(f)
            .with_context(|| format!("reading shapes {}", f.display()))?;
        merged.push_str(&text);
        merged.push('\n');
    }
    std::fs::write(dest, merged)?;
    Ok(())
}

/// Run the engine and count violations by `(path, constraint component)`.
fn validate(graph: &Path, shapes: &Path, tmp: &Path) -> Result<BTreeMap<(String, String), usize>> {
    let report = tmp.join("report.nt");
    let output = Command::new(engine())
        .args(["-s"])
        .arg(shapes)
        // The report as N-Triples: flat, one statement per line, and enough to group the
        // findings without pulling in an RDF toolchain to read our own tool's output.
        .args(["-f", "nt", "-o"])
        .arg(&report)
        .args(["-a", "-i", "none"])
        .arg(graph)
        .output()
        .context("running the SHACL engine")?;
    // Exit status 1 means "does not conform", which is a result rather than a failure;
    // anything else is the engine itself going wrong.
    if !matches!(output.status.code(), Some(0) | Some(1)) {
        bail!(
            "SHACL engine failed on {}:\n{}",
            graph.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = std::fs::read_to_string(&report).context("reading the validation report")?;
    Ok(count_violations(&text))
}

const SH: &str = "http://www.w3.org/ns/shacl#";

/// Group a SHACL validation report into `(resultPath, sourceConstraintComponent) -> count`.
///
/// The report is a flat graph of `sh:ValidationResult` nodes, so the three properties that
/// matter are collected per subject and joined at the end — no general RDF machinery
/// needed for a document this shape.
fn count_violations(report: &str) -> BTreeMap<(String, String), usize> {
    #[derive(Default)]
    struct Result_ {
        severity: Option<String>,
        path: Option<String>,
        component: Option<String>,
    }

    let mut results: BTreeMap<String, Result_> = BTreeMap::new();
    for line in report.lines() {
        let Some((subject, predicate, object)) = triple(line) else {
            continue;
        };
        let entry = results.entry(subject).or_default();
        match predicate.strip_prefix(SH) {
            Some("resultSeverity") => entry.severity = Some(local(&object)),
            Some("resultPath") => entry.path = Some(local(&object)),
            Some("sourceConstraintComponent") => entry.component = Some(local(&object)),
            _ => {}
        }
    }

    let mut out: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in results.into_values() {
        if r.severity.as_deref() != Some("Violation") {
            continue;
        }
        let key = (
            r.path.unwrap_or_else(|| "?".to_owned()),
            r.component.unwrap_or_else(|| "?".to_owned()),
        );
        *out.entry(key).or_default() += 1;
    }
    out
}

/// Split one N-Triples line into subject, predicate and object.
///
/// Only IRI and blank-node terms are of interest here — every property this reads has a
/// node or an IRI on the right — so a literal object simply yields nothing to match.
fn triple(line: &str) -> Option<(String, String, String)> {
    let line = line.trim().strip_suffix('.')?.trim_end();
    let (subject, rest) = term(line)?;
    let (predicate, rest) = term(rest.trim_start())?;
    let (object, _) = term(rest.trim_start())?;
    Some((subject, predicate, object))
}

fn term(s: &str) -> Option<(String, &str)> {
    if let Some(body) = s.strip_prefix('<') {
        let end = body.find('>')?;
        return Some((body[..end].to_owned(), &body[end + 1..]));
    }
    if s.starts_with("_:") {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        return Some((s[..end].to_owned(), &s[end..]));
    }
    None
}

/// The local name of an IRI, which is how a finding is worth reporting to a human:
/// `…#MinCountConstraintComponent`, `…#IdentifiedObject.mRID`.
fn local(iri: &str) -> String {
    iri.rsplit(['#', '/']).next().unwrap_or(iri).to_owned()
}

/// A scratch directory that cleans itself up.
struct TempDir(PathBuf);

impl TempDir {
    fn new(root: &Path) -> Result<TempDir> {
        let dir = root
            .join("target")
            .join(format!("shacl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(TempDir(dir))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_validation_report_is_grouped_by_path_and_constraint() {
        // The shape a pyshacl report actually has: one blank node per result, its
        // properties on separate lines and in no particular order.
        let report = concat!(
            "_:r1 <http://www.w3.org/ns/shacl#resultSeverity> ",
            "<http://www.w3.org/ns/shacl#Violation> .\n",
            "_:r1 <http://www.w3.org/ns/shacl#resultPath> ",
            "<http://iec.ch/TC57/CIM100#IdentifiedObject.mRID> .\n",
            "_:r1 <http://www.w3.org/ns/shacl#sourceConstraintComponent> ",
            "<http://www.w3.org/ns/shacl#MinCountConstraintComponent> .\n",
            "_:r2 <http://www.w3.org/ns/shacl#resultPath> ",
            "<http://iec.ch/TC57/CIM100#IdentifiedObject.mRID> .\n",
            "_:r2 <http://www.w3.org/ns/shacl#sourceConstraintComponent> ",
            "<http://www.w3.org/ns/shacl#MinCountConstraintComponent> .\n",
            "_:r2 <http://www.w3.org/ns/shacl#resultSeverity> ",
            "<http://www.w3.org/ns/shacl#Violation> .\n",
            // A warning is not a violation and must not be counted.
            "_:r3 <http://www.w3.org/ns/shacl#resultSeverity> ",
            "<http://www.w3.org/ns/shacl#Warning> .\n",
            "_:r3 <http://www.w3.org/ns/shacl#resultPath> ",
            "<http://iec.ch/TC57/CIM100#ACLineSegment.r> .\n",
            "_:r3 <http://www.w3.org/ns/shacl#sourceConstraintComponent> ",
            "<http://www.w3.org/ns/shacl#DatatypeConstraintComponent> .\n",
            // A literal object must not derail the line reader.
            "_:r3 <http://www.w3.org/ns/shacl#resultMessage> \"less than 1 values\" .\n",
        );

        let counts = count_violations(report);
        assert_eq!(
            counts,
            BTreeMap::from([(
                (
                    "IdentifiedObject.mRID".to_owned(),
                    "MinCountConstraintComponent".to_owned()
                ),
                2
            )])
        );
    }

    #[test]
    fn an_empty_report_means_the_graph_conforms() {
        assert!(count_violations("").is_empty());
        assert!(count_violations("# nothing here\n").is_empty());
    }

    #[test]
    fn iri_local_names_are_what_a_reader_recognises() {
        assert_eq!(
            local("http://iec.ch/TC57/CIM100#ACLineSegment.r"),
            "ACLineSegment.r"
        );
        assert_eq!(local("http://www.w3.org/ns/shacl#Violation"), "Violation");
        assert_eq!(local("urn:uuid:abc"), "urn:uuid:abc");
    }
}
