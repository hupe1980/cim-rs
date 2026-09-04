# Changelog

Notable changes to `cim-rs`. The format follows [Keep a Changelog][kac] and the versioning
is [SemVer][semver] — before 1.0 the **minor** version is the breaking one, so `0.1` → `0.2`
is where incompatible changes land.

[kac]: https://keepachangelog.com/en/1.1.0/
[semver]: https://semver.org/

## [0.2.0] — unreleased

Fidelity and the two halves of the tooling: a re-exported model is now the file that was
read down to how each number was written, a document written with default options is
conforming, and the command line can apply a change set as well as compute one.

### Added

- **`cim apply <input>… --change FILE --out DIR`** — the receiving half of IEC 61970-552's
  incremental exchange. Change sets are named rather than discovered among the inputs, are
  applied in the order given, and objects a change leaves empty are pruned.
- **`--assume-profile KEY`** names the profile of a file that declares none, so a document
  with no `md:FullModel` can still be exported as itself.
- **`--profile` is repeatable**: `--profile EQ --profile OP --profile SC` writes the single
  graph a CGMES 2.4.15 Equipment file contains, which is what its shapes constrain.
- **`CIM0022 UnreadableFile`** — a file of a model set that cannot be parsed at all is
  reported by name and the rest of the set is still read. `LoadReport::failed` lists them;
  `Strictness::Strict` still refuses.
- **`SaveReport::unwritten`** counts objects no output file covers, so a partly exported
  model is a number rather than an inference.
- `Real`, `HeaderSource`, `load::detect_vintage`, `writer::derive_header` and
  `Dataset::set_header` are public.
- A computed difference reports how many objects it could not delete — the statement syntax
  has no form for deletion, so an applied change set that does not reproduce its target now
  explains itself.

### Changed

Breaking, all of it:

- **`Value::Float` carries a `Real`** rather than an `f64`: the number *and* the spelling it
  was read with. Equality, hashing and `Dataset::content_id` use only the number, so a
  difference never reports a reformatting as a change, while a re-export reproduces
  `2.62637E-05` and `250.000000` instead of rendering them afresh. Build one with
  `Value::from(f64)`.
- **`WriteOptions::header` is a `HeaderSource`**, defaulting to `Derive`. A document written
  with default options now carries a conforming `md:FullModel` declaring the profiles
  written; `with_header` supplies one and `headerless` asks for none.
- **`writer::write_profile` and `write_profiles` lost their `header` parameter** — it
  duplicated `WriteOptions::header`.
- **`Rule` gained a variant** (`UnreadableFile`), and the enum, its codes and `Rule::ALL` are
  now generated from one declaration.
- **`instance_files` takes `impl AsRef<Path>`** rather than `&Path`.
- `LoadReport` and `SaveReport` gained fields.
- Vintage detection asks every input in turn rather than only the first, so a set whose
  first file is unreadable is still identified correctly.

### Fixed

- A single unreadable file no longer aborts the load of a whole model set.
- `cim export` no longer leaves an empty directory and exits 0 when the input declares no
  profile; it writes one file per profile and says why on stderr.
- Values are no longer reformatted on export — 8,623 numbers in the published CGMES 3.0
  corpus were.
- `validate` reports a compound field the compound's class does not declare; unlike an
  object's stray value, it is written rather than dropped.
- A header that appears after the objects it describes fills its file's slot instead of
  being dropped.

### Repository

- The generator takes namespace prefixes from the vocabularies' own `xmlns:` bindings, so a
  new profile set is named as its publisher names it.
- `cargo xtask shacl --vintage cgmes2` runs the older vintage. It validates nothing today:
  all ten CGMES 2.4.15 shapes files ENTSO-E publishes are invalid SHACL, and the unusable
  set is pinned so the day one is fixed is visible.
- `tests/qocdc.rs` sweeps ENTSO-E's quality-check corpus — 100 model sets of real TSO
  output. A weekly workflow runs all four archives and longer fuzzing.

## [0.1.0] — 2026-08-27

First release. CGMES 3.0 and CGMES 2.4.15 from the published RDFS vocabularies: sparse
object store with multi-profile merge, streaming CIM/XML reader and writer, profile-aware
validation with stable rule codes, typed RDF export, difference models read, applied,
written and computed, and the `cim` command line.

[0.2.0]: https://github.com/hupe1980/cim-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hupe1980/cim-rs/releases/tag/v0.1.0
