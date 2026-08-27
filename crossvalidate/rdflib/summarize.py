"""Parse cim-rs's RDF export with rdflib and print what rdflib thinks is in it.

The counterpart of the PowSyBl check, for the other output format. `cim-rs` claims its RDF
export is ordinary RDF that any toolchain can load — the whole point of `src/rdf.rs`, since
CIM/XML itself is not RDF/XML and carries no datatypes. The repository already checks that
output against a hand-written N-Triples grammar and against ENTSO-E's SHACL shapes, but
both of those are checks *this* repository wrote. rdflib is not.

Three things are reported, and each answers a question the crate cannot answer about
itself:

* **triples** — that the file parses at all, and to how much. A grammar checker accepts a
  file; a parser builds a graph from it, which is a stronger statement.
* **subjects / typed subjects** — that the objects have global names and types, rather than
  the graph having silently collapsed into blank nodes.
* **datatypes** — the histogram of literal datatypes. This is the claim `rdf.rs` exists to
  make: that literals carry the XSD type the profile assigns, rather than all being
  strings. If the enrichment silently stopped working, every count would move to
  `xsd:string` and nothing else in the repository would notice.
"""

import json
import sys
from collections import Counter

from rdflib import Graph, Literal, URIRef
from rdflib.namespace import RDF


def summarize(path: str) -> dict:
    graph = Graph()
    # Let rdflib infer the syntax from the suffix, so this handles both the `.ttl` and the
    # `.nt` the exporter can write and disagrees loudly if one is mislabelled.
    graph.parse(path)

    datatypes = Counter()
    plain_literals = 0
    for _, _, o in graph:
        if isinstance(o, Literal):
            if o.datatype is None:
                plain_literals += 1
            else:
                datatypes[str(o.datatype)] += 1

    subjects = set(graph.subjects())
    typed = set(graph.subjects(RDF.type, None))

    # Which classes appear as a blank node, and which as a named one.
    #
    # A blank node is not automatically a defect: a CIM *compound* — `Location.mainAddress`
    # holding a `StreetAddress` — has no identity to name, so a blank node is exactly right
    # for it. An *object* becoming one is the defect, and the two are told apart without any
    # schema knowledge: a class that appears only ever blank is a compound, while a class
    # that appears both named and blank means an identity was lost somewhere.
    blank_types = Counter()
    named_types = Counter()
    for s, o in graph.subject_objects(RDF.type):
        (named_types if isinstance(s, URIRef) else blank_types)[str(o)] += 1

    return {
        "triples": len(graph),
        "subjects": len(subjects),
        "namedSubjects": sum(1 for s in subjects if isinstance(s, URIRef)),
        "typedSubjects": len(typed),
        "plainLiterals": plain_literals,
        # Classes that lost an identity: seen both ways in one graph.
        "inconsistentlyNamed": len(set(blank_types) & set(named_types)),
        "blankOnlyTypes": len(set(blank_types) - set(named_types)),
        # Named so a failure says *which* class lost an identity, not only how many.
        "inconsistent": {
            t.rsplit("#", 1)[-1]: blank_types[t]
            for t in sorted(set(blank_types) & set(named_types))
        },
        "datatypes": dict(sorted(datatypes.items())),
    }


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: summarize.py <graph.ttl|graph.nt>", file=sys.stderr)
        return 2
    print(json.dumps(summarize(sys.argv[1]), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
