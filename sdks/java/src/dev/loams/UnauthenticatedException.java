package dev.loams;

/**
 * unauthenticated: the request carried no valid credential.
 *
 * <p>One of the subclasses of {@link LoamsException} D611 names. It adds no field of its own,
 * because the branch is what a caller wants and the {@link #reason()} is on the base.
 *
 * <p>See {@link LoamsException} for the hierarchy, the three cases that are deliberately not
 * conflated, and how to branch on it.
 */
public class UnauthenticatedException extends LoamsException {

    private static final long serialVersionUID = 1L;

    /**
     * @param reason the stable cause, or {@code null} when the failure came from below the API
     * @param unknownReason a reason off the wire this SDK's registry does not have
     * @param metadata the structured context the server sent. Never secrets.
     * @param hint a short next step in the caller's locale
     * @param rpc the failing RPC, as {@code package.Service/Method}
     * @param cause the failure below this one
     */
    public UnauthenticatedException(
            Code code,
            dev.loams.facade.Reason reason,
            String unknownReason,
            java.util.Map<String, String> metadata,
            String hint,
            String rpc,
            Throwable cause) {
        super(code, reason, unknownReason, metadata, hint, rpc, cause, false);
    }
}
