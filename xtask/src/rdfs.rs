//! Minimal reader for the flat RDF/XML dialect used by CIM/CGMES RDFS vocabularies.
//!
//! CIM RDFS files (IEC 61970-501 style, as published by ENTSO-E) are deliberately
//! *flat*: a single `rdf:RDF` root containing a list of `rdf:Description` elements,
//! each with simple property children. Properties carry their value either as an
//! `rdf:resource` IRI or as element text. That makes a small purpose-built reader
//! both sufficient and far simpler than a general RDF toolchain.

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;
use std::path::Path;

/// One `rdf:Description` block from an RDFS document.
#[derive(Debug, Clone)]
pub struct Description {
    /// Absolute IRI of the described resource (`rdf:about`, resolved against `xml:base`).
    pub iri: String,
    /// Property values in document order.
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub struct Prop {
    /// Expanded predicate IRI, e.g. `http://www.w3.org/2000/01/rdf-schema#label`.
    pub predicate: String,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub enum Value {
    /// `rdf:resource="..."` — an IRI reference, resolved against `xml:base`.
    Resource(String),
    /// Element text content.
    Literal(String),
}

impl Description {
    /// First value of `predicate` as a resource IRI.
    pub fn resource(&self, predicate: &str) -> Option<&str> {
        self.props.iter().find_map(|p| match &p.value {
            Value::Resource(r) if p.predicate == predicate => Some(r.as_str()),
            _ => None,
        })
    }

    /// First value of `predicate` as literal text.
    pub fn literal(&self, predicate: &str) -> Option<&str> {
        self.props.iter().find_map(|p| match &p.value {
            Value::Literal(l) if p.predicate == predicate => Some(l.as_str()),
            _ => None,
        })
    }

    /// All resource values of `predicate` (properties such as `rdf:type` repeat).
    pub fn resources<'a>(&'a self, predicate: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.props.iter().filter_map(move |p| match &p.value {
            Value::Resource(r) if p.predicate == predicate => Some(r.as_str()),
            _ => None,
        })
    }
}

/// A parsed RDFS document.
#[derive(Debug, Default)]
pub struct Document {
    pub descriptions: Vec<Description>,
    /// Namespace prefix -> IRI declared on the root element.
    pub prefixes: HashMap<String, String>,
}

pub fn parse_file(path: &Path) -> Result<Document> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading RDFS file {}", path.display()))?;
    parse_bytes(&bytes).with_context(|| format!("parsing RDFS file {}", path.display()))
}

pub fn parse_bytes(bytes: &[u8]) -> Result<Document> {
    let text = decode(bytes)?;
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    let mut doc = Document::default();
    let mut base = String::new();
    // Namespace bindings are collected from the root element; CIM RDFS files never
    // rebind prefixes on inner elements.
    let mut ns: HashMap<String, String> = HashMap::new();

    // State while inside an <rdf:Description>.
    let mut current: Option<Description> = None;
    let mut pending: Option<(String, String)> = None; // (predicate IRI, accumulated text)

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) => {
                let name = qname(e.name().as_ref());
                if ns.is_empty() {
                    collect_namespaces(&e, &mut ns, &mut base)?;
                }
                let expanded = expand(&name, &ns);

                if expanded == RDF_DESCRIPTION {
                    if current.is_some() {
                        bail!("nested rdf:Description is not supported in CIM RDFS documents");
                    }
                    current = Some(Description {
                        iri: resolve(&about(&e, &ns)?, &base),
                        props: Vec::new(),
                    });
                } else if let Some(desc) = current.as_mut() {
                    match resource_attr(&e, &ns)? {
                        Some(res) => desc.props.push(Prop {
                            predicate: expanded,
                            value: Value::Resource(resolve(&res, &base)),
                        }),
                        // Value arrives as text; accumulate until the matching end tag.
                        None => pending = Some((expanded, String::new())),
                    }
                }
            }
            Event::Empty(e) => {
                let name = qname(e.name().as_ref());
                if ns.is_empty() {
                    collect_namespaces(&e, &mut ns, &mut base)?;
                }
                let expanded = expand(&name, &ns);
                if let Some(desc) = current.as_mut() {
                    let value = match resource_attr(&e, &ns)? {
                        Some(res) => Value::Resource(resolve(&res, &base)),
                        None => Value::Literal(String::new()),
                    };
                    desc.props.push(Prop {
                        predicate: expanded,
                        value,
                    });
                } else if expanded == RDF_DESCRIPTION {
                    doc.descriptions.push(Description {
                        iri: resolve(&about(&e, &ns)?, &base),
                        props: Vec::new(),
                    });
                }
            }
            Event::Text(t) => {
                if let Some((_, buf)) = pending.as_mut() {
                    buf.push_str(&t.xml10_content()?);
                }
            }
            // A reference is its own event, so dropping it would silently delete the
            // character. The RDFS documentation strings are full of `&amp;` and `&lt;`,
            // and they end up in the generated rustdoc.
            Event::GeneralRef(r) => {
                if let Some((_, buf)) = pending.as_mut() {
                    match r.resolve_char_ref() {
                        Ok(Some(c)) => buf.push(c),
                        Ok(None) | Err(_) => {
                            let name = r.decode()?;
                            match quick_xml::escape::resolve_predefined_entity(&name) {
                                Some(text) => buf.push_str(text),
                                None => buf.push_str(&format!("&{name};")),
                            }
                        }
                    }
                }
            }
            Event::CData(t) => {
                if let Some((_, buf)) = pending.as_mut() {
                    buf.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::End(e) => {
                let expanded = expand(&qname(e.name().as_ref()), &ns);
                if let Some((predicate, buf)) = pending.take() {
                    if predicate == expanded {
                        if let Some(desc) = current.as_mut() {
                            desc.props.push(Prop {
                                predicate,
                                value: Value::Literal(buf),
                            });
                        }
                    } else {
                        // Unbalanced: restore so we do not silently drop the value.
                        pending = Some((predicate, buf));
                    }
                }
                if expanded == RDF_DESCRIPTION
                    && let Some(desc) = current.take()
                {
                    doc.descriptions.push(desc);
                }
            }
            _ => {}
        }
    }

    doc.prefixes = ns;
    Ok(doc)
}

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDF_DESCRIPTION: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#Description";

