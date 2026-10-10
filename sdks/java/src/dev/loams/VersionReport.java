package dev.loams;

import java.util.List;

/**
 * What this SDK speaks beside what the server serves (runtime contract R9).
 *
 * @param protoRev the proto revision this SDK was generated from
 * @param serverVersion the server's own semver, as {@code GetInstance} reports it
 * @param apiVersions the server's {@code GetInstance.api_versions}: only what it serves, so a
 *     package this SDK speaks and the server does not is {@code missing} rather than reported as
 *     available
 * @param compatible whether every package the SDK speaks is served
 * @param missing the SDK's packages the server does not serve
 */
public record VersionReport(
        String protoRev,
        String serverVersion,
        List<String> apiVersions,
        boolean compatible,
        List<String> missing) {

    public VersionReport {
        apiVersions = List.copyOf(apiVersions);
        missing = List.copyOf(missing);
    }
}