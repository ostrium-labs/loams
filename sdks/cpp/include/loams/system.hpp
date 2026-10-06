// Feature detection and version reporting (design §44 §4, D600; runtime
// contract R5 and R9).
//
// Two halves of R5, and an SDK needs both.
//
//  1. **Without calling.** `GetInstance.services[]` says which packages this
//     binary carries. One call, no auth, cheap. `Available`, `Served`,
//     `Unavailable` and `Guard` wrap it; the catalogue is cached for the life of
//     the client and **concurrent readers share one in-flight fetch**, so a
//     hundred threads asking at once cost one call.
//  2. **When the caller calls anyway.** Every RPC of an absent package answers
//     `unimplemented` with `reason = feature_not_in_variant` and the variant in
//     `metadata.variant`. The runtime turns that into a
//     `loams::FeatureNotInVariantError`, so the branch is a type and never the
//     package name, which is a proto detail, and never the message.
//
// `Guard` raises the *same* error type, so one `catch` covers "the guard said
// no" and "the server refused", and the guard costs no RPC once the catalogue is
// cached.

#ifndef LOAMS_SYSTEM_HPP
#define LOAMS_SYSTEM_HPP

#include <memory>
#include <mutex>
#include <string>
#include <vector>

#include "loams/call.hpp"

namespace loams {

class Client;
class SystemModule;

/// One package of the API catalogue and what this binary does with it, as
/// `GetInstance.services[]` reports it.
struct ServiceStatus {
  /// The proto package, for example `loams.live.v1`.
  std::string package;
  /// The package's API version, for example `v1`.
  std::string version;
  /// Whether this binary serves the package. False means every one of its RPCs
  /// answers `unimplemented` with reason `feature_not_in_variant`.
  bool available = false;
  /// The fully qualified service names in the package.
  std::vector<std::string> services;
  /// True for a package whose wire contract may still change; `buf breaking`
  /// skips it.
  bool unstable = false;
};

/// The module catalogue, as one call reports it.
struct Catalogue {
  /// The packages this binary serves.
  std::vector<std::string> served;
  /// The packages it knows about and does not serve.
  std::vector<std::string> unavailable;
  /// The packages this SDK speaks that the instance does not list at all — a
  /// package whose services have not been defined yet, which is different from
  /// one that exists and is switched off.
  std::vector<std::string> missing;
  /// Every entry, in the server's order.
  std::vector<ServiceStatus> services;
};

/// What the SDK speaks beside what the server serves (R9).
struct VersionReport {
  /// The proto revision this SDK was generated from.
  std::string proto_rev;
  /// The server's own semver, as `GetInstance` reports it.
  std::string server_version;
  /// The server's `GetInstance.api_versions`: only what it serves, so a package
  /// this SDK speaks and the server does not is missing rather than reported as
  /// available.
  std::vector<std::string> api_versions;
  /// True when every package the SDK speaks is served.
  bool compatible = false;
  /// The SDK's packages the server does not serve.
  std::vector<std::string> missing;
  /// The warnings R9 asks for: a missing package is a **warning, not an
  /// exception**, so they are returned rather than thrown. A caller decides what
  /// a missing module means.
  std::vector<std::string> warnings;
};

/// The module catalogue, feature detection and the version check.
class SystemModule {
 public:
  explicit SystemModule(Client* client);
  ~SystemModule();
  SystemModule(SystemModule&&) noexcept;
  SystemModule& operator=(SystemModule&&) noexcept;
  SystemModule(const SystemModule&) = delete;
  SystemModule& operator=(const SystemModule&) = delete;

  /// The catalogue, fetched once and shared with every concurrent reader.
  /// Throws whatever `GetInstance` throws.
  const Catalogue& GetCatalogue();

  /// Whether a package is served. Accepts either a full package name
  /// (`loams.live.v1`) or a module name (`live`, `tables`), because the guard is
  /// usually reached through a module name and requiring the caller to know the
  /// proto package is exactly the proto detail R5 says not to branch on.
  bool Available(const std::string& package_or_module);

  /// The packages this binary serves.
  std::vector<std::string> Served();

  /// The packages it knows about and does not serve.
  std::vector<std::string> Unavailable();

  /// Throws `loams::FeatureNotInVariantError` when the module is not served, and
  /// returns normally when it is. The **same type** a call gets when the caller
  /// skips the guard, which is what makes one `catch` cover both.
  void Guard(const std::string& module);

  /// `PROTO_REV` beside the server's `api_versions`. A package this SDK speaks
  /// and the server does not is a warning in `VersionReport::warnings`, not a
  /// failure.
  VersionReport Version();

  /// Forgets the cached catalogue, so the next check calls again. Exposed
  /// because a client that outlives an instance's variant change would otherwise
  /// report the old one forever.
  void Invalidate();

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

/// Resolves a module name to the proto package behind it, as the guard's
/// argument resolution needs. `tables` and `live` are two facade names for
/// `loams.live.v1` (design §44 §7.2), which is why both work.
std::string PackageForModule(const std::string& module);

}  // namespace loams

#endif  // LOAMS_SYSTEM_HPP