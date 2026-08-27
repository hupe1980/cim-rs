//! `cim` — a command line over the library, behind the `cli` feature.
//!
//! ```text
//! cargo install cim-rs --features cli
//! cim info     MicroGrid-BE/
//! cim validate MicroGrid-BE/ --rule CIM0007
//! cim rdf      MicroGrid-BE/ --out graphs/
//! cim diff     before/ after/ --out change_DIFF.xml
//! ```
//!
//! Everything here is a thin shell around the public API — deliberately, so that the tool
//! cannot do something a caller of the library could not, and so that a gap in the tool is
//! a gap in the library. It takes no dependency of its own, including for argument
//! parsing: the grammar is six subcommands and a handful of flags, and a parser for that
//! is shorter than the manifest entry for a crate that would do it.
//!
//! Output goes through an explicit writer rather than `println!` for one reason:
//! `println!` *panics* when its reader goes away, so `cim info big-model/ | head` — which
//! is how anyone will actually use it — would end in a backtrace. A closed pipe is a
//! normal way for a command to be told it has said enough.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cim_rs::VINTAGES;
use cim_rs::prelude::*;
use cim_rs::rdf::{RdfOptions, Syntax};
use cim_rs::schema::{ProfileId, Schema};

const USAGE: &str = "\
cim — IEC CIM / CGMES grid models

usage: cim <command> [options] <input>...

commands:
  info       <input>...              what a model set contains
  validate   <input>...              structural checks; exits 1 on any error
  export     <input>... --out DIR    write the model back as CIM/XML
  rdf        <input>... --out DIR    export as RDF, one graph per profile with data
  diff       <base> <target>         compute a dm:DifferenceModel
  schema                             what the built-in vintages declare

an <input> is a CIM/XML file, a zip archive, or a directory holding either; several are
loaded into one model, which is what a CGMES profile set is.

options:
  -o, --out PATH    where to write (a directory, or a file for `diff`)
  --vintage KEY     which schema to read against (default: detected from the input)
  --profile KEY     restrict `rdf` and `diff` to one profile, e.g. SSH
  --rule CODE       show only this rule in `validate`, e.g. CIM0007
  --ntriples        write N-Triples rather than Turtle
  --merged          `rdf` writes one graph for the whole model, not one per profile
  --strict          fail on anything the schema does not define
  --limit N         how many findings to list (default 20; 0 for all)
  -q, --quiet       less chatter
  -h, --help        this text

exit status: 0 clean, 1 the model has errors, 2 the command was wrong.
";

