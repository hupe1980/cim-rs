//! Cross-validation: what *other* implementations see in what this crate writes.
//!
//! # The gap this closes
//!
//! Every check in this repository so far is written by this repository. The corpus census
//! compares our output to the input we parsed; `check_well_formed` and `check_ntriples`
//! judge it against grammars we implemented; even the SHACL run, which uses a real engine,
//! is driven by shapes we selected against a graph we produced. All of it can be true while
//! `cim-rs` misunderstands CGMES in a way that is *internally consistent* — a writer and a
//! reader that share a misconception agree with each other perfectly, and a round trip is
//! exactly the test that cannot see one.
//!
//! The only way out is an implementation that does not share our assumptions. Two run here,
//! in two languages, neither of them ours:
//!
//! * **PowSyBl** (Java, LF Energy) imports a CGMES model set into its own network object
//!   model. It is asked what it found in the *original* published files and what it found
//!   in `cim-rs`'s re-export of them. The two answers must be identical — same counts of
//!   every kind of equipment, and the same set of identifiers. That is the strongest
//!   statement available about a serializer: *a different implementation cannot tell our
//!   output apart from the input.*
//! * **rdflib** (Python) parses the RDF export. Not with our N-Triples grammar checker, and
//!   not to validate shapes, but to build a graph and report what is in it — including the
//!   histogram of literal datatypes, which is the specific claim `src/rdf.rs` exists to
//!   make and the one thing no other check in the repository would notice losing.
//!
//! # Why containers, and why not testcontainers
//!
//! Containers, because a cross-validation is worth nothing if the reference implementation
//! drifts: a pinned image makes a failure *our* regression rather than the other project's
//! release notes. They also mean a contributor needs Docker and neither a JDK nor a Python
//! environment.
//!
//! Not `testcontainers`, which is built for something else. It manages the lifecycle of
//! *service* containers — a database to connect to, with a dynamic port and a readiness
//! probe — and its value is the waiting and the port mapping. What happens here is a batch
//! process over a mounted directory that writes JSON to stdout and exits: there is no port,
//! no readiness, and nothing to wait for that `docker run --rm` does not already do. Adding
//! it would put a dependency tree behind a call to `docker run` that this module makes in
//! one line.
//!
//! ```text
//! cargo xtask crossvalidate                    # MicroGrid Type 1, the default
//! cargo xtask crossvalidate <model-directory>
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// A *complete* model set, deliberately: the assembled MicroGrid carries its own boundary,
/// so PowSyBl builds a network with resolved tie lines rather than a bag of dangling ones.
/// Pointing at a parent directory instead would load several authorities' models at once
/// and cross-validate an assembly no exchange ever produces.
const DEFAULT_MODEL: &str = "specs/test-models/cas-3.0.3/\
    CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3/v3.0/\
    MicroGrid/MicroGrid-Type1/MicroGrid-Type1-Merged";

/// Image tags. Pinned by content in the sense that the Dockerfiles pin their dependencies;
/// the tag only has to be stable enough not to collide with anything else on the machine.
const POWSYBL_IMAGE: &str = "cim-rs-crossvalidate-powsybl:local";
const RDFLIB_IMAGE: &str = "cim-rs-crossvalidate-rdflib:local";

/// A model set as given on the command line: a directory of instance files, or the single
/// zip archive a CGMES exchange is normally packaged as.
///
/// Both shapes have to work, and not for symmetry: ENTSO-E ships the CGMES 3.0 conformity
/// configurations as directories and the CGMES 2.4.15 ones as archives, so covering both
/// vintages means covering both shapes.
struct Input {
    /// The directory to mount into the container.
    mount: PathBuf,
    /// What to name inside it — the mount point itself for a directory, a file within it
    /// for an archive.
    target: String,
}

