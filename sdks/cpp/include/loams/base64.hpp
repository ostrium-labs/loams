// Base64, as the Connect and gRPC-Web error framing needs it.
//
// A failed RPC carries its `loams.errors.v1.ErrorInfo` as a **base64** string
// in `details[].value` — Connect JSON puts a `bytes` field there as base64,
// and gRPC-Web's `grpc-status-details-bin` is base64 of a `google.rpc.Status`
// whose details are base64 too. Nothing else in this SDK needs base64, so it is
// here rather than in the error path: the conformance suite reads recorded
// fixtures with it, and a suite that had its own copy would be a second
// implementation to keep correct.
//
// The alphabet is the standard one (`A-Za-z0-9+/`, `=` padding), **not** the
// URL-safe variant, because that is what the wire uses. Decoding accepts
// whitespace and rejects anything else rather than skipping it: a decoder that
// quietly drops invalid characters turns a truncated detail into a shorter but
// still-plausible one, which is how a `reason` goes missing without an error.

#ifndef LOAMS_BASE64_HPP
#define LOAMS_BASE64_HPP

#include <optional>
#include <string>
#include <string_view>

namespace loams {

/// Encodes bytes as standard base64 with `=` padding. Empty input encodes to
/// the empty string, which is what a detail with no bytes must produce.
std::string Base64Encode(std::string_view bytes);

/// Decodes standard base64. Returns `std::nullopt` for input that is not valid
/// base64 — a wrong length, a character outside the alphabet, or padding in the
/// middle. Whitespace between groups is ignored, because HTTP header folding and
/// line-wrapped base64 both put it there and neither changes the value.
std::optional<std::string> Base64Decode(std::string_view text);

/// Hex, lower case, for the request ids and cursors a test compares.
std::string ToHex(std::string_view bytes);

/// The reverse, for reading a recorded fixture's `bodyBase64`.
std::optional<std::string> FromHex(std::string_view text);

}  // namespace loams

#endif  // LOAMS_BASE64_HPP