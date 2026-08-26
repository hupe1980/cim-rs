//! Which RDFS vocabularies make up each CIM vintage.
//!
//! Vintages differ in ways the files themselves do not always state. CGMES 3.0 carries an
//! `owl:Ontology` block naming its keyword and version IRI, so its profiles describe
//! themselves. CGMES 2.4.15 predates that convention: its vocabularies carry no ontology
//! header, its Equipment profile ships as pre-combined Core / Operation / ShortCircuit
//! files, and the profile IRIs that appear in instance headers exist only in the
//! specification text. Both therefore need an explicit description here.

/// How to interpret one RDFS file as a profile.
pub struct ProfileSpec {
    /// Short keyword, e.g. `EQ`.
    pub keyword: &'static str,
    /// RDFS file name within the vintage's directory.
    pub file: &'static str,
    /// Value written as `md:Model.profile`. Empty means "take it from the ontology".
    pub version_iri: &'static str,
    /// Further IRIs that also denote this profile when read.
    pub aliases: &'static [&'static str],
    /// Restrict this profile to attributes carrying one of these `cims:stereotype`
    /// values, and exclude those stereotypes from every other profile drawn from the
    /// same file. Empty means "everything not claimed by a sibling".
    pub stereotypes: &'static [&'static str],
    pub title: &'static str,
}

pub struct Vintage {
    /// Module name, e.g. `cgmes3`.
    pub key: &'static str,
    pub title: &'static str,
    /// Directory holding the RDFS files, relative to `specs/`.
    pub rdfs_dir: &'static str,
    pub profiles: &'static [ProfileSpec],
}

pub const VINTAGES: &[Vintage] = &[CGMES3, CGMES2];

/// CGMES 3.0 (IEC TS 61970-600-1/-2:2021).
///
/// Each vocabulary declares its own keyword and version IRI, so only the file list is
/// needed. The 2019 header vocabulary shipped alongside the 2020 one is superseded.
pub const CGMES3: Vintage = Vintage {
    key: "cgmes3",
    title: "CGMES 3.0 (IEC TS 61970-600-1/-2:2021)",
    rdfs_dir: "application-profiles-library/CGMES/CurrentRelease/RDFS",
    profiles: &[
        p("EQ", "61970-600-2_Equipment-AP-Voc-RDFS2020.rdf"),
        p("OP", "61970-600-2_Operation-AP-Voc-RDFS2020.rdf"),
        p("SC", "61970-600-2_ShortCircuit-AP-Voc-RDFS2020.rdf"),
        p("EQBD", "61970-600-2_EquipmentBoundary-AP-Voc-RDFS2020.rdf"),
        p(
            "SSH",
            "61970-600-2_SteadyStateHypothesis-AP-Voc-RDFS2020.rdf",
        ),
        p("TP", "61970-600-2_Topology-AP-Voc-RDFS2020.rdf"),
        p("SV", "61970-600-2_StateVariables-AP-Voc-RDFS2020.rdf"),
        p("DL", "61970-600-2_DiagramLayout-AP-Voc-RDFS2020.rdf"),
        p("GL", "61970-600-2_GeographicalLocation-AP-Voc-RDFS2020.rdf"),
        p("DY", "61970-600-2_Dynamics-AP-Voc-RDFS2020.rdf"),
        p("FH", "61970-600-2_Header-AP-Voc-RDFS2020.rdf"),
    ],
};

/// A profile whose RDFS declares its own identity.
const fn p(keyword: &'static str, file: &'static str) -> ProfileSpec {
    ProfileSpec {
        keyword,
        file,
        version_iri: "",
        aliases: &[],
        stereotypes: &[],
        title: keyword,
    }
}

