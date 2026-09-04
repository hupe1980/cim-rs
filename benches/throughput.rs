//! Read, write and validation throughput.
//!
//! Measured on synthetic models rather than the ENTSO-E corpus so the numbers are
//! reproducible on any machine and CI does not need the standards artifacts. When the
//! corpus *is* present, the largest published model is measured too — that is the number
//! worth quoting, since synthetic data cannot reproduce real attribute distributions.
//!
//! Run with `cargo bench -p cim-rs`.

use std::hint::black_box;
use std::time::Instant;

use cim_rs::cgmes3::SCHEMA;
use cim_rs::prelude::*;

const NS: &str = "http://iec.ch/TC57/CIM100#";

/// Build an instance document of `n` interconnected objects.
fn synthetic_model(n: usize) -> String {
    let mut s = String::with_capacity(n * 400);
    s.push_str(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:eu="http://iec.ch/TC57/CIM100-European#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
         xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.scenarioTime>2021-03-25T15:30:00Z</md:Model.scenarioTime>
    <md:Model.created>2021-03-25T23:16:27Z</md:Model.created>
    <md:Model.version>001</md:Model.version>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0</md:Model.profile>
  </md:FullModel>
"##,
    );
    for i in 0..n {
        let line = uuid(i as u64, 1);
        let t1 = uuid(i as u64, 2);
        let t2 = uuid(i as u64, 3);
        s.push_str(&format!(
            r##"  <cim:ACLineSegment rdf:ID="_{line}">
    <cim:IdentifiedObject.name>Line {i}</cim:IdentifiedObject.name>
    <cim:IdentifiedObject.mRID>{line}</cim:IdentifiedObject.mRID>
    <cim:ACLineSegment.r>{}</cim:ACLineSegment.r>
    <cim:ACLineSegment.x>{}</cim:ACLineSegment.x>
    <cim:ACLineSegment.bch>0.0000648739</cim:ACLineSegment.bch>
    <cim:Conductor.length>{}</cim:Conductor.length>
    <cim:Equipment.aggregate>false</cim:Equipment.aggregate>
  </cim:ACLineSegment>
  <cim:Terminal rdf:ID="_{t1}">
    <cim:IdentifiedObject.name>T{i}a</cim:IdentifiedObject.name>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
    <cim:Terminal.ConductingEquipment rdf:resource="#_{line}"/>
    <cim:Terminal.phases rdf:resource="{NS}PhaseCode.ABC"/>
  </cim:Terminal>
  <cim:Terminal rdf:ID="_{t2}">
    <cim:IdentifiedObject.name>T{i}b</cim:IdentifiedObject.name>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
    <cim:Terminal.ConductingEquipment rdf:resource="#_{line}"/>
    <cim:Terminal.phases rdf:resource="{NS}PhaseCode.ABC"/>
  </cim:Terminal>
"##,
            0.42 + i as f64 * 0.001,
            6.3 + i as f64 * 0.01,
            1000.0 + i as f64,
        ));
    }
    s.push_str("</rdf:RDF>\n");
    s
}

/// A deterministic well-formed UUID from two counters.
fn uuid(a: u64, b: u64) -> String {
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        a,
        b,
        a & 0xfff,
        b & 0xfff,
        a
    )
}

struct Measurement {
    label: &'static str,
    seconds: f64,
    bytes: u64,
    items: usize,
}

impl Measurement {
    fn report(&self) {
        let mib = self.bytes as f64 / (1u64 << 20) as f64;
        println!(
            "{:<22} {:>8.3}s  {:>8.1} MiB/s  {:>10} items  {:>10.0} items/s",
            self.label,
            self.seconds,
            mib / self.seconds,
            self.items,
            self.items as f64 / self.seconds
        );
    }
}

/// Run `f` repeatedly for at least `min_seconds` and report the best rate seen.
fn measure(label: &'static str, bytes: u64, items: usize, mut f: impl FnMut()) -> Measurement {
    // A short warm-up so the first run's allocation growth is not measured.
    f();
    let mut best = f64::INFINITY;
    let overall = Instant::now();
    let mut runs = 0;
    while overall.elapsed().as_secs_f64() < 1.5 || runs < 3 {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
        runs += 1;
    }
    Measurement {
        label,
        seconds: best,
        bytes,
        items,
    }
}

fn bench_synthetic() {
    const OBJECTS: usize = 20_000;
    let doc = synthetic_model(OBJECTS / 3);
    let bytes = doc.len() as u64;
    println!(
        "\n== synthetic model: {:.1} MiB, {} objects ==",
        bytes as f64 / (1u64 << 20) as f64,
        OBJECTS
    );

    let mut objects = 0;
    measure("read", bytes, OBJECTS, || {
        let mut ds = Dataset::new(SCHEMA);
        cim_rs::reader::read_into(&mut ds, doc.as_bytes(), None, &ReadOptions::lenient()).unwrap();
        objects = ds.len();
        black_box(&ds);
    })
    .report();

    let mut ds = Dataset::new(SCHEMA);
    cim_rs::reader::read_into(&mut ds, doc.as_bytes(), None, &ReadOptions::lenient()).unwrap();

    let mut out_bytes = 0u64;
    measure("write", bytes, ds.len(), || {
        let mut buf = Vec::with_capacity(bytes as usize);
        cim_rs::writer::write(&ds, &mut buf, &WriteOptions::default()).unwrap();
        out_bytes = buf.len() as u64;
        black_box(&buf);
    })
    .report();

    measure("validate", bytes, ds.len(), || {
        black_box(cim_rs::validate::validate(&ds));
    })
    .report();

    measure("inverse index", bytes, ds.len(), || {
        black_box(cim_rs::InverseIndex::build(&ds));
    })
    .report();

    // Traversal cost: the operation model code actually spends its time in.
    measure("typed traversal", bytes, ds.len(), || {
        let mut sum = 0.0;
        for line in ds.view::<cim_rs::cgmes3::views::ACLineSegment>() {
            sum += line.r().unwrap_or(0.0) + line.x().unwrap_or(0.0);
        }
        black_box(sum);
    })
    .report();
}

