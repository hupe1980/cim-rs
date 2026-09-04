//! Fetching the machine-readable CIM/CGMES standards artifacts into `specs/`.
//!
//! Only publicly licensed material is fetched:
//!
//! * UCAIug/ENTSO-E RDFS + SHACL + PROF artifacts — Apache-2.0
//! * ENTSO-E conformity test configurations — CC BY-SA 4.0 (attribution required, and
//!   never redistributed inside the published crate)
//! * public ENTSO-E technical documents (PDF)
//!
//! IEC standard *documents* (61970-301/-552/-600, …) are paywalled and are deliberately
//! not downloaded. Nothing in the build or test pipeline may depend on them.
//!
//! # Why this is not a shell script any more
//!
//! It was one, and the shell could not do the last thing this does. The list of archives
//! to download and the list of RDFS files [`crate::vintage`] feeds to the generator were
//! two independent lists naming the same directories, so renaming a vintage's `rdfs_dir`
//! left the fetch step quietly downloading into the wrong place — a failure that only
//! showed up later as "missing …, run scripts/fetch-specs.sh", which one had just done.
//! Here the fetch step ends by asking the vintage table whether every file it names
//! arrived, so the two lists cannot disagree without saying so.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::vintage;

/// Pinned upstream ref for the ENTSO-E application profiles library.
const APL_REPO: &str = "https://github.com/entsoe/application-profiles-library";
const APL_TAG: &str = "v1.1.1";

/// What to do with a downloaded artifact.
enum Kind {
    /// A zip archive, expanded into `dest`.
    Zip,
    /// A single file, saved as `dest`.
    File,
}

struct Artifact {
    url: &'static str,
    /// Destination under `specs/` — a directory for [`Kind::Zip`], a file path otherwise.
    dest: &'static str,
    kind: Kind,
    /// What the material is licensed under, recorded in the manifest.
    license: &'static str,
}

const ARTIFACTS: &[Artifact] = &[
    // CGMES 2.4.15 schema packages (RDFS 04Jul2016 + the 2020 component refresh).
    Artifact {
        url: "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/ENTSOE_CGMES_v2.4.15_04Jul2016_RDFS.zip",
        dest: "cgmes-2.4.15/rdfs-2016",
        kind: Kind::Zip,
        license: "Apache-2.0 (UCAIug/ENTSO-E)",
    },
    Artifact {
        url: "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/CGMES2415_Components_2020.zip",
        dest: "cgmes-2.4.15/components-2020",
        kind: Kind::Zip,
        license: "Apache-2.0 (UCAIug/ENTSO-E)",
    },
    // Conformity assessment test configurations. CC BY-SA 4.0: a local test corpus only,
    // never redistributed with the crate.
    Artifact {
        url: "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3.zip",
        dest: "test-models/cas-3.0.3",
        kind: Kind::Zip,
        license: "CC BY-SA 4.0 (ENTSO-E)",
    },
    Artifact {
        url: "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/TestConfigurations_packageCASv2.0.zip",
        dest: "test-models/cas-2.0",
        kind: Kind::Zip,
        license: "CC BY-SA 4.0 (ENTSO-E)",
    },
    Artifact {
        url: "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/QoCDC_v3.2.1_test_models.zip",
        dest: "test-models/qocdc-3.2.1",
        kind: Kind::Zip,
        license: "CC BY-SA 4.0 (ENTSO-E)",
    },
    // Public ENTSO-E technical documents.
    Artifact {
        // v1.1.0, which is the revision the documentation cites: a reader following a
        // citation has to land on the document in `specs/`.
        url: "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/RDF-SyntaxUserGuide_v_1-1-0.pdf",
        dest: "docs/RDF-SyntaxUserGuide_v1-1-0.pdf",
        kind: Kind::File,
        license: "public ENTSO-E document",
    },
    Artifact {
        url: "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/00_ApplicationProfilesReadMe.pdf",
        dest: "docs/ApplicationProfilesReadMe.pdf",
        kind: Kind::File,
        license: "public ENTSO-E document",
    },
    Artifact {
        url: "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/140807_ENTSOE_CGMES_v2.4.15.pdf",
        dest: "docs/CGMES_v2.4.15_TechnicalSpecification.pdf",
        kind: Kind::File,
        license: "public ENTSO-E document",
    },
    Artifact {
        url: "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/IOP/CGMES_2_5_TechnicalSpecification_61970-600_Part%201_Ed2.pdf",
        dest: "docs/CGMES_2.5_TechnicalSpecification_61970-600_Part1_Ed2.pdf",
        kind: Kind::File,
        license: "public ENTSO-E document",
    },
    Artifact {
        url: "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/QandA_on_CIM_CGMES_based_data_exchange_implementation_v1-0-1.pdf",
        dest: "docs/QandA_CIM_CGMES_data_exchange_implementation_v1-0-1.pdf",
        kind: Kind::File,
        license: "public ENTSO-E document",
    },
];

