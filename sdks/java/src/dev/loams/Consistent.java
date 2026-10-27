package dev.loams;

import dev.loams.gen.loams.collection.v1.Consistency;
import dev.loams.gen.loams.collection.v1.Freshness;

/** Consistency levels for a collection read (SDKP §2.4). */
public final class Consistent {

    private Consistent() {}

    /** Read everything acknowledged so far. */
    public static Consistency strong() {
        return Consistency.newBuilder().setFreshness(Freshness.FRESHNESS_STRONG).build();
    }

    /** Read whatever the node has; fastest. */
    public static Consistency eventual() {
        return Consistency.newBuilder().setFreshness(Freshness.FRESHNESS_EVENTUAL).build();
    }

    /** Read at least what a write's token reached. */
    public static Consistency atLeast(String token) {
        return Consistency.newBuilder().setAtLeast(token).build();
    }

    /** Read at least what a write's token reached. */
    public static Consistency atLeast(ConsistencyToken token) {
        return atLeast(token.toString());
    }
}
