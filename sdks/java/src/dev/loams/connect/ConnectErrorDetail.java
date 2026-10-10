package dev.loams.connect;

/**
 * One error detail, as the Connect and gRPC protocols carry it: a type URL and the serialized
 * message.
 *
 * <p>The Connect protocol's JSON form base64-encodes the message; gRPC's
 * {@code grpc-status-details-bin} wraps it in a {@code google.protobuf.Any}, whose {@code value}
 * the reader unwraps before getting here. Either way what arrives at this record is the same
 * pair, which is what lets one lookup find the {@code ErrorInfo} on all three protocols (R8).
 *
 * @param type the detail's type with any {@code type.googleapis.com/} prefix removed, for
 *     example {@code loams.errors.v1.ErrorInfo}
 * @param value the serialized message's bytes
 */
public record ConnectErrorDetail(String type, byte[] value) {

    /** The {@code ErrorInfo} type URL the runtime looks for among the details (R8). */
    public static final String ERROR_INFO = "loams.errors.v1.ErrorInfo";
}
