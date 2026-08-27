# Cross-validation

Every other check in this repository is written by this repository. The corpus census
compares our output against the input we parsed; `check_well_formed` and `check_ntriples`
judge it against grammars we implemented; even the SHACL run — which uses a real engine —
is driven by shapes we selected, over a graph we produced.

All of that can be true while `cim-rs` misunderstands CGMES in a way that is *internally
consistent*. A writer and a reader that share a misconception agree with each other
perfectly, and a round trip is precisely the test that cannot see one.

This directory holds the answer: implementations that are not ours.

```bash
cargo xtask crossvalidate                    # the assembled MicroGrid, by default
cargo xtask crossvalidate <model-directory>
```

Needs Docker and nothing else — no JDK, no Python environment.

## What runs

**`powsybl/`** — [PowSyBl](https://www.powsybl.org/) (Java, LF Energy) imports a CGMES model
set into its own network object model. It is asked what it found in the *published* files
and what it found in `cim-rs`'s re-export of them, and the two answers must be identical:
the same count of every kind of equipment, the same bus counts from both topology views,
and the same SHA-256 digest over every identifier in the network.

The independence is real rather than nominal. PowSyBl reads CGMES by loading it into an
RDF4J triple store and querying with SPARQL; `cim-rs` reads it with a purpose-built
streaming parser for the IEC 61970-552 subset. Two implementations that share a
misconception is what a self-check cannot rule out — two that do not share an *approach* is
about as independent as this gets.

**`rdflib/`** — [rdflib](https://rdflib.dev/) (Python) parses the RDF export and reports
what the graph contains. Not against our N-Triples grammar and not to validate shapes, but
to build a graph and count what is in it. The histogram of literal datatypes is the point:
that is the specific claim `src/rdf.rs` exists to make — CIM/XML carries no datatypes, so
the profile has to supply them — and it is the one thing no other check here would notice
losing. If the enrichment silently stopped working every literal would become a plain
string, and the grammar checker and the shapes would both still pass.

## What it currently says

On `MicroGrid-Type1-Merged`, PowSyBl builds the identical network from both file sets:

```
identifiables.......................         98 == 98
identifierDigest....................  f37e5b39… == f37e5b39…
byType.SWITCH.......................         28 == 28
busBreakerViewBuses.................         14 == 14
```

and rdflib parses 12,505 triples over 1,973 subjects, every one of them named rather than a
blank node, every one typed, with 5,219 literals carrying an XSD datatype — 3,493 of them
`xsd:float`, which is the distribution ENTSO-E's own shapes expect.

## It can fail

A check that cannot fail proves nothing, so this one was made to. Removing a single
`cim:Breaker` from one equipment file of the published set moves every dependent number and
changes the digest completely:

```
identifiables      98 -> 97
byType.SWITCH      28 -> 27
busBreakerViewBuses 14 -> 15      (the switch was holding two buses together)
identifierDigest   f37e5b39… -> cf84218c…
```

That the bus count went *up* is the part worth keeping: it is why the digest is here. A
check comparing only totals would have to reason about which direction each number should
move; a digest over the identifiers does not care.

## Why containers

A cross-validation is worth nothing if the reference implementation drifts underneath it —
a failure then reports somebody else's release notes rather than our regression. Both
images pin their dependency versions, and both are built from these Dockerfiles rather than
pulled, so what runs is what this directory says runs.

## Why not testcontainers

`testcontainers` manages the lifecycle of *service* containers: a database to connect to,
with a dynamic port and a readiness probe, torn down when the test ends. Its value is the
waiting and the port mapping.

Neither harness is a service. Each is a batch process over a read-only mounted directory
that writes one JSON object to stdout and exits. There is no port, no readiness, and
nothing to wait for that `docker run --rm` does not already do — and both images are built
from local Dockerfiles, which is the case `testcontainers` handles least naturally. It
would add a dependency tree behind a call that `xtask/src/crossvalidate.rs` makes in one
line.
