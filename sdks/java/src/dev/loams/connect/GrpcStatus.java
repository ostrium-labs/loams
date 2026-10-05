package dev.loams.connect;

import com.google.protobuf.CodedInputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;

/**
 * {@code google.rpc.Status}, read off the wire by field number.
 *
 * <p>gRPC and gRPC-Web do not put the reason in the body the way the Connect protocol does: a
 * failure arrives as an HTTP status plus {@code grpc-status}, {@code grpc-message} and
 * {@code grpc-status-details-bin}, and only the last one carries the
 * {@code loams.errors.v1.ErrorInfo}. Parsing it is what stops a gRPC-Web failure arriving at the
 * caller with a code and a message but no reason, which is the single outcome R8 forbids.
 *
 * <p>The message class lives in {@code google-common-protos}, which this SDK does not depend on
 * for the sake of four fields, so this reads them directly:
 *
 * <pre>
 * message Status {
 *   int32 code = 1;            // the Connect code's number
 *   string message = 2;
 *   repeated google.protobuf.Any details = 3;
 * }
 * </pre>
 *
 * <p>Unknown fields are skipped, so a server that adds one to {@code Status} still parses.
 */
public final class GrpcStatus {

    private GrpcStatus() {}

    /**
     * Parse the {@code google.rpc.Status} in a {@code grpc-status-details-bin} header value.
     *
     * @param base64 the header's value, which is standard base64 of the serialized message
     * @return the parsed status, or {@code null} when the value did not parse
     */
    public static Status parse(String base64) {
        if (base64 == null || base64.isEmpty()) {
            return null;
        }
        byte[] bytes;
        try {
            bytes = java.util.Base64.getDecoder().decode(base64);
        } catch (IllegalArgumentException e) {
            return null;
        }
        try {
            return parse(bytes);
        } catch (IOException e) {
            return null;
        }
    }

    /** Parse a serialized {@code google.rpc.Status}. */
    public static Status parse(byte[] bytes) throws IOException {
        CodedInputStream in = CodedInputStream.newInstance(bytes);
        int code = 0;
        String message = "";
        List<ConnectErrorDetail> details = new ArrayList<>();
        while (true) {
            int tag = in.readTag();
            if (tag == 0) {
                break;
            }
            switch (tag >>> 3) {
                case 1 -> code = in.readInt32();
                case 2 -> message = in.readStringRequireUtf8();
                case 3 -> details.add(any(in.readByteArray()));
                default -> in.skipField(tag);
            }
        }
        return new Status(code, message, details);
    }

    /** One {@code google.protobuf.Any}: a type URL and the serialized message. */
    private static ConnectErrorDetail any(byte[] encoded) {
        String typeUrl = "";
        byte[] value = new byte[0];
        try {
            CodedInputStream in = CodedInputStream.newInstance(encoded);
            while (true) {
                int tag = in.readTag();
                if (tag == 0) {
                    break;
                }
                switch (tag >>> 3) {
                    case 1 -> typeUrl = in.readStringRequireUtf8();
                    case 2 -> value = in.readByteArray();
                    default -> in.skipField(tag);
                }
            }
        } catch (IOException e) {
            // A detail that does not parse is dropped; the reason lives in a sibling.
            return new ConnectErrorDetail("", new byte[0]);
        }
        // gRPC puts the default Any prefix on the URL, so the wire's
        // `type.googleapis.com/loams.errors.v1.ErrorInfo` is trimmed back to the bare name the
        // Connect protocol uses, so one lookup finds the ErrorInfo on every protocol.
        String bare = typeUrl;
        int slash = bare.lastIndexOf('/');
        if (slash >= 0) {
            bare = bare.substring(slash + 1);
        }
        return new ConnectErrorDetail(bare, value);
    }




    /**
     * A parsed {@code google.rpc.Status}.
     *
     * @param code the gRPC status code, which is the Connect code's number
     * @param message the human-readable message
     * @param details the details, with their type URLs trimmed to their bare names
     */
    public record Status(int code, String message, List<ConnectErrorDetail> details) {

        public Status {
            details = List.copyOf(details);
        }
    }
}