fn decode(bytes: &[u8]) -> Result<String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_owned()),
        // A few published artifacts are latin-1; recover rather than fail the build.
        Err(_) => Ok(bytes.iter().map(|&b| b as char).collect()),
    }
}

fn qname(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

fn collect_namespaces(
    e: &BytesStart<'_>,
    ns: &mut HashMap<String, String>,
    base: &mut String,
) -> Result<()> {
    for attr in e.attributes().with_checks(false) {
        let attr = attr?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        let val = attr
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)?
            .into_owned();
        if let Some(prefix) = key.strip_prefix("xmlns:") {
            ns.insert(prefix.to_owned(), val);
        } else if key == "xmlns" {
            ns.insert(String::new(), val);
        } else if key == "xml:base" {
            *base = val.trim().to_owned();
        }
    }
    Ok(())
}

fn expand(qname: &str, ns: &HashMap<String, String>) -> String {
    match qname.split_once(':') {
        Some((prefix, local)) => match ns.get(prefix) {
            Some(iri) => format!("{iri}{local}"),
            None => qname.to_owned(),
        },
        None => match ns.get("") {
            Some(iri) => format!("{iri}{qname}"),
            None => qname.to_owned(),
        },
    }
}

fn attr_value(
    e: &BytesStart<'_>,
    ns: &HashMap<String, String>,
    want: &str,
) -> Result<Option<String>> {
    for attr in e.attributes().with_checks(false) {
        let attr = attr?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        // Attribute names are QNames too; `rdf:about` must expand to the RDF namespace.
        if let Some((prefix, local)) = key.split_once(':')
            && local == want
            && ns.get(prefix).map(String::as_str) == Some(RDF_NS)
        {
            return Ok(Some(
                attr.normalized_value(quick_xml::XmlVersion::Explicit1_0)?
                    .trim()
                    .to_owned(),
            ));
        }
    }
    Ok(None)
}

fn about(e: &BytesStart<'_>, ns: &HashMap<String, String>) -> Result<String> {
    if let Some(v) = attr_value(e, ns, "about")? {
        return Ok(v);
    }
    if let Some(v) = attr_value(e, ns, "ID")? {
        return Ok(format!("#{v}"));
    }
    bail!("rdf:Description without rdf:about or rdf:ID")
}

fn resource_attr(e: &BytesStart<'_>, ns: &HashMap<String, String>) -> Result<Option<String>> {
    attr_value(e, ns, "resource")
}

/// Resolve a possibly relative IRI against the document's `xml:base`.
///
/// CIM RDFS uses only the `#fragment` form, which appends to the base.
fn resolve(iri: &str, base: &str) -> String {
    if base.is_empty() || !iri.starts_with('#') {
        iri.to_owned()
    } else {
        format!("{base}{iri}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_descriptions_with_base_resolution() {
        let src = r##"<?xml version="1.0"?>
        <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                 xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
                 xmlns:cims="http://iec.ch/TC57/1999/rdf-schema-extensions-19990926#"
                 xml:base="http://iec.ch/TC57/CIM100">
          <rdf:Description rdf:about="#ACLineSegment">
            <rdfs:label xml:lang="en">ACLineSegment</rdfs:label>
            <rdfs:subClassOf rdf:resource="#Conductor"/>
            <cims:stereotype>CIMDatatype</cims:stereotype>
          </rdf:Description>
        </rdf:RDF>"##;
        let doc = parse_bytes(src.as_bytes()).unwrap();
        assert_eq!(doc.descriptions.len(), 1);
        let d = &doc.descriptions[0];
        assert_eq!(d.iri, "http://iec.ch/TC57/CIM100#ACLineSegment");
        assert_eq!(
            d.literal("http://www.w3.org/2000/01/rdf-schema#label"),
            Some("ACLineSegment")
        );
        assert_eq!(
            d.resource("http://www.w3.org/2000/01/rdf-schema#subClassOf"),
            Some("http://iec.ch/TC57/CIM100#Conductor")
        );
        assert_eq!(
            d.literal("http://iec.ch/TC57/1999/rdf-schema-extensions-19990926#stereotype"),
            Some("CIMDatatype")
        );
    }

    #[test]
    fn absolute_iris_are_not_rebased() {
        let src = r##"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                 xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
                 xml:base="http://iec.ch/TC57/CIM100">
          <rdf:Description rdf:about="http://iec.ch/TC57/CIM100-European#BoundaryPoint">
            <rdfs:domain rdf:resource="http://iec.ch/TC57/CIM100-European#BoundaryPoint"/>
          </rdf:Description>
        </rdf:RDF>"##;
        let doc = parse_bytes(src.as_bytes()).unwrap();
        assert_eq!(
            doc.descriptions[0].iri,
            "http://iec.ch/TC57/CIM100-European#BoundaryPoint"
        );
    }
}
