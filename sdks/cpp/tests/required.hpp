// Every required fixture, driven through the SDK (design §44 §10.4, D617).
//
// This is what makes the 100% bar **checkable** for C++, and the rules it
// follows are the ones worth following:
//
// - **The required set is read from `manifest.json`, never written here.** A list
//   in the suite would be a list that goes stale, and a corpus that grows is
//   supposed to turn the gate red.
// - **`ran` is what `DriveRequiredFixtures` returns**, and a fixture is in it only
//   if its own steps were driven *and held*. A fixture whose replay failed is a
//   failure with a name attached, not a line in a report.
// - **The SDK does the work.** The recorded request is decoded into the generated
//   message and the SDK encodes it again, so the fixture server's byte-for-byte
//   comparison of the request is a real check on this SDK's encoder: field order,
//   an enum's spelling, a `uint64` as a string. Replaying the recorded bytes
//   directly would pass no matter what the SDK wrote.
//
// ## What it drives, and what it cannot
//
// The corpus covers two servers. `loams dev` is the primary target. The
// `loams-apps-mock` cases are on services that carry **no**
// `loams.options.v1.module` annotation yet — `ApprovalService` and
// `DeviceService` arrive with API1 Tasks 2–4 — so there is no `loams.approvals`
// facade to call. Those steps go through `Client::Invoke` and
// `Client::OpenStream`, which is what every facade method delegates to: same
// transport, same credentials, same retry decision, same error mapping, with the
// generated module wrapper left off.
//
// One recorded shape the SDK's own keyed path cannot reproduce: **D610 gives
// every mutation an idempotency key**, and seven of the app-mock mutations were
// recorded *without* one. Putting a key on the wire would change the request, and
// the fixture server — correctly — refuses a request that is not the recorded
// one. Those steps are driven with the key deliberately left off, which is decided
// **per step, from the request schema** (see `KeylessMutation`), not written down
// as a list of fixture names.

#ifndef LOAMS_TESTS_REQUIRED_HPP
#define LOAMS_TESTS_REQUIRED_HPP

#include <exception>
#include <optional>
#include <string>
#include <vector>

#include "loams/loams.hpp"
#include "support.hpp"

namespace loams_test {

/// What `manifest.json` says about one fixture.
struct ManifestFixture {
  std::string name;
  std::string kind;
  std::string server;
  std::string file;
  bool required = false;
  std::string transport;
  std::optional<std::string> reason;
};

/// The manifest, which is the authority on the 100% bar.
std::vector<ManifestFixture> Manifest();

/// The fixtures `manifest.json` marks `required`, in manifest order.
std::vector<ManifestFixture> RequiredFixtures();

/// The transport a run used, which is what the report records and what
/// `maySkip` keys off.
///
/// `connect`, **not** `connect-unary`: the one skip the rule permits is a
/// `transport: grpc-only` fixture on the Connect-unary **fallback**, for a host
/// with no gRPC of its own (D613). This suite speaks Connect and gRPC-Web, so
/// there is no fallback and nothing may be skipped on it. A `connect-unary`
/// transport here would make thirteen `mock_*` fixtures silently skippable and
/// the 100% bar would stop meaning anything.
inline constexpr const char* kTransport = "connect";

/// Drives every required fixture and returns the names that **held**.
///
/// Each fixture is driven on its own so one failure names one fixture, and the
/// returned list must not claim the ones that failed alongside the one that did.
std::vector<std::string> DriveRequiredFixtures(const Endpoint& endpoint);

}  // namespace loams_test

#endif  // LOAMS_TESTS_REQUIRED_HPP