/// CGMES 2.4.15, the profile set in production use across Europe since 2021.
///
/// Two things differ from CGMES 3.0 and are handled here rather than in the parser:
///
/// * The vocabularies carry no `owl:Ontology`, so keywords, titles and the profile IRIs
///   that appear in `md:Model.profile` are stated explicitly.
/// * Equipment, Operation and ShortCircuit share one vocabulary file. They are separated
///   by the `Operation` and `ShortCircuit` stereotypes the file already carries, which is
///   exactly the split CGMES 3.0 later made into separate files.
const CGMES2: Vintage = Vintage {
    key: "cgmes2",
    title: "CGMES 2.4.15",
    rdfs_dir: "application-profiles-library/CGMES/PastReleases/v2-4/Original/RDFS",
    profiles: &[
        ProfileSpec {
            keyword: "EQ",
            file: EQ_2_4,
            version_iri: "http://entsoe.eu/CIM/EquipmentCore/3/1",
            aliases: &[],
            // Everything the Operation and ShortCircuit siblings do not claim.
            stereotypes: &[],
            title: "Equipment Core",
        },
        ProfileSpec {
            keyword: "OP",
            file: EQ_2_4,
            version_iri: "http://entsoe.eu/CIM/EquipmentOperation/3/1",
            aliases: &[],
            stereotypes: &["Operation"],
            title: "Equipment Operation",
        },
        ProfileSpec {
            keyword: "SC",
            file: EQ_2_4,
            version_iri: "http://entsoe.eu/CIM/EquipmentShortCircuit/3/1",
            aliases: &[],
            stereotypes: &["ShortCircuit"],
            title: "Equipment Short Circuit",
        },
        ProfileSpec {
            keyword: "EQBD",
            file: "EquipmentBoundaryProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/EquipmentBoundary/3/1",
            aliases: &["http://entsoe.eu/CIM/EquipmentBoundaryOperation/3/1"],
            stereotypes: &[],
            title: "Equipment Boundary",
        },
        ProfileSpec {
            keyword: "TPBD",
            file: "TopologyBoundaryProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/TopologyBoundary/3/1",
            aliases: &[],
            stereotypes: &[],
            title: "Topology Boundary",
        },
        ProfileSpec {
            keyword: "SSH",
            file: "SteadyStateHypothesisProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/SteadyStateHypothesis/1/1",
            aliases: &[],
            stereotypes: &[],
            title: "Steady State Hypothesis",
        },
        ProfileSpec {
            keyword: "TP",
            file: "TopologyProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/Topology/4/1",
            aliases: &[],
            stereotypes: &[],
            title: "Topology",
        },
        ProfileSpec {
            keyword: "SV",
            file: "StateVariableProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/StateVariables/4/1",
            aliases: &[],
            stereotypes: &[],
            title: "State Variables",
        },
        ProfileSpec {
            keyword: "DL",
            file: "DiagramLayoutProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/DiagramLayout/3/1",
            aliases: &[],
            stereotypes: &[],
            title: "Diagram Layout",
        },
        ProfileSpec {
            keyword: "GL",
            file: "GeographicalLocationProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/GeographicalLocation/2/1",
            aliases: &[],
            stereotypes: &[],
            title: "Geographical Location",
        },
        ProfileSpec {
            keyword: "DY",
            file: "DynamicsProfileRDFSAugmented-v2_4_15-4Sep2020.rdf",
            version_iri: "http://entsoe.eu/CIM/Dynamics/3/1",
            aliases: &[],
            stereotypes: &[],
            title: "Dynamics",
        },
        ProfileSpec {
            keyword: "FH",
            file: "FileHeader.rdf",
            version_iri: "http://iec.ch/TC57/61970-552/ModelDescription/1",
            aliases: &[],
            stereotypes: &[],
            title: "File Header",
        },
    ],
};

/// The Equipment vocabulary carrying Core, Operation and ShortCircuit together.
const EQ_2_4: &str = "EquipmentProfileCoreOperationShortCircuitRDFSAugmented-v2_4_15-4Sep2020.rdf";

pub fn by_key(key: &str) -> Option<&'static Vintage> {
    VINTAGES.iter().find(|v| v.key == key)
}
