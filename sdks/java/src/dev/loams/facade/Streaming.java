package dev.loams.facade;

/** Whether a call answers with one message or with a stream. */
public enum Streaming {
    /** One request, one response. */
    UNARY,
    /**
     * A server stream. There is no client streaming and no bidi (D420): a browser cannot do
     * it over {@code fetch}, and half-duplex works through every proxy.
     */
    SERVER
}