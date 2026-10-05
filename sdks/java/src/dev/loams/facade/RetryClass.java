package dev.loams.facade;

/** Whether the SDK may retry a call on its own. Derived from {@link IdempotencyLevel}. */
public enum RetryClass {
    /** A read or an idempotent RPC: the SDK retries it. */
    SAFE,
    /** A mutation: the SDK retries it only once it carries an idempotency key. */
    MANUAL
}