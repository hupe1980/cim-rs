//! The prose, held to the same standard as the code.
//!
//! Every Rust example in the README and on the documentation site is already compiled as a
//! doctest, which is what keeps the *code* in the documentation honest. Nothing kept the
//! documentation's own cross-references honest, and a long design document is exactly where
//! they rot: `CONCEPT.md` linked `[7.1](#71-the-registry-name-is-not-the-import-name)` at a
//! section that had been renamed to "The name", and rendered as a live link that goes
//! nowhere.
//!
//! `zola build` already fails on a broken link between site pages. These two files are not
//! site pages — one is the crate's front page and the other is repository machinery — so
//! they need their own check.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// GitHub's heading slug: lower-cased, punctuation dropped, spaces to hyphens.
///
/// An em dash is punctuation, so `A — B` yields `a--b`: it is dropped and the two spaces
/// around it each become a hyphen. Getting that wrong would make this test report
/// working links as broken, which is worse than not having it.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn headings(text: &str) -> Vec<String> {
    let mut in_fence = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim();
            if !title.is_empty() {
                out.push(slug(title));
            }
        }
    }
    out
}

/// Every `](#anchor)` in `text`, with the line it sits on.
///
/// Inline code spans are removed first. A link written inside backticks is not a link — it
/// is prose *about* a link, which is exactly what a section explaining a broken one
/// contains, and counting it would make this test unable to describe its own subject.
/// Fenced blocks are skipped for the same reason.
fn anchor_links(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (n, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let plain = strip_code_spans(line);
        let mut rest = plain.as_str();
        while let Some(i) = rest.find("](#") {
            rest = &rest[i + 3..];
            let Some(end) = rest.find(')') else { break };
            out.push((n + 1, rest[..end].to_owned()));
            rest = &rest[end..];
        }
    }
    out
}

/// Drop every `` `…` `` span, keeping the rest of the line.
fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut inside = false;
    for c in line.chars() {
        match c {
            '`' => inside = !inside,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn check(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        // `CONCEPT.md` is `exclude`d from the published tarball, so the copy of this test
        // that `cargo package --verify` runs has no file to read. Skipping is right: the
        // property is about the repository, and the repository's own CI does check it.
        eprintln!("skipping: {} is not present", path.display());
        return;
    };
    let headings = headings(&text);
    let broken: Vec<String> = anchor_links(&text)
        .into_iter()
        .filter(|(_, a)| !headings.contains(a))
        .map(|(line, a)| format!("{}:{line} -> #{a}", path.display()))
        .collect();
    assert!(
        broken.is_empty(),
        "{} link(s) point at no heading:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
}

#[test]
fn every_internal_anchor_resolves_to_a_heading() {
    check(&root().join("README.md"));
    check(&root().join("CONCEPT.md"));
}

/// The slug rule this test depends on, spelled out — a wrong one gives false failures.
#[test]
fn headings_slugify_the_way_github_does() {
    assert_eq!(slug("4.6 Dataset layer"), "46-dataset-layer");
    assert_eq!(
        slug("4.2 Mapping CIM onto Rust — the decision that shaped everything"),
        "42-mapping-cim-onto-rust--the-decision-that-shaped-everything"
    );
    assert_eq!(
        slug("7. Single-Crate Strategy & Features"),
        "7-single-crate-strategy--features"
    );
    assert_eq!(slug("What is verified today"), "what-is-verified-today");
    // A `#` inside a fenced block is not a heading.
    assert_eq!(headings("```\n# not a heading\n```\n## real\n"), ["real"]);
}

/// A link is what renders as a link, which excludes one quoted inside backticks.
#[test]
fn a_link_written_as_code_is_prose_rather_than_a_link() {
    assert_eq!(anchor_links("see [x](#real).")[0].1, "real");
    assert!(anchor_links("it linked `[7.1](#gone)` at a renamed section").is_empty());
    assert!(anchor_links("```\n[x](#in-a-fence)\n```").is_empty());
    // Both on one line: the live one is found and the quoted one is not.
    let both = anchor_links("`[a](#quoted)` and [b](#live)");
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].1, "live");
}
