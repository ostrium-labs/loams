package dev.loams.facade;

import java.util.List;

/**
 * One SDK module and its calls.
 *
 * <p>Provenance is the Q604 hand-written-facade fallback; see {@link Reason}.
 *
 * @param name the module's name in snake_case, as {@code loams.<name>}
 * @param summary one line for the module's reference docs
 * @param service the service behind the module
 * @param protoPackage the proto package, which is what {@code GetInstance.services[]} keys on
 * @param unstable whether the package's wire contract may still change, so {@code buf breaking}
 *     skips it and an SDK marks the module experimental (§44 §10.3)
 * @param derived whether this is a second facade name for the same RPCs, which has no
 *     summary of its own
 * @param calls the module's facade calls
 */
public record ModuleBinding(
        String name,
        String summary,
        String service,
        String protoPackage,
        boolean unstable,
        boolean derived,
        List<CallBinding> calls) {

    public ModuleBinding {
        calls = List.copyOf(calls);
    }
}