//! Locating the ENTSO-E conformity test corpus.
//!
//! The models are fetched by `scripts/fetch-specs.sh` into the gitignored `specs/`
//! directory. They are licensed CC BY-SA 4.0 and owned by ENTSO-E, so they serve as a
//! local test corpus only and are never redistributed with this crate. Tests that need
//! them skip when the corpus is absent, keeping `cargo test` green on a fresh clone.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

pub fn specs() -> Option<PathBuf> {
    let p = workspace_root().join("specs");
    p.is_dir().then_some(p)
}

/// Root of the CGMES 3.0 conformity assessment configurations.
pub fn cgmes3_models() -> Option<PathBuf> {
    let p = specs()?
        .join("test-models/cas-3.0.3")
        .join("CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3/v3.0");
    p.is_dir().then_some(p)
}

/// A named model directory inside the CGMES 3.0 corpus.
pub fn cgmes3_model(rel: &str) -> Option<PathBuf> {
    let p = cgmes3_models()?.join(rel);
    p.is_dir().then_some(p)
}

/// Every `.xml` instance file under `dir`, sorted for reproducibility.
pub fn xml_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
            out.push(p);
        }
    }
}

/// Skip a test with an explanatory note when the corpus is unavailable.
#[macro_export]
macro_rules! require_corpus {
    ($e:expr) => {
        match $e {
            Some(v) => v,
            None => {
                eprintln!(
                    "skipping: ENTSO-E conformity models not found; run scripts/fetch-specs.sh"
                );
                return;
            }
        }
    };
}