impl Input {
    fn resolve(path: &Path) -> Result<Input> {
        if path.is_dir() {
            return Ok(Input {
                mount: path.to_path_buf(),
                target: "/model".to_owned(),
            });
        }
        if path.is_file() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .context("model archive has no usable name")?;
            let parent = path.parent().unwrap_or(Path::new("."));
            return Ok(Input {
                mount: parent.to_path_buf(),
                target: format!("/model/{name}"),
            });
        }
        bail!(
            "model set not found: {}\nRun `cargo xtask fetch-specs` first.",
            path.display()
        )
    }
}

pub fn check(root: &Path, model: Option<&str>, vintage: &str) -> Result<()> {
    let model_path = match model {
        Some(m) => PathBuf::from(m),
        None => root.join(DEFAULT_MODEL),
    };
    let input = Input::resolve(&model_path)?;
    ensure_docker()?;

    let tmp = TempDir::new(root, "crossvalidate")?;
    println!("{} ({vintage})\n", model_path.display());

    let mut failed = false;
    failed |= !powsybl(root, &model_path, &input, tmp.path(), vintage)?;
    println!();
    failed |= !rdflib(root, &model_path, tmp.path(), vintage)?;

    println!();
    if failed {
        bail!("cross-validation found a disagreement");
    }
    println!("every implementation agrees");
    Ok(())
}

// ---------------------------------------------------------------------------
// PowSyBl: the same network from our output as from the original
// ---------------------------------------------------------------------------

/// Import both file sets with PowSyBl and require the two networks to be identical.
fn powsybl(root: &Path, model: &Path, input: &Input, tmp: &Path, vintage: &str) -> Result<bool> {
    println!("PowSyBl (Java) — the network it builds from each file set");
    build_image(root, "crossvalidate/powsybl", POWSYBL_IMAGE)?;

    // Re-export through the command line rather than a private copy, for the reason
    // `shacl.rs` gives: what the reference implementation judges has to be what a user of
    // the tool would actually get.
    let exported = tmp.join("exported");
    std::fs::create_dir_all(&exported)?;
    cim(root, &["export"], model, &exported, vintage)?;

    let before = summarize_network(input)?;
    // The re-export is always a directory, whatever shape the input had.
    let after = summarize_network(&Input::resolve(&exported)?)?;

    // A model set PowSyBl declines to read at all is not a pass. Its importer is tolerant,
    // so an empty network is what a total misunderstanding would look like rather than an
    // error, and reporting "identical" for two empty networks is the one way this check
    // could be vacuously green.
    let identifiables: u64 = before
        .get("identifiables")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if identifiables == 0 {
        bail!(
            "PowSyBl built an empty network from the *original* files, so it cannot say \
             anything about ours; the model set or the importer is the problem, not cim-rs"
        );
    }

    report_diff(&before, &after)
}

/// Ask the container what PowSyBl found in one file set.
fn summarize_network(input: &Input) -> Result<BTreeMap<String, String>> {
    // Mounted read-only: the container has no business writing into a model set, and the
    // published corpus is the one thing here that cannot be regenerated.
    let json = docker_run(POWSYBL_IMAGE, &input.mount, "/model", &[&input.target])
        .with_context(|| format!("PowSyBl reading {}", input.mount.display()))?;
    parse_flat_json(&json)
}

// ---------------------------------------------------------------------------
// rdflib: the RDF export as an actual graph
// ---------------------------------------------------------------------------

