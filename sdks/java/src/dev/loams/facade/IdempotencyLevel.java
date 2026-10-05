package dev.loams.facade;

/** The {@code idempotency_level} off a proto method. The retry class is derived from it. */
public enum IdempotencyLevel {
    /**
     * {@code NO_SIDE_EFFECTS}: a read, retryable on its own.
     */
    NO_SIDE_EFFECTS("no_side_effects"),
    /**
     * {@code IDEMPOTENT}: repeating it is the same call.
     */
    IDEMPOTENT("idempotent"),
    /**
     * Carries no idempotency level: a mutation, which may only be retried when it carries an
     * idempotency key.
     */
    NONE("none");

    private final String wire;

    IdempotencyLevel(String wire) {
        this.wire = wire;
    }

    public String wire() {
        return wire;
    }
}