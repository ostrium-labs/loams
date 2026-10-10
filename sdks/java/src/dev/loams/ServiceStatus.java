package dev.loams;

import java.util.List;

/**
 * One package of the API catalogue and what this binary does with it, as
 * {@code GetInstance.services[]} reports it.
 *
 * @param protoPackage the proto package, for example {@code loams.live.v1}
 * @param version the package's API version, for example {@code v1}
 * @param available whether this binary serves the package. False means every one of its RPCs
 *     answers {@code unimplemented} with reason {@code feature_not_in_variant}.
 * @param services the fully qualified service names in the package
 * @param unstable whether the package's wire contract may still change
 */
public record ServiceStatus(
        String protoPackage,
        String version,
        boolean available,
        List<String> services,
        boolean unstable) {

    public ServiceStatus {
        services = List.copyOf(services);
    }

    /**
     * The SDK's view of {@code GetInstance.services[]}.
     *
     * <p>The generated class is also called {@code ServiceStatus}, and Java has no import alias,
     * so it is named in full here rather than imported.
     */
    static ServiceStatus from(dev.loams.gen.loams.instance.v1.ServiceStatus status) {
        return new ServiceStatus(
                status.getPackage(),
                status.getVersion(),
                status.getAvailable(),
                status.getServicesList(),
                status.getUnstable());
    }
}