/// Parse the RDF export with rdflib and check it is a graph rather than a text file.
fn rdflib(root: &Path, model: &Path, tmp: &Path, vintage: &str) -> Result<bool> {
    println!("rdflib (Python) — the graph it parses from the RDF export");
    build_image(root, "crossvalidate/rdflib", RDFLIB_IMAGE)?;

    let graphs = tmp.join("graphs");
    std::fs::create_dir_all(&graphs)?;
    cim(root, &["rdf", "--merged"], model, &graphs, vintage)?;

    // `--merged` writes one graph for the whole model; find it rather than assuming a name,
    // since the name is the tool's business and not this check's.
    let graph = std::fs::read_dir(&graphs)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "ttl" || e == "nt"))
        .context("`cim rdf --merged` wrote no graph")?;
    let name = graph.file_name().expect("a file").to_string_lossy();

    let summary = parse_flat_json(&docker_run(
        RDFLIB_IMAGE,
        &graphs,
        "/graphs",
        &[&format!("/graphs/{name}")],
    )?)?;

    let n = |k: &str| -> u64 { summary.get(k).and_then(|v| v.parse().ok()).unwrap_or(0) };
    let mut ok = true;

    for (key, what) in [
        ("triples", "triples parsed"),
        ("namedSubjects", "subjects with a global name"),
        ("typedSubjects", "subjects carrying rdf:type"),
    ] {
        let value = n(key);
        println!("  {what:<34} {value:>8}");
        if value == 0 {
            println!("        ^ zero: the graph is not usable for what it claims to be");
            ok = false;
        }
    }

    // Reported, not failed on: both plausible assertions here are wrong.
    //
    // "Every subject must be named" fails on any model carrying a postal address — a CIM
    // *compound* has no identity, so a blank node is the only correct rendering. "A class
    // must not appear both named and blank" fires on the published CGMES 2.4.15 MicroGrid,
    // where 43 `CurrentLimit`s are identified by `_88240efd-…-e6b77d4bac881`: a UUID with a
    // digit appended, which has no `urn:uuid:` form and cannot be given one without the
    // fabrication `Mrid` exists to refuse. That model is non-conforming (`CIM0004`) and the
    // graph is right to show it, so this is information rather than a verdict.
    let blank = n("subjects").saturating_sub(n("namedSubjects"));
    println!("  {:<34} {blank:>8}", "blank subjects");
    if blank > 0 {
        println!(
            "        expected: a compound has no identity, and an object whose mRID is \
             not a UUID has no urn:uuid: form (CIM0004)"
        );
    }

    // The claim `src/rdf.rs` exists to make. If the profile enrichment silently stopped
    // working, every literal would be a plain string and nothing else in the repository
    // would fail — the N-Triples grammar accepts plain literals, and so do the shapes for
    // any property they do not constrain.
    let typed: u64 = summary
        .iter()
        .filter(|(k, _)| k.starts_with("datatypes."))
        .filter_map(|(_, v)| v.parse::<u64>().ok())
        .sum();
    println!("  {:<34} {typed:>8}", "typed literals");
    if typed == 0 {
        println!("        ^ no literal carries a datatype; the profile enrichment is gone");
        ok = false;
    }
    for (k, v) in summary.iter().filter(|(k, _)| k.starts_with("datatypes.")) {
        println!("        {v:>7}x {}", k.trim_start_matches("datatypes."));
    }

    println!("  {}", if ok { "OK" } else { "FAIL" });
    Ok(ok)
}

// ---------------------------------------------------------------------------
// Comparison and reporting
// ---------------------------------------------------------------------------

/// Print the two summaries side by side and say whether they agree.
fn report_diff(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Result<bool> {
    let mut keys: Vec<&String> = before.keys().chain(after.keys()).collect();
    keys.sort();
    keys.dedup();

    let mut ok = true;
    for key in keys {
        let a = before.get(key).map(String::as_str).unwrap_or("-");
        let b = after.get(key).map(String::as_str).unwrap_or("-");
        let same = a == b;
        ok &= same;
        // A digest is 64 characters and only its equality matters, so it is not printed in
        // full unless it is what disagrees.
        let show = |v: &str| {
            if v.len() > 16 && same {
                format!("{}…", &v[..8])
            } else {
                v.to_owned()
            }
        };
        // Padded rather than formatted with a width, because a key longer than the column
        // pushes everything after it out of line and the table stops being scannable —
        // which is the whole reason for printing them side by side.
        let name = format!("{key:.<36}");
        println!(
            "  {name} {:>10} {} {:<10}{}",
            show(a),
            if same { "==" } else { "!=" },
            show(b),
            if same { "" } else { "   <-- differs" }
        );
    }
    println!("  {}", if ok { "OK" } else { "FAIL" });
    Ok(ok)
}

// ---------------------------------------------------------------------------
// Docker and the command line
// ---------------------------------------------------------------------------

fn ensure_docker() -> Result<()> {
    let ok = Command::new("docker")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        bail!(
            "`docker` is not runnable.\nCross-validation runs other projects' \
             implementations in pinned containers, so that a disagreement is this crate's \
             regression rather than somebody else's release. Start Docker and try again."
        );
    }
    Ok(())
}

