//! The command line, run as a command line.
//!
//! `cim` is a shell over the public API, so most of what it does is covered by the library's
//! own tests. What is not is the part that *is* the tool rather than the library: which
//! stream each thing it writes goes to, and what it leaves on disk.
//!
//! `cim diff a b > change.xml` means the document is stdout, so anything else the command
//! has to say must go elsewhere or the file stops being a document.

#![cfg(all(feature = "cli", feature = "cgmes3"))]

mod common;

use std::path::Path;
use std::process::Command;

const NS: &str = "http://iec.ch/TC57/CIM100#";
const LOCATION: &str = "22222222-2222-4222-8222-222222222222";

fn cim() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cim"))
}

/// A scratch directory named for the test using it, removed and recreated on entry.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cim-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A one-object Geographical Location file whose address is a compound.
///
/// A compound is the one difference IEC 61970-552's statement syntax cannot carry, so
/// changing one is the shortest way to make `cim diff` have something to report. The
/// published conformity corpus contains no compound at all, which is why this is written
/// here rather than found there.
fn location_file(dir: &Path, postal_code: &str) {
    let text = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="{NS}"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#">
  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">
    <md:Model.created>2024-01-01T00:00:00Z</md:Model.created>
    <md:Model.scenarioTime>2024-01-01T00:00:00Z</md:Model.scenarioTime>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/GeographicalLocation-EU/3.0</md:Model.profile>
  </md:FullModel>
  <cim:Location rdf:ID="_{LOCATION}">
    <cim:IdentifiedObject.mRID>{LOCATION}</cim:IdentifiedObject.mRID>
    <cim:Location.mainAddress rdf:parseType="Resource">
      <cim:StreetAddress.postalCode>{postal_code}</cim:StreetAddress.postalCode>
    </cim:Location.mainAddress>
  </cim:Location>
</rdf:RDF>
"##
    );
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("GL.xml"), text).unwrap();
}

/// With no `--out`, the change set *is* standard output — so nothing else may go there.
#[test]
fn a_change_set_written_to_stdout_is_a_document_and_nothing_else() {
    let root = scratch("diff-stdout");
    location_file(&root.join("base"), "1000");
    location_file(&root.join("target"), "2000");

    let out = cim()
        .arg("diff")
        .arg(root.join("base"))
        .arg(root.join("target"))
        .output()
        .unwrap();
    assert!(out.status.success(), "cim diff failed: {out:?}");

    let document = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();

    // The report exists — otherwise this test proves nothing — and it is on stderr.
    assert!(
        stderr.contains("CIM0015") && stderr.contains("compound"),
        "the compound change was not reported at all:\nstderr: {stderr}"
    );
    assert!(
        !document.contains("CIM0015"),
        "the diagnostics report was written into the document:\n{document}"
    );

    // And the document is a document, judged by the checker the corpus sweep uses rather
    // than by this crate's own tolerant reader.
    common::assert_well_formed("cim diff > change.xml", &document);
    assert!(document.trim_end().ends_with("</rdf:RDF>"), "{document}");

    let _ = std::fs::remove_dir_all(&root);
}

/// With `--out`, the document is a file and stdout is free to carry the report.
#[test]
fn a_change_set_written_to_a_file_leaves_stdout_for_the_report() {
    let root = scratch("diff-file");
    location_file(&root.join("base"), "1000");
    location_file(&root.join("target"), "2000");
    let path = root.join("change_DIFF.xml");

    let out = cim()
        .arg("diff")
        .arg(root.join("base"))
        .arg(root.join("target"))
        .arg("--out")
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "cim diff --out failed: {out:?}");

    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("CIM0015"), "{stdout}");
    common::assert_well_formed("change_DIFF.xml", &std::fs::read_to_string(&path).unwrap());

    let _ = std::fs::remove_dir_all(&root);
}

/// `cim rdf` writes a graph for a profile the model describes, and none for one it does not.
///
/// A graph of nothing but the loaded files' headers is not an export of a profile: it looks
/// like one, it validates like one, and it says nothing about the model. Writing no file is
/// what lets a caller — `cargo xtask shacl` among them — tell "conformant" from "absent".
#[test]
fn rdf_writes_a_graph_only_for_a_profile_the_model_describes() {
    let root = scratch("rdf-empty");
    location_file(&root.join("model"), "1000");
    let graphs = root.join("graphs");

    let out = cim()
        .arg("rdf")
        .arg(root.join("model"))
        .arg("--out")
        .arg(&graphs)
        .arg("--ntriples")
        .output()
        .unwrap();
    assert!(out.status.success(), "cim rdf failed: {out:?}");

    assert!(graphs.join("GL.nt").is_file(), "the GL graph is missing");
    for absent in ["EQ.nt", "SSH.nt", "SV.nt", "TP.nt", "DY.nt", "FH.nt"] {
        assert!(
            !graphs.join(absent).is_file(),
            "{absent} was written for a profile this model says nothing about"
        );
    }
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no data for"), "{stderr}");

    // And the graph that was written carries this file's header and no other profile's.
    let gl = std::fs::read_to_string(graphs.join("GL.nt")).unwrap();
    assert!(gl.contains("GeographicalLocation-EU"), "{gl}");
    assert!(common::check_ntriples(&gl).unwrap() > 3, "{gl}");

    let _ = std::fs::remove_dir_all(&root);
}

/// The vintage comes from the document, not from which feature happens to be first.
///
/// Only reachable through the binary: the library takes a `&'static Schema` and so cannot
/// choose one. Reading a CGMES 2.4.15 model against the CGMES 3.0 tables resolves nothing,
/// and the result — an empty model, exit status 0 — reads as a successful load.
#[test]
#[cfg(feature = "cgmes2")]
fn the_vintage_is_detected_from_the_input() {
    let root = scratch("vintage");
    let model = root.join("model");
    std::fs::create_dir_all(&model).unwrap();
    std::fs::write(
        model.join("EQ.xml"),
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            "\n",
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#""#,
            r#" xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#""#,
            r#" xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#">"#,
            "\n",
            r#"  <md:FullModel rdf:about="urn:uuid:11111111-1111-4111-8111-111111111111">"#,
            r#"<md:Model.profile>http://entsoe.eu/CIM/EquipmentCore/3/1</md:Model.profile>"#,
            r#"</md:FullModel>"#,
            "\n",
            r#"  <cim:Substation rdf:ID="_22222222-2222-4222-8222-222222222222">"#,
            r#"<cim:IdentifiedObject.name>S1</cim:IdentifiedObject.name></cim:Substation>"#,
            "\n</rdf:RDF>\n"
        ),
    )
    .unwrap();

    // No `--vintage`: the document is read as what it says it is.
    let out = cim().arg("info").arg(&model).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("objects              1"), "{stdout}");
    assert!(!stdout.contains("CIM0021"), "{stdout}");

    // Forced to the wrong one, it says so rather than reporting an empty model as fine.
    let out = cim()
        .arg("info")
        .arg(&model)
        .args(["--vintage", "cgmes3"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("CIM0021"),
        "no mismatch reported:\n{stdout}"
    );
    assert!(stdout.contains("objects              0"), "{stdout}");
    assert_eq!(
        out.status.code(),
        Some(1),
        "a model that read as nothing exited 0"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// An unknown flag is refused rather than taken for an input path.
#[test]
fn a_mistyped_flag_is_an_error_rather_than_a_silently_ignored_one() {
    let out = cim().arg("info").arg("-Q").arg(".").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown option"),
        "{out:?}"
    );
}
