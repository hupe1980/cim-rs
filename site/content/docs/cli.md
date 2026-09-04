+++
title = "Command line"
description = "The cim command: inspect, validate, export, convert to RDF and diff CGMES model sets from a shell. Full option reference and exit codes."
weight = 70
+++

A library for a file format that cannot be pointed at a file is hard to evaluate. `cim` is
seven subcommands over a model set given as files, archives or directories.

```bash
cargo install cim-rs --features cli
```

It adds no dependency of its own, including for argument parsing, and is a thin shell over
the public API — so a gap in the tool is a gap in the library.

## Commands

| Command | Does |
|---|---|
| `cim info <input>…` | What a model set contains: files, object counts, profile coverage, findings |
| `cim validate <input>…` | Structural checks; exits 1 on any error |
| `cim export <input>… --out DIR` | Write the model back as CIM/XML |
| `cim rdf <input>… --out DIR` | Export as RDF, one graph per profile with data |
| `cim diff <base> <target>` | Compute a `dm:DifferenceModel` |
| `cim apply <input>… --change F --out DIR` | Apply a change set and write the result |
| `cim schema` | What the built-in vintages declare |

An `<input>` is a CIM/XML file, a zip archive, or a directory holding either. Several are
loaded into **one** model, which is what a CGMES profile set is.

## Options

| Option | Effect |
|---|---|
| `-o`, `--out PATH` | Where to write — a directory, or a file for `diff` |
| `--vintage KEY` | Which schema to read against (`cgmes3`, `cgmes2`); by default it is detected from the input |
| `--profile KEY` | Restrict `rdf` and `diff` to a profile, e.g. `SSH`; repeat it for a file that serves several — `--profile EQ --profile OP --profile SC` writes the one graph a CGMES 2.4.15 Equipment file contains |
| `--change FILE` | A `dm:DifferenceModel` for `apply`; repeat to apply several, in the order given |
| `--assume-profile KEY` | Which profile a file's values belong to when it declares none — a document with no `md:FullModel` |
| `--rule CODE` | Show only this rule in `validate`, e.g. `CIM0007` |
| `--ntriples` | Write N-Triples rather than Turtle |
| `--merged` | `rdf` writes one graph for the whole model, not one per profile |
| `--strict` | Fail on anything the schema does not define |
| `--limit N` | How many findings to list (default 20; `0` for all) |
| `-q`, `--quiet` | Less chatter |
| `-h`, `--help` | Usage |

## Exit status

| | |
|---|---|
| `0` | Clean |
| `1` | The model has errors |
| `2` | The command was wrong |

That makes `cim validate` usable directly as a CI gate:

```bash
cim validate model/ --rule CIM0007 || exit 1
```

## Examples

```text
$ cim info MicroGrid-Type1-NL-MAS/
== files ==
  20210323T1730Z_1D_NL_EQ_1.xml     EQ+SC   2021-03-23T17:30:00Z
  20210323T1730Z_1D_NL_SSH_1.xml    SSH     2021-03-23T17:30:00Z
  …

== model ==
  objects              602
  values               3632
  Substation           1
  ACLineSegment        5
  PowerTransformer     3

== profile coverage ==
  EQ          247 objects       1577 values
  SSH         114 objects        189 values
  SV           86 objects        213 values

== diagnostics ==
  CIM0006       51
  CIM0014        1
```

```bash
# Both vintages in one binary. The document says which it is, so this needs no flag.
cim info CGMES_v2.4.15_MicroGridTestConfiguration_T4_BE_BB_Complete_v2.zip

# Compute a change set, then apply it to the model it was computed against.
cim diff before/ after/ --out change_DIFF.xml
cim apply before/ --change change_DIFF.xml --out updated/

# The change set as a document, the summary as commentary.
cim diff before/ after/ > change_DIFF.xml

# Typed RDF for a SHACL engine or a triple store.
cim rdf model/ --out graphs/ --ntriples
```

## Vintages

CIM/XML declares its vocabulary in the document, so `cim` reads the vintage off the root
element and `--vintage` is an override rather than a requirement. Forcing the wrong one is
reported as `CIM0021` and exits 1, because a model read against the wrong schema resolves no
class at all — an empty model, which otherwise looks like a clean load.

```bash
cim info model/                     # detected
cim info model/ --vintage cgmes2    # forced; a mismatch is an error, not an empty result
```

## Piping

`cim` writes through a fallible writer rather than `println!`, so
`cim info big-model/ | head` closes cleanly instead of panicking. A closed pipe is how a
command is told it has said enough.

**Standard output carries one thing at a time.** Where a command's result is a document —
`cim diff` without `--out` — the document is the whole of stdout and everything else goes to
stderr, so `cim diff a b > change_DIFF.xml` yields a file and `cim diff a b | xmllint --noout -`
works. With `--out` the document is a file and the report goes to stdout instead.

`cim rdf` follows the same rule: the paths it wrote go to stdout, the profiles it skipped for
want of data go to stderr.