/// Build one of the harness images, quietly unless it fails.
fn build_image(root: &Path, context: &str, tag: &str) -> Result<()> {
    let dir = root.join(context);
    if !dir.is_dir() {
        bail!("missing cross-validation harness: {}", dir.display());
    }
    let out = Command::new("docker")
        .args(["build", "--quiet", "-t", tag])
        .arg(&dir)
        .output()
        .context("running `docker build`")?;
    if !out.status.success() {
        bail!(
            "building {tag} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Run a harness image over `host_dir`, returning its stdout.
fn docker_run(tag: &str, host_dir: &Path, mount: &str, args: &[&str]) -> Result<String> {
    let host = host_dir
        .canonicalize()
        .with_context(|| format!("resolving {}", host_dir.display()))?;
    let mut cmd = Command::new("docker");
    cmd.args(["run", "--rm", "-v"])
        // Read-only: nothing here needs to write, and the published corpus cannot be
        // regenerated if something does.
        .arg(format!("{}:{mount}:ro", host.display()))
        .arg(tag)
        .args(args);
    let out = cmd.output().context("running `docker run`")?;
    if !out.status.success() {
        bail!(
            "{tag} failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run the crate's own command line, as a user would.
fn cim(root: &Path, subcommand: &[&str], model: &Path, out: &Path, vintage: &str) -> Result<()> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args([
            "run",
            "--release",
            "--quiet",
            "--bin",
            "cim",
            "--features",
            // Both vintages and `zip`, because which one a model set needs is the caller's
            // choice and CGMES 2.4.15 is published as archives.
            "cli,cgmes3,cgmes2,zip",
            "--",
        ])
        .args(subcommand)
        .args(["--quiet", "--vintage", vintage])
        .arg(model)
        .arg("--out")
        .arg(out)
        .status()
        .context("running `cim`")?;
    if !status.success() {
        bail!("`cim {}` failed on {}", subcommand[0], model.display());
    }
    Ok(())
}

/// Read the flat JSON object the harnesses print.
///
/// Deliberately a small reader rather than a JSON dependency: both harnesses emit one
/// object of string and number values, at most one level of nesting, and the whole grammar
/// that has to be understood is what they are documented to write. A nested object is
/// flattened to `parent.child` keys, which is what makes rdflib's datatype histogram
/// readable without a second shape.
fn parse_flat_json(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let body = text
        .trim()
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .with_context(|| format!("not a JSON object: {}", first_line(text)))?;

    let mut rest = body;
    while let Some((key, tail)) = take_key(rest) {
        let (value, tail) = take_value(tail)
            .with_context(|| format!("no value for {key:?} in {}", first_line(text)))?;
        match value {
            // A nested object contributes `parent.child` keys; `take_value` returns its body.
            Value::Object(inner) => {
                let mut inner_rest = inner;
                while let Some((k, t)) = take_key(inner_rest) {
                    let (v, t) = take_value(t).context("nested value")?;
                    if let Value::Scalar(s) = v {
                        out.insert(format!("{key}.{k}"), s);
                    }
                    inner_rest = t.trim_start().strip_prefix(',').unwrap_or(t);
                }
            }
            Value::Scalar(s) => {
                out.insert(key, s);
            }
        }
        rest = tail.trim_start().strip_prefix(',').unwrap_or(tail);
        if rest.trim().is_empty() {
            break;
        }
    }
    if out.is_empty() {
        bail!("no fields in harness output: {}", first_line(text));
    }
    Ok(out)
}

enum Value<'a> {
    Scalar(String),
    Object(&'a str),
}

/// Take a `"key":` from the front, returning the key and what follows the colon.
fn take_key(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let rest = s.strip_prefix('"')?;
    let end = rest.find('"')?;
    let after = rest[end + 1..].trim_start().strip_prefix(':')?;
    Some((rest[..end].to_owned(), after))
}

/// Take one value from the front: a quoted string, a nested object, or a bare number.
fn take_value(s: &str) -> Option<(Value<'_>, &str)> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((Value::Scalar(rest[..end].to_owned()), &rest[end + 1..]));
    }
    if let Some(rest) = s.strip_prefix('{') {
        // One level of nesting is all the harnesses emit, so the matching brace is the
        // next one rather than a balanced search.
        let end = rest.find('}')?;
        return Some((Value::Object(&rest[..end]), &rest[end + 1..]));
    }
    let end = s
        .find([',', '}'])
        .unwrap_or(s.len())
        .min(s.find(char::is_whitespace).unwrap_or(s.len()));
    (end > 0).then(|| (Value::Scalar(s[..end].to_owned()), &s[end..]))
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

/// A scratch directory that cleans itself up.
struct TempDir(PathBuf);

impl TempDir {
    fn new(root: &Path, name: &str) -> Result<TempDir> {
        let dir = root
            .join("target")
            .join(format!("{name}-{}", std::process::id()));
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
    fn a_harness_summary_reads_back_as_fields() {
        let json = r#"{"substations":14,"identifierDigest":"ab12","lines":3}"#;
        let got = parse_flat_json(json).unwrap();
        assert_eq!(got["substations"], "14");
        assert_eq!(got["identifierDigest"], "ab12");
        assert_eq!(got["lines"], "3");
    }

    /// rdflib's datatype histogram is a nested object, and the keys are IRIs containing
    /// both `#` and `:` — so the reader must not treat those as structure.
    #[test]
    fn a_nested_histogram_is_flattened_onto_its_parent() {
        let json = concat!(
            r#"{"triples":9,"datatypes":{"http://www.w3.org/2001/XMLSchema#float":7,"#,
            r#""http://www.w3.org/2001/XMLSchema#integer":2},"subjects":4}"#
        );
        let got = parse_flat_json(json).unwrap();
        assert_eq!(got["triples"], "9");
        assert_eq!(got["subjects"], "4");
        assert_eq!(got["datatypes.http://www.w3.org/2001/XMLSchema#float"], "7");
        assert_eq!(
            got["datatypes.http://www.w3.org/2001/XMLSchema#integer"],
            "2"
        );
    }

    #[test]
    fn output_that_is_not_a_summary_is_an_error_rather_than_an_empty_result() {
        // The failure that matters: a harness that logged to stdout instead of stderr, or
        // died before printing. Silently reporting "no fields, therefore equal" would make
        // the whole check pass vacuously.
        for bad in ["", "Exception in thread \"main\"\n", "{}", "not json"] {
            assert!(parse_flat_json(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn two_identical_summaries_agree_and_one_changed_field_does_not() {
        let a = BTreeMap::from([
            ("substations".to_owned(), "14".to_owned()),
            ("identifierDigest".to_owned(), "abc".to_owned()),
        ]);
        assert!(report_diff(&a, &a).unwrap());

        let mut b = a.clone();
        b.insert("identifierDigest".to_owned(), "def".to_owned());
        assert!(!report_diff(&a, &b).unwrap());

        // A field only one side has is a disagreement too, not a skip.
        let mut c = a.clone();
        c.insert("lines".to_owned(), "3".to_owned());
        assert!(!report_diff(&a, &c).unwrap());
    }
}