/// The largest published model, when the corpus has been fetched.
fn bench_corpus() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("specs").is_dir());
    let Some(workspace) = workspace else {
        println!("\n== published corpus not present; run `cargo xtask fetch-specs` ==");
        return;
    };
    let root = workspace
        .join("specs/test-models/cas-3.0.3")
        .join("CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3/v3.0/RealGrid");
    // A corpus that is *present* but does not hold the model this benchmark names is a
    // different thing from a corpus that is absent, and only the first is a skip. The
    // second means a path drifted — the configuration directory carries its CAS version,
    // and those are renamed between releases — and the honest outcome is a failure, not a
    // line of output nobody reads. Without this the CI job that fetches the corpus would
    // go on passing while measuring nothing, and the throughput figures in the README
    // would quietly stop being measured at all.
    assert!(
        root.is_dir(),
        "the standards corpus is present but RealGrid is not at {}.\n\
         A conformity configuration was renamed, or `cargo xtask fetch-specs` changed \
         what it lays down; update the path here so the throughput numbers keep being \
         measured.",
        root.display()
    );

    // The instance files sit in a subdirectory of the configuration.
    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
                out.push(p);
            }
        }
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "RealGrid is at {} but holds no instance files",
        root.display()
    );
    let bytes: u64 = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();

    // Read the files into memory first: the number should measure parsing, not the disk.
    let contents: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some((name, std::fs::read(p).ok()?))
        })
        .collect();

    println!(
        "\n== RealGrid: {:.1} MiB, {} files ==",
        bytes as f64 / (1u64 << 20) as f64,
        files.len()
    );

    let parse = || {
        let mut ds = Dataset::new(SCHEMA);
        for (name, body) in &contents {
            cim_rs::reader::read_into(
                &mut ds,
                body.as_slice(),
                Some(name),
                &ReadOptions::lenient(),
            )
            .unwrap();
        }
        ds
    };
    let mut best = f64::MAX;
    let mut ds = Dataset::new(SCHEMA);
    for _ in 0..3 {
        let t = Instant::now();
        ds = parse();
        best = best.min(t.elapsed().as_secs_f64());
    }
    ds.shrink_to_fit();
    Measurement {
        label: "read",
        seconds: best,
        bytes,
        items: ds.len(),
    }
    .report();

    // Write the model back as the file set it was read from, into memory, so the number
    // measures serialization rather than the disk.
    measure("write (file set)", bytes, ds.len(), || {
        let mut total = 0usize;
        for (i, header) in ds.headers().iter().enumerate() {
            let profiles = header
                .profiles
                .iter()
                .filter_map(|iri| SCHEMA.profile_by_iri(iri))
                .fold(0, |acc, p| acc | p.mask());
            if profiles == 0 {
                continue;
            }
            let options = WriteOptions {
                profiles,
                header: cim_rs::writer::HeaderSource::Given(Box::new(header.clone())),
                ..Default::default()
            };
            let mut buf = Vec::new();
            cim_rs::writer::write_objects(&ds, ds.objects_from(i), &mut buf, &options).unwrap();
            total += buf.len();
        }
        black_box(total);
    })
    .report();

    // The other output the crate offers: the same model as a typed RDF graph, which is
    // what a SHACL engine or a triple store consumes.
    measure("rdf (n-triples)", bytes, ds.len(), || {
        let mut buf = Vec::new();
        cim_rs::rdf::write(
            &ds,
            &mut buf,
            &cim_rs::rdf::RdfOptions::new(cim_rs::rdf::Syntax::NTriples),
        )
        .unwrap();
        black_box(buf.len());
    })
    .report();

    measure("validate", bytes, ds.len(), || {
        black_box(cim_rs::validate::validate(&ds));
    })
    .report();

    measure("inverse index", bytes, ds.len(), || {
        black_box(cim_rs::InverseIndex::build(&ds));
    })
    .report();

    // Computing a change set walks both models. Diffing a model against itself is the
    // honest worst case for the *comparison* — every object is visited and compared — and
    // the honest best case for the output, which is what an incremental exchange between
    // two nearly identical states looks like in practice.
    measure("diff (no change)", bytes, ds.len(), || {
        let d = ds.difference_to(&ds, &cim_rs::diff::DiffOptions::default());
        black_box(d.model.forward.len() + d.model.reverse.len());
    })
    .report();
}

fn main() {
    println!("cim throughput — best of repeated runs, release build");
    bench_synthetic();
    bench_corpus();
}
