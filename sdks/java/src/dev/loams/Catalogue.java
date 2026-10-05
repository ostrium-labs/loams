package dev.loams;

import java.util.ArrayList;
import java.util.List;

/**
 * The module catalogue, as one call reports it.
 *
 * @param served the packages this binary serves
 * @param unavailable the packages it knows about and does not serve
 * @param missing the packages this SDK speaks that the instance does not list <em>at all</em> —
 *     a package whose services have not been defined yet, which is different from one that exists
 *     and is switched off
 * @param services every entry, in the server's order
 */
public record Catalogue(
        List<String> served, List<String> unavailable, List<String> missing, List<ServiceStatus> services) {

    public Catalogue {
        served = List.copyOf(served);
        unavailable = List.copyOf(unavailable);
        missing = List.copyOf(missing);
        services = List.copyOf(services);
    }

    /**
     * Whether a package is available, collapsing three states into a boolean.
     *
     * <p>A package that is not listed at all is <em>not</em> available, which is the answer a
     * caller wants even though it means something different — {@link #missing()} is where the
     * difference is kept.
     */
    public boolean available(String protoPackage) {
        for (ServiceStatus status : services) {
            if (status.protoPackage().equals(protoPackage)) {
                return status.available();
            }
        }
        return false;
    }

    /** The status of one package, or {@code null} when the server did not list it. */
    public ServiceStatus status(String protoPackage) {
        for (ServiceStatus status : services) {
            if (status.protoPackage().equals(protoPackage)) {
                return status;
            }
        }
        return null;
    }

    /**
     * Turn a {@code GetInstance} response into the catalogue.
     *
     * @param speaks every proto package this SDK speaks, which is what the {@code missing} half is
     *     computed against
     */
    static Catalogue from(
            dev.loams.gen.loams.instance.v1.GetInstanceResponse info, List<String> speaks) {
        List<ServiceStatus> services = new ArrayList<>();
        List<String> served = new ArrayList<>();
        List<String> unavailable = new ArrayList<>();
        for (dev.loams.gen.loams.instance.v1.ServiceStatus entry : info.getServicesList()) {
            ServiceStatus status = ServiceStatus.from(entry);
            services.add(status);
            if (status.available()) {
                served.add(status.protoPackage());
            } else {
                unavailable.add(status.protoPackage());
            }
        }
        // The SDK's own packages the server does not list at all. A package whose services have
        // not been defined yet has no entry, so this is different from one that exists and is
        // switched off, and conflating the two would report a server bug as a missing feature.
        List<String> missing = new ArrayList<>();
        for (String pkg : speaks) {
            boolean listed = false;
            for (ServiceStatus status : services) {
                if (status.protoPackage().equals(pkg)) {
                    listed = true;
                    break;
                }
            }
            if (!listed) {
                missing.add(pkg);
            }
        }
        return new Catalogue(served, unavailable, missing, services);
    }
}