fn main() -> ExitCode {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let code = match run(&mut out) {
        Ok(code) => code,
        // A closed pipe is not a failure: `cim info … | head` is a normal thing to type.
        Err(Fail::Pipe) => ExitCode::SUCCESS,
        Err(Fail::Message(m)) => {
            let _ = writeln!(io::stderr(), "cim: {m}");
            ExitCode::from(2)
        }
    };
    match out.flush() {
        Ok(()) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(io::stderr(), "cim: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(out: &mut dyn Write) -> Result<ExitCode, Fail> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        write!(out, "{USAGE}")?;
        return Ok(ExitCode::SUCCESS);
    }
    let Some(command) = args.command.as_deref() else {
        write!(out, "{USAGE}")?;
        return Ok(ExitCode::from(2));
    };

    match command {
        "schema" => schema_command(out, &args),
        "info" => info(out, &args),
        "validate" => validate(out, &args),
        "export" => export(out, &args),
        "rdf" => rdf(out, &args),
        "diff" => diff(out, &args),
        other => Err(format!("unknown command {other:?}; `cim --help` lists them").into()),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn schema_command(out: &mut dyn Write, args: &Args) -> Result<ExitCode, Fail> {
    if VINTAGES.is_empty() {
        return Err(no_vintage().into());
    }
    for schema in VINTAGES {
        writeln!(
            out,
            "{} — {} classes, {} attributes, {} enumerations, {} profiles",
            schema.vintage,
            schema.classes.len(),
            schema.attributes.len(),
            schema.enums.len(),
            schema.profiles.len()
        )?;
        if !args.quiet {
            for p in schema.profiles {
                writeln!(out, "  {:<6} {}", p.keyword, p.version_iri)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn info(out: &mut dyn Write, args: &Args) -> Result<ExitCode, Fail> {
    let schema = args.schema()?;
    let (ds, report) = args.load(schema)?;

    writeln!(out, "== files ==")?;
    for h in ds.headers() {
        let profiles: Vec<&str> = h
            .profiles
            .iter()
            .filter_map(|iri| schema.profile_by_iri(iri))
            .map(|p| schema.profile(p).keyword)
            .collect();
        let profiles = match profiles.is_empty() {
            true => "(no profile)".to_owned(),
            false => profiles.join("+"),
        };
        writeln!(
            out,
            "  {:<44} {profiles:<18} {}",
            h.source.as_deref().unwrap_or("(unnamed)"),
            h.scenario_time.as_deref().unwrap_or("-")
        )?;
    }

    writeln!(out, "\n== model ==")?;
    writeln!(out, "  {:<20} {}", "objects", ds.len())?;
    writeln!(
        out,
        "  {:<20} {}",
        "values",
        ds.iter().map(|(_, o)| o.len()).sum::<usize>()
    )?;
    // Classes are looked up by local name, so the same command works on either vintage:
    // `Substation` sits in `CIM100` in CGMES 3.0 and in `cim16` in 2.4.15.
    for name in [
        "Substation",
        "ACLineSegment",
        "PowerTransformer",
        "SynchronousMachine",
        "EnergyConsumer",
        "Terminal",
    ] {
        if let Some(class) = schema.class_by_name(name) {
            writeln!(out, "  {name:<20} {}", ds.count_class(class))?;
        }
    }

    writeln!(out, "\n== profile coverage ==")?;
    for cov in cim_rs::validate::profile_coverage(&ds) {
        if cov.objects == 0 {
            continue;
        }
        writeln!(
            out,
            "  {:<6} {:>8} objects {:>10} values",
            cov.keyword, cov.objects, cov.attributes
        )?;
    }

    let mut all = report;
    all.extend(ds.validate());
    print_report(out, &all, args.limit, "diagnostics")?;
    Ok(exit_on_errors(&all))
}

fn validate(out: &mut dyn Write, args: &Args) -> Result<ExitCode, Fail> {
    let schema = args.schema()?;
    let (ds, report) = args.load(schema)?;
    let mut all = report;
    all.extend(ds.validate_with(&cim_rs::ValidateOptions::thorough()));

    if let Some(code) = &args.rule {
        let wanted = *cim_rs::Rule::ALL
            .iter()
            .find(|r| r.code().eq_ignore_ascii_case(code))
            .ok_or_else(|| format!("no such rule {code:?}; `cim validate` lists the codes"))?;
        all.diagnostics.retain(|d| d.rule == wanted);
    }

    print_report(out, &all, args.limit, "findings")?;
    Ok(exit_on_errors(&all))
}

fn export(out: &mut dyn Write, args: &Args) -> Result<ExitCode, Fail> {
    let schema = args.schema()?;
    let dir = args.out_dir()?;
    let (ds, report) = args.load(schema)?;
    let saved = ds
        .save_as_loaded(&dir)
        .map_err(|e| format!("writing to {}: {e}", dir.display()))?;
    // `--quiet` means the same thing here as in `rdf`: the files are on disk, so listing
    // them is a convenience rather than the result. What was *skipped* still goes to
    // stderr, because that is the caller not getting what they asked for.
    if !args.quiet {
        for p in &saved.written {
            writeln!(out, "{}", p.display())?;
        }
    }
    for s in &saved.skipped {
        let _ = writeln!(io::stderr(), "skipped: {s}");
    }
    Ok(exit_on_errors(&report))
}

fn rdf(out: &mut dyn Write, args: &Args) -> Result<ExitCode, Fail> {
    let schema = args.schema()?;
    let dir = args.out_dir()?;
    let (ds, report) = args.load(schema)?;
    let syntax = if args.ntriples {
        Syntax::NTriples
    } else {
        Syntax::Turtle
    };
    let extension = if args.ntriples { "nt" } else { "ttl" };

    // One graph per profile by default, because ENTSO-E's shapes are per profile: a
    // profile constrains a reference's target to the classes *it* declares, so a merged
    // graph reports violations no instance file could have had. `--merged` is for a triple
    // store, where the whole model in one graph is the point.
    let profiles: Vec<Option<ProfileId>> = if args.merged {
        vec![None]
    } else if args.profile.is_some() {
        vec![Some(args.profile_id(schema)?)]
    } else {
        (0..schema.profiles.len())
            .map(|i| Some(ProfileId(i as u16)))
            .collect()
    };

    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut empty: Vec<&str> = Vec::new();
    for profile in profiles {
        let (name, mask) = match profile {
            Some(p) => (schema.profile(p).keyword, p.mask()),
            None => ("model", 0),
        };
        // A profile the model carries no data for gets no file: a graph of nothing but the
        // loaded files' headers reads as an export and validates as one. `--merged` is
        // exempt — it asks for *the model*, and an empty model is a legitimate export.
        if profile.is_some() && !cim_rs::rdf::has_content(&ds, mask) {
            empty.push(name);
            continue;
        }
        let path = dir.join(format!("{name}.{extension}"));
        let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        cim_rs::rdf::write(
            &ds,
            io::BufWriter::with_capacity(1 << 16, file),
            &RdfOptions::new(syntax).profiles(mask),
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        if !args.quiet {
            writeln!(out, "{}", path.display())?;
        }
    }
    if !empty.is_empty() {
        let _ = writeln!(
            io::stderr(),
            "no data for {} — no graph written",
            empty.join(", ")
        );
    }
    Ok(exit_on_errors(&report))
}

fn diff(out: &mut dyn Write, args: &Args) -> Result<ExitCode, Fail> {
    let schema = args.schema()?;
    let [base, target] = match args.inputs.as_slice() {
        [a, b] => [a, b],
        _ => {
            return Err("diff takes exactly two inputs: <base> <target>"
                .to_owned()
                .into());
        }
    };
    let before = load_one(schema, base, args)?;
    let after = load_one(schema, target, args)?;

    let mut options = cim_rs::DiffOptions::default();
    if args.profile.is_some() {
        options = options.profiles(args.profile_id(schema)?.mask());
    }
    let change = before.difference_to(&after, &options);
    let _ = writeln!(
        io::stderr(),
        "{} added, {} removed, {} changed — {} statements to retract, {} to assert",
        change.added,
        change.removed,
        change.changed,
        change.model.reverse.len(),
        change.model.forward.len()
    );

    match &args.out {
        Some(path) => {
            let file =
                std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            cim_rs::writer::write_difference(
                schema,
                &change.model,
                io::BufWriter::new(file),
                &Default::default(),
            )
            .map_err(|e| format!("{}: {e}", path.display()))?;
            writeln!(out, "{}", path.display())?;
        }
        // No `--out`: the change set is the output, so it goes to stdout and the summary
        // above went to stderr, which is what makes `cim diff a b > change.xml` work.
        None => {
            cim_rs::writer::write_difference(schema, &change.model, &mut *out, &Default::default())
                .map_err(|e| e.to_string())?
        }
    }
    // With `--out` the document is a file and stdout is free to carry the report; without it
    // the document *is* stdout, and appending anything to it makes `cim diff a b >
    // change.xml` produce a file no XML parser accepts.
    if !change.report.is_empty() {
        let label = "not expressible as statements";
        match &args.out {
            Some(_) => print_report(out, &change.report, args.limit, label)?,
            None => {
                let mut err = io::stderr();
                let _ = print_report(&mut err, &change.report, args.limit, label);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

/// A command either failed with something to say, or was told to stop writing.
enum Fail {
    Message(String),
    Pipe,
}

impl From<String> for Fail {
    fn from(m: String) -> Fail {
        Fail::Message(m)
    }
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Fail {
        match e.kind() {
            io::ErrorKind::BrokenPipe => Fail::Pipe,
            _ => Fail::Message(e.to_string()),
        }
    }
}

fn no_vintage() -> String {
    "this build has no CIM vintage compiled in; rebuild with --features cgmes3".to_owned()
}

fn load_one(schema: &'static Schema, input: &Path, args: &Args) -> Result<Dataset, String> {
    let files = cim_rs::instance_files(input);
    if files.is_empty() {
        return Err(format!("no CIM inputs under {}", input.display()));
    }
    let mut ds = Dataset::new(schema);
    ds.load_files(&files, &args.read_options())
        .map_err(|e| format!("{}: {e}", input.display()))?;
    Ok(ds)
}

fn print_report(
    out: &mut dyn Write,
    report: &cim_rs::Report,
    limit: usize,
    label: &str,
) -> Result<(), Fail> {
    writeln!(out, "\n== {label} ==")?;
    if report.is_empty() {
        writeln!(out, "  none")?;
        return Ok(());
    }
    for (rule, n) in report.summary() {
        writeln!(out, "  {rule}  {n:>7}")?;
    }
    writeln!(out)?;
    let shown = if limit == 0 { report.len() } else { limit };
    for d in report.iter().take(shown) {
        writeln!(out, "  {d}")?;
    }
    if report.len() > shown {
        writeln!(out, "  … {} more (--limit 0 for all)", report.len() - shown)?;
    }
    Ok(())
}

fn exit_on_errors(report: &cim_rs::Report) -> ExitCode {
    if report.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Default)]
struct Args {
    command: Option<String>,
    inputs: Vec<PathBuf>,
    out: Option<PathBuf>,
    vintage: Option<String>,
    profile: Option<String>,
    rule: Option<String>,
    ntriples: bool,
    merged: bool,
    strict: bool,
    quiet: bool,
    help: bool,
    limit: usize,
}

impl Args {
    fn parse(raw: impl Iterator<Item = String>) -> Result<Args, String> {
        fn value(raw: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
            raw.next().ok_or_else(|| format!("{flag} needs a value"))
        }

        let mut args = Args {
            limit: 20,
            ..Default::default()
        };
        let mut raw = raw;
        while let Some(a) = raw.next() {
            match a.as_str() {
                "-h" | "--help" => args.help = true,
                "-q" | "--quiet" => args.quiet = true,
                "--strict" => args.strict = true,
                "--ntriples" => args.ntriples = true,
                "--merged" => args.merged = true,
                "-o" | "--out" => args.out = Some(PathBuf::from(value(&mut raw, "--out")?)),
                "--vintage" => args.vintage = Some(value(&mut raw, "--vintage")?),
                "--profile" => args.profile = Some(value(&mut raw, "--profile")?),
                "--rule" => args.rule = Some(value(&mut raw, "--rule")?),
                "--limit" => {
                    let v = value(&mut raw, "--limit")?;
                    args.limit = v
                        .parse()
                        .map_err(|_| format!("--limit takes a number, not {v:?}"))?;
                }
                // Any unrecognised flag, not only a long one: a mistyped `-Q` falling
                // through to the arm below would be taken for an input path, where
                // `instance_files` finds nothing and says nothing, and the command would
                // run with the flag silently ignored. A bare `-` stays available for a
                // future "read stdin".
                other if other.starts_with('-') && other != "-" => {
                    return Err(format!("unknown option {other:?}"));
                }
                _ if args.command.is_none() => args.command = Some(a),
                _ => args.inputs.push(PathBuf::from(a)),
            }
        }
        Ok(args)
    }

    /// Which schema to read against: what `--vintage` says, or what the files say.
    ///
    /// Detection is the default because the vintage is written in the document. Guessing it
    /// from feature order gets a CGMES 2.4.15 model read against the CGMES 3.0 tables, where
    /// no class resolves and the result is an empty model — which looks like a clean read.
    fn schema(&self) -> Result<&'static Schema, String> {
        if let Some(key) = &self.vintage {
            return VINTAGES
                .iter()
                .find(|s| s.vintage.eq_ignore_ascii_case(key))
                .copied()
                .ok_or_else(|| {
                    let known: Vec<&str> = VINTAGES.iter().map(|s| s.vintage).collect();
                    format!("no vintage {key:?} in this build; it has {known:?}")
                });
        }
        let first = self
            .inputs
            .iter()
            .flat_map(|p| cim_rs::instance_files(p))
            .next();
        if let Some(path) = first
            && let Some(schema) = cim_rs::load::detect_file(&path)
        {
            return Ok(schema);
        }
        VINTAGES.first().copied().ok_or_else(no_vintage)
    }

    fn profile_id(&self, schema: &'static Schema) -> Result<ProfileId, String> {
        let key = self.profile.as_deref().unwrap_or_default();
        schema.profile_by_keyword(key).ok_or_else(|| {
            let known: Vec<&str> = schema.profiles.iter().map(|p| p.keyword).collect();
            format!("no profile {key:?} in {}; it has {known:?}", schema.vintage)
        })
    }

    fn read_options(&self) -> ReadOptions {
        if self.strict {
            ReadOptions::strict()
        } else {
            ReadOptions::lenient()
        }
    }

    fn out_dir(&self) -> Result<PathBuf, String> {
        self.out
            .clone()
            .ok_or_else(|| "this command needs --out DIR".to_owned())
    }

    /// Load every input into one dataset, which is what makes a multi-profile model set a
    /// model rather than a pile of documents.
    fn load(&self, schema: &'static Schema) -> Result<(Dataset, cim_rs::Report), String> {
        if self.inputs.is_empty() {
            return Err("no inputs given; `cim --help` shows the usage".to_owned());
        }
        let files: Vec<PathBuf> = self
            .inputs
            .iter()
            .flat_map(|p| cim_rs::instance_files(p))
            .collect();
        if files.is_empty() {
            return Err("no CIM/XML or zip inputs found".to_owned());
        }
        let mut ds = Dataset::new(schema);
        let report = ds
            .load_files(&files, &self.read_options())
            .map_err(|e| e.to_string())?;
        Ok((ds, report.report))
    }
}
