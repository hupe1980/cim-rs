package dev.cimrs;

import com.powsybl.commons.datasource.DirectoryDataSource;
import com.powsybl.iidm.network.Identifiable;
import com.powsybl.iidm.network.Network;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * Read a CGMES model set with PowSyBl and print what PowSyBl thinks is in it.
 *
 * <p>This exists to answer one question that cim-rs cannot answer about itself: <em>does
 * another CGMES implementation, written independently and in another language, see the same
 * grid in the files cim-rs writes as in the files cim-rs read?</em> Round-tripping through
 * cim-rs's own reader cannot answer it — a writer and a reader that share a misunderstanding
 * agree with each other perfectly.
 *
 * <p>The output is deliberately a summary rather than a dump. Two CGMES files may differ in
 * whitespace, attribute order and element order while denoting the same network, so
 * comparing the documents would report differences that are not differences. What must
 * match is the <em>network</em>: how many of each kind of equipment, and which identifiers.
 *
 * <p>Everything is derived from {@link Identifiable}, which is the narrowest and most
 * stable part of the PowSyBl API — every element of a network is one, and each reports its
 * own {@code IdentifiableType}. Written against the per-type accessors instead
 * ({@code getDanglingLineStream()} and friends) this broke on a minor release, which is
 * precisely the failure a pinned cross-validation exists to keep out of the signal.
 */
public final class Summarize {

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: summarize <cgmes-zip-or-directory>");
            System.exit(2);
        }
        Network network = read(Path.of(args[0]));

        // How many of each kind of thing, keyed by PowSyBl's own type name.
        Map<String, Long> byType = new TreeMap<>();
        List<String> ids = new ArrayList<>();
        for (Identifiable<?> i : network.getIdentifiables()) {
            byType.merge(i.getType().name(), 1L, Long::sum);
            ids.add(normalizeId(i.getId()));
        }

        Map<String, Object> out = new TreeMap<>();
        out.put("identifiables", (long) ids.size());
        // The part that makes this a real check: counts alone still pass when one breaker
        // is swapped for another, but a digest over every identifier does not.
        Collections.sort(ids);
        out.put("identifierDigest", sha256(String.join("\n", ids)));
        // Buses are a view over the network rather than stored elements, so they are the
        // one count worth taking separately: they are what connectivity *produces*, and a
        // topology misread shows up here before it shows up anywhere else.
        out.put("busBreakerViewBuses", network.getBusBreakerView().getBusStream().count());
        out.put("busViewBuses", network.getBusView().getBusStream().count());
        out.put("byType", byType);

        // A digest says "these differ" and nothing more, so the identifiers themselves are
        // available for the one question that follows: which ones.
        if (System.getenv("CIMRS_DUMP_IDS") != null) {
            ids.forEach(System.err::println);
        }

        System.out.println(toJson(out));
    }

    /**
     * Put a *synthesized* identifier into a form that does not depend on file order.
     *
     * <p>Most identifiers here are mRIDs and are compared exactly, which is the point. A
     * few are not: PowSyBl names a tie line by joining the mRIDs of the two dangling lines
     * it paired, with a `+`, <em>in the order it encountered them</em>. cim-rs writes
     * objects in mRID order — deliberately, so that an unchanged model re-exports
     * byte-identically — which can put the two halves in the other order than the source
     * file did, and the composite name flips.
     *
     * <p>The network is the same either way: a tie line's identity is the unordered pair of
     * its halves, and PowSyBl's rendering of it is an artefact of its own reading order
     * rather than anything the exchange says. Sorting the parts compares what the pair is
     * instead of which half was seen first. Identifiers with no `+` are untouched, so this
     * loosens nothing about the objects that actually carry an mRID.
     */
    private static String normalizeId(String id) {
        if (id.indexOf('+') < 0) {
            return id;
        }
        String[] parts = id.split("\\+");
        java.util.Arrays.sort(parts);
        return String.join("+", parts);
    }

    /**
     * Read either a zip archive or a directory of instance files.
     *
     * <p>A CGMES model set is a *set*, and both shapes occur: ENTSO-E ships the conformity
     * configurations as directories, while a real exchange is normally one archive.
     * `Network.read(Path)` handles the archive; a directory needs a data source, because
     * there is no single file for PowSyBl to sniff a format from.
     */
    private static Network read(Path path) {
        if (java.nio.file.Files.isDirectory(path)) {
            return Network.read(new DirectoryDataSource(path, ""));
        }
        return Network.read(path);
    }

    private static String sha256(String text) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256")
                .digest(text.getBytes(StandardCharsets.UTF_8));
        StringBuilder sb = new StringBuilder(digest.length * 2);
        for (byte b : digest) {
            sb.append(String.format("%02x", b));
        }
        return sb.toString();
    }

    /**
     * Emit a JSON object, one level of nesting deep. Hand-written rather than pulled from a
     * library: the values are longs, hex strings and one string-to-long map, so there is
     * nothing to escape, and a dependency for this would be a dependency to keep current.
     */
    private static String toJson(Map<String, ?> map) {
        StringBuilder sb = new StringBuilder("{");
        boolean first = true;
        for (Map.Entry<String, ?> e : map.entrySet()) {
            if (!first) {
                sb.append(',');
            }
            first = false;
            sb.append('"').append(e.getKey()).append("\":");
            Object v = e.getValue();
            if (v instanceof String s) {
                sb.append('"').append(s).append('"');
            } else if (v instanceof Map<?, ?> m) {
                @SuppressWarnings("unchecked")
                Map<String, ?> nested = (Map<String, ?>) m;
                sb.append(toJson(nested));
            } else {
                sb.append(v);
            }
        }
        return sb.append('}').toString();
    }

    private Summarize() {
    }
}
