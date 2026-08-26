#!/usr/bin/env bash
# fetch-specs.sh — download the machine-readable CIM/CGMES standards artifacts
# into ./specs (gitignored). Only publicly licensed material is fetched:
#   - UCAIug/ENTSO-E RDFS + SHACL + PROF artifacts: Apache-2.0
#   - ENTSO-E conformity test configurations:       CC BY-SA 4.0 (attribution required)
#   - Public ENTSO-E technical documents (PDF)
# IEC standard documents (61970-301/-552/-600, ...) are paywalled and are NOT
# downloaded here — purchase from the IEC webstore if you need the prose.
#
# Usage: scripts/fetch-specs.sh [--clean]
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPECS="$ROOT/specs"
ARCHIVES="$SPECS/_archives"

# Pinned upstream ref for the ENTSO-E application profiles library.
APL_REPO="https://github.com/entsoe/application-profiles-library"
APL_TAG="v1.1.1"

[ "${1:-}" = "--clean" ] && rm -rf "$SPECS"
mkdir -p "$SPECS" "$ARCHIVES"

FAILURES=()

fetch_zip() { # url, dest-dir
  local url="$1" dest="$2" name
  name="$(basename "${url%%\?*}" | sed 's/%20/ /g')"
  local zip="$ARCHIVES/$name"
  if [ ! -s "$zip" ]; then
    echo "==> downloading $name"
    if ! curl -fSL --http1.1 -C - --retry 5 --retry-all-errors --connect-timeout 30 -o "$zip.part" "$url"; then
      echo "!! FAILED: $url" >&2; FAILURES+=("$url"); return 1
    fi
    mv "$zip.part" "$zip"
  else
    echo "==> cached   $name"
  fi
  if [ ! -d "$SPECS/$dest" ]; then
    mkdir -p "$SPECS/$dest"
    unzip -q -o "$zip" -d "$SPECS/$dest" || { echo "!! unzip failed: $name" >&2; FAILURES+=("$url"); }
  fi
}

fetch_file() { # url, dest-path (relative to specs/)
  local url="$1" dest="$SPECS/$2"
  mkdir -p "$(dirname "$dest")"
  if [ ! -s "$dest" ]; then
    echo "==> downloading $(basename "$dest")"
    if ! curl -fSL --retry 3 --connect-timeout 30 -o "$dest.part" "$url"; then
      echo "!! FAILED: $url" >&2; rm -f "$dest.part"; FAILURES+=("$url"); return 1
    fi
    mv "$dest.part" "$dest"
  else
    echo "==> cached   $(basename "$dest")"
  fi
}

# ---------------------------------------------------------------------------
# 1. ENTSO-E application profiles library (CGMES 3.0 + 2.4 RDFS/SHACL/PROF, NC)
#    Apache-2.0, pinned tag.
# ---------------------------------------------------------------------------
if [ ! -d "$SPECS/application-profiles-library/.git" ]; then
  echo "==> cloning application-profiles-library @ $APL_TAG"
  git clone --depth 1 --branch "$APL_TAG" "$APL_REPO" \
    "$SPECS/application-profiles-library" \
    || { echo "!! clone failed, trying default branch" >&2; \
         git clone --depth 1 "$APL_REPO" "$SPECS/application-profiles-library" \
         || FAILURES+=("$APL_REPO"); }
else
  echo "==> cached   application-profiles-library"
fi

# ---------------------------------------------------------------------------
# 2. CGMES 2.4.15 schema packages (RDFS 04Jul2016 + 2020 component refresh)
# ---------------------------------------------------------------------------
fetch_zip "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/ENTSOE_CGMES_v2.4.15_04Jul2016_RDFS.zip" \
          "cgmes-2.4.15/rdfs-2016"
fetch_zip "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/CGMES2415_Components_2020.zip" \
          "cgmes-2.4.15/components-2020"

# ---------------------------------------------------------------------------
# 3. Conformity assessment test configurations (CC BY-SA 4.0 — attribution
#    required; keep OUT of any published crate, use as local test corpus only)
# ---------------------------------------------------------------------------
fetch_zip "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/CGMES_ConformityAssessmentScheme_TestConfigurations_v3-0-3.zip" \
          "test-models/cas-3.0.3"
fetch_zip "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/TestConfigurations_packageCASv2.0.zip" \
          "test-models/cas-2.0"
fetch_zip "https://www.entsoe.eu/Documents/CIM_documents/Grid_Model_CIM/QoCDC_v3.2.1_test_models.zip" \
          "test-models/qocdc-3.2.1"

# ---------------------------------------------------------------------------
# 4. Public ENTSO-E technical documents
# ---------------------------------------------------------------------------
fetch_file "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/RDF-SyntaxUserGuide_v1-0.pdf" \
           "docs/RDF-SyntaxUserGuide_v1-0.pdf"
fetch_file "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/00_ApplicationProfilesReadMe.pdf" \
           "docs/ApplicationProfilesReadMe.pdf"
fetch_file "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/140807_ENTSOE_CGMES_v2.4.15.pdf" \
           "docs/CGMES_v2.4.15_TechnicalSpecification.pdf"
fetch_file "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/IOP/CGMES_2_5_TechnicalSpecification_61970-600_Part%201_Ed2.pdf" \
           "docs/CGMES_2.5_TechnicalSpecification_61970-600_Part1_Ed2.pdf"
fetch_file "https://eepublicdownloads.entsoe.eu/clean-documents/CIM_documents/Grid_Model_CIM/QandA_on_CIM_CGMES_based_data_exchange_implementation_v1-0-1.pdf" \
           "docs/QandA_CIM_CGMES_data_exchange_implementation_v1-0-1.pdf"

# ---------------------------------------------------------------------------
# Manifest: provenance + checksums of everything fetched
# ---------------------------------------------------------------------------
{
  echo "# Generated by scripts/fetch-specs.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# application-profiles-library ref: $(git -C "$SPECS/application-profiles-library" rev-parse HEAD 2>/dev/null || echo 'n/a')"
  (cd "$SPECS" && find _archives docs -type f \( -name '*.zip' -o -name '*.pdf' \) -exec shasum -a 256 {} \; 2>/dev/null | sort -k2)
} > "$SPECS/MANIFEST.txt"

echo
if [ ${#FAILURES[@]} -gt 0 ]; then
  echo "Completed with ${#FAILURES[@]} failure(s):"; printf ' - %s\n' "${FAILURES[@]}"; exit 1
fi
echo "All specs fetched into $SPECS"
