package dev.loams;

import dev.loams.facade.Reason;
import java.util.Map;

/**
 * A package this build variant does not carry (design §44 §4, D600).
 *
 * <p>The server answers {@code unimplemented} with {@code reason = feature_not_in_variant} and
 * names the variant in {@code metadata.variant}, which is what {@link #variant()} reads — read
 * out of the metadata rather than parsed out of the message, because the message is written for
 * a person and may change.
 *
 * <p>A caller usually never gets here: {@code client.system().guard("live")} feature-detects
 * from {@code GetInstance.services[]} before calling, so an unavailable module raises this same
 * type from the guard with no request spent. This type is the path for a caller who skipped the
 * guard, or whose instance changed variant.
 *
 * <p>One {@code catch (FeatureNotInVariantException)} therefore covers both "the guard said no"
 * and "the server refused", which is the point of the guard raising the same type.
 *
 * <p>It extends {@link UnimplementedException} so a caller that only cares about the coarse code
 * still matches.
 */
public class FeatureNotInVariantException extends UnimplementedException {

    private static final long serialVersionUID = 1L;

    /**
     * What {@link #variant()} says when the SDK learned the package was absent from the
     * catalogue rather than from a refusal.
     *
     * <p>A plausible-looking variant in a support ticket is worse than an honest "unknown", so
     * the guard records nothing rather than inventing one.
     */
    public static final String VARIANT_UNKNOWN = "unknown";

    private final String variant;

    public FeatureNotInVariantException(
            Code code,
            Reason reason,
            String unknownReason,
            Map<String, String> metadata,
            String hint,
            String rpc,
            Throwable cause,
            String variant) {
        super(code, reason, unknownReason, metadata, hint, rpc, cause);
        this.variant = variant == null ? "" : variant;
    }

    /**
     * The build variant that was asked for, from {@code metadata.variant}.
     *
     * <p>It is {@link #VARIANT_UNKNOWN} when a guard produced this rather than a server refusal,
     * and empty when the server sent no variant at all.
     */
    public final String variant() {
        return variant;
    }
}