pub fn fetch(root: &Path, clean: bool) -> Result<()> {
    let specs = root.join("specs");
    if clean {
        println!("==> removing {}", specs.display());
        let _ = fs::remove_dir_all(&specs);
    }
    let archives = specs.join("_archives");
    fs::create_dir_all(&archives)?;

    let mut failures: Vec<String> = Vec::new();

    // The profiles library is a git repository rather than an archive, and it is pinned:
    // regenerating against a moving upstream would make the committed sources depend on
    // when they were generated.
    if let Err(e) = clone_profiles_library(&specs) {
        eprintln!("!! {e:#}");
        failures.push(APL_REPO.to_owned());
    }

    for a in ARTIFACTS {
        if let Err(e) = fetch_one(a, &specs, &archives) {
            eprintln!("!! {e:#}");
            failures.push(a.url.to_owned());
        }
    }

    write_manifest(&specs).context("writing MANIFEST.txt")?;

    if !failures.is_empty() {
        bail!(
            "completed with {} failure(s):\n{}",
            failures.len(),
            failures
                .iter()
                .map(|u| format!("  - {u}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // The point of doing this in the same program as the generator: ask the vintage table
    // whether what it needs actually arrived, instead of finding out at codegen time.
    verify(&specs)?;
    println!("\nAll specs fetched into {}", specs.display());
    Ok(())
}

/// Check that every RDFS file the generator will ask for is present.
pub fn verify(specs: &Path) -> Result<()> {
    let mut missing: Vec<String> = Vec::new();
    for v in vintage::VINTAGES {
        let dir = specs.join(v.rdfs_dir);
        for p in v.profiles {
            let path = dir.join(p.file);
            if !path.is_file() {
                missing.push(format!("{} [{}] {}", v.key, p.keyword, path.display()));
            }
        }
    }
    if !missing.is_empty() {
        bail!(
            "fetched, but {} vocabulary file(s) the generator needs are absent — the \
             artifact list and the vintage table disagree:\n{}",
            missing.len(),
            missing
                .iter()
                .map(|m| format!("  - {m}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    println!(
        "==> verified   {} vintage(s), every vocabulary file present",
        vintage::VINTAGES.len()
    );
    Ok(())
}

fn clone_profiles_library(specs: &Path) -> Result<()> {
    let dest = specs.join("application-profiles-library");
    if dest.join(".git").is_dir() {
        println!("==> cached     application-profiles-library");
        return Ok(());
    }
    println!("==> cloning    application-profiles-library @ {APL_TAG}");
    fs::create_dir_all(specs)?;
    let ok = git(&[
        "clone",
        "--depth",
        "1",
        "--branch",
        APL_TAG,
        APL_REPO,
        &dest.to_string_lossy(),
    ])?;
    if !ok {
        bail!("git clone of {APL_REPO} @ {APL_TAG} failed");
    }
    Ok(())
}

fn git(args: &[&str]) -> Result<bool> {
    let status = std::process::Command::new("git")
        .args(args)
        .status()
        .context("running git — is it installed and on PATH?")?;
    Ok(status.success())
}

fn fetch_one(a: &Artifact, specs: &Path, archives: &Path) -> Result<()> {
    match a.kind {
        Kind::File => {
            let dest = specs.join(a.dest);
            if is_populated(&dest) {
                println!("==> cached     {}", a.dest);
                return Ok(());
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            download(a.url, &dest)?;
        }
        Kind::Zip => {
            let name = archive_name(a.url);
            let zip = archives.join(&name);
            if !is_populated(&zip) {
                download(a.url, &zip)?;
            } else {
                println!("==> cached     {name}");
            }
            let dest = specs.join(a.dest);
            if !dest.is_dir() {
                fs::create_dir_all(&dest)?;
                unzip(&zip, &dest).with_context(|| format!("extracting {name}"))?;
            }
        }
    }
    Ok(())
}

/// The archive's file name, undoing the percent-encoding a URL may carry.
fn archive_name(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("download.zip");
    last.replace("%20", " ")
}

fn is_populated(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Download to a `.part` file and rename, so an interrupted run never leaves a truncated
/// artifact that the next run would treat as cached.
fn download(url: &str, dest: &Path) -> Result<()> {
    println!(
        "==> downloading {}",
        dest.file_name().unwrap_or_default().to_string_lossy()
    );
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().unwrap_or_default().to_string_lossy()
    ));

    let mut response = ureq::get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let mut reader = response
        .body_mut()
        .with_config()
        // These are hundred-megabyte archives; the default response cap is far smaller.
        .limit(4 * 1024 * 1024 * 1024)
        .reader();

    let mut file = fs::File::create(&part)?;
    std::io::copy(&mut reader, &mut file).with_context(|| format!("saving {url}"))?;
    drop(file);
    fs::rename(&part, dest)?;
    Ok(())
}

fn unzip(zip: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(zip)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    // `extract` refuses entries whose path escapes the destination, which matters because
    // these archives are third-party: their entry names are chosen by whoever built them.
    archive.extract(dest)?;
    Ok(())
}

/// Provenance and checksums of everything fetched.
fn write_manifest(specs: &Path) -> Result<()> {
    let mut lines = vec![
        "# Generated by `cargo xtask fetch-specs`.".to_owned(),
        "# Checksums of the downloaded artifacts, with the licence each is published under."
            .to_owned(),
    ];

    if let Some(head) = git_head(&specs.join("application-profiles-library")) {
        lines.push(format!(
            "# application-profiles-library ref: {head} ({APL_TAG})"
        ));
    }
    lines.push(String::new());

    let mut rows: Vec<String> = Vec::new();
    for a in ARTIFACTS {
        let path = match a.kind {
            Kind::Zip => specs.join("_archives").join(archive_name(a.url)),
            Kind::File => specs.join(a.dest),
        };
        let Some(digest) = sha256(&path) else {
            continue;
        };
        rows.push(format!(
            "{digest}  {}  [{}]",
            path.strip_prefix(specs).unwrap_or(&path).display(),
            a.license
        ));
    }
    rows.sort();
    lines.extend(rows);

    fs::write(specs.join("MANIFEST.txt"), lines.join("\n") + "\n")?;
    Ok(())
}

fn git_head(repo: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn sha256(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}
