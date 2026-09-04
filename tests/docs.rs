//! The prose, held to the same standard as the code.
//!
//! Every Rust example in the README and on the site is compiled as a doctest, which keeps
//! the *code* in the documentation honest. Cross-references need their own check: `zola
//! build` fails on a broken link between site pages, but the README and `CONTRIBUTING.md`
//! are not site pages, and a renamed section leaves a live link that goes nowhere.

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
        // `CONTRIBUTING.md` is `exclude`d from the published tarball, so the copy of this
        // test that `cargo package --verify` runs has no file to read. Skipping is right:
        // the property is about the repository, whose own CI does check it.
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

/// Every Rust snippet names the library `cim_rs`, which is what it is called.
///
/// The package is `cim-rs` and the library `cim_rs`; `cim` is a different crate on
/// crates.io. The README and the site pages are doctests, so they cannot get this wrong;
/// anything else is checked by nothing but this.
#[test]
fn documented_snippets_name_the_library_correctly() {
    for file in ["README.md", "CONTRIBUTING.md", "CHANGELOG.md"] {
        let Ok(text) = std::fs::read_to_string(root().join(file)) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            assert!(
                !line.contains("cim::"),
                "{file}:{}: the library is `cim_rs`, not `cim`: {line}",
                n + 1
            );
        }
    }
}

#[test]
fn every_internal_anchor_resolves_to_a_heading() {
    check(&root().join("README.md"));
    check(&root().join("CONTRIBUTING.md"));
    check(&root().join("CHANGELOG.md"));
}

/// The changelog's newest entry is the version being built.
///
/// Bumping the manifest and forgetting the changelog is the one mistake a changelog makes,
/// and the release workflow — which checks the *tag* against the manifest — would not
/// notice it. `CARGO_PKG_VERSION` is the manifest, so this compares the two documents that
/// are supposed to agree.
#[test]
fn the_changelog_leads_with_the_version_being_built() {
    let text = std::fs::read_to_string(root().join("CHANGELOG.md")).expect("CHANGELOG.md");
    let newest = text
        .lines()
        .find_map(|l| l.strip_prefix("## [")?.split(']').next())
        .expect("a `## [version]` heading");
    assert_eq!(
        newest,
        env!("CARGO_PKG_VERSION"),
        "the changelog leads with {newest} and the manifest says {}",
        env!("CARGO_PKG_VERSION")
    );
}

/// The rule catalogue on the site lists every rule the crate has.
///
/// A table of stable codes is exactly the documentation a pipeline author relies on and
/// exactly the kind that falls behind, since adding a `Rule` variant compiles fine without
/// it. `zola` cannot check this and neither can a doctest.
#[test]
fn the_rule_catalogue_lists_every_rule() {
    let page = root().join("site/content/docs/validation.md");
    let Ok(text) = std::fs::read_to_string(&page) else {
        eprintln!("skipping: {} is not present", page.display());
        return;
    };
    let missing: Vec<String> = cim_rs::Rule::ALL
        .iter()
        .filter(|r| !text.contains(&format!("`{}`", r.code())))
        .map(|r| format!("{} ({r:?})", r.code()))
        .collect();
    assert!(
        missing.is_empty(),
        "the rule catalogue does not list: {}",
        missing.join(", ")
    );
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
    // An emoji is dropped and the space after it is not, so the anchor keeps a leading
    // hyphen — which a link to such a heading has to spell. The README relies on this.
    assert_eq!(
        slug("⚖️ Licensing and attribution"),
        "-licensing-and-attribution"
    );
    assert_eq!(slug("📦 Install"), "-install");
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
