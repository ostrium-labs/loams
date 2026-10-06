// What this SDK was generated from (design §44 §10.3, D616; runtime contract R9).
//
// An SDK declares the proto revision it was generated from and reports the
// server's `GetInstance.api_versions` beside it. A package the SDK speaks that
// the server does not serve is a **warning, not an exception**: the SDK still
// works for the modules that are there, and the caller decides what a missing
// one means. `loams::system::VersionReport` is that report.
//
// The package list is a property of the SDK, not of the server, so it is a
// constant rather than something read from a descriptor at runtime: a caller
// comparing two SDKs should be able to see the answer in a header.

#ifndef LOAMS_VERSION_HPP
#define LOAMS_VERSION_HPP

#include <string>
#include <vector>

namespace loams {

/// The proto revision these stubs were generated from (§44 §10.3). Also
/// spelled `LOAMS_PROTO_REV` in the environment-based `Env()` token source's
/// sibling helpers, so a script and a binary report the same string.
inline constexpr const char* kProtoRev = "v1";

/// The proto revision, as a `std::string`, for the report and for logs.
const std::string& ProtoRev();

/// Every proto package this SDK speaks, in registry order. R9's report
/// compares the server's `api_versions` against this.
const std::vector<std::string>& ProtoPackages();

/// The user agent this SDK sends: `loams-cpp/<version>`. Sent on every request
/// so an operator reading a server log can tell which client made a call, and
/// so a proxy can route on it.
const std::string& UserAgent();

/// The SDK's own version. Independent of the server's and of the proto
/// revision: a server release never requires an SDK release and vice versa
/// within one API major (§44 §10.3).
const std::string& SdkVersion();

}  // namespace loams

#endif  // LOAMS_VERSION_HPP