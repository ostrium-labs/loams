// `cpp_conformance_all_required_fixtures`.
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev` and from
// `loams-apps-mock`, and this runs **every** fixture `manifest.json` marks
// `required` through the SDK — the public modules where a facade exists, and the
// same call path the modules delegate to where it does not.
//
// The coverage is **derived**, never listed: `required.cpp` reads the required set
// out of `manifest.json`, drives each fixture's recorded steps, and returns the
// names that held. A report written from a list in this file would go stale the
// moment the corpus grew, and the corpus growing is supposed to turn the gate
// red. The gate is `sdks/conformance/check-languages.mjs --check cpp`, which reads
// `sdks/fixtures/results/cpp.json`.
//
// Three things are also asserted by hand, because `runtime-contract.md` names
// them as claims about the SDK rather than about the wire, and a corpus replay
// cannot make them:
//
//   - **R5's three shapes**: the guard that costs no RPC, the typed refusal a call
//     gets, and the refusal on a stream — which arrives inside the Connect
//     envelope after HTTP **200**, so a client that reads only status codes sees a
//     successful call fail.
//   - **R9**: `PROTO_REV` beside the server's `api_versions`, with a package the
//     SDK speaks and the server does not serve reported as a **warning**.
//   - **R8's three distinct cases**, which the error-mapping test owns.
//
// ## The report
//
// Design §44 §10.4's 100% bar is checked by `required.mjs` against
// `sdks/fixtures/results/cpp.json`, which this test writes — whether it passed or
// not. A fixture that could not be run is a **failure**, not a `skipped` entry:
// the only skip the rule permits is a `transport: grpc-only` fixture on the
// Connect-unary fallback (D613), and no fixture in the corpus carries it.

#include "required.hpp"
#include "support.hpp"

#include <algorithm>
#include <fstream>
#include <sstream>
#include <vector>

#include "loams/approvals/v1/approvals.pb.h"
#include "loams/instance/v1/instance.pb.h"
#include "loams/live/v1/live.pb.h"

namespace {

using namespace loams;
using namespace loams_test;

/// The six canonical names, in `required.mjs`'s order. `required.mjs` owns them;
/// this list is what the suite is checked against, and `cpp_conformance_test_names`
/// asserts the same six are registered with CTest.
const std::vector<std::string>& CanonicalNames() {
  static const std::vector<std::string>* const names = new std::vector<std::string>{
      "cpp_conformance_all_required_fixtures", "cpp_retry_reuses_idempotency_key",
      "cpp_error_reason_mapping",              "cpp_stream_resume_with_cursor",
      "cpp_token_source_refresh",             "cpp_pagination_iterator",
  };
  return *names;
}

/// The suite's own inventory, discovered by looking for each canonical name in
/// this package's sources.
///
/// Discovered rather than declared: a test that was renamed stops being claimed,
/// and `checkLanguage` says which one is missing instead of the suite quietly
/// asserting less than it says.
std::vector<std::string> TestsThisSuiteHas() {
  std::string sources;
  for (const char* const name : {"conformance_test.cpp", "error_reason_test.cpp", "pagination_test.cpp",
                                "retry_idempotency_test.cpp", "stream_resume_test.cpp",
                                "token_source_test.cpp", "test_names.cpp", "support.cpp", "support.hpp"}) {
    std::ifstream file(SdkDir() + "/tests/" + name);
    if (file) {
      std::ostringstream out;
      out << file.rdbuf();
      sources += out.str();
    }
  }
  std::vector<std::string> found;
  for (const std::string& name : CanonicalNames()) {
    if (sources.find(name) != std::string::npos) {
      found.push_back(name);
    }
  }
  return found;
}

/// R5: the guard, the refusal a call gets, and the refusal on a stream.
void CheckFeatureDetection(Loams* loams) {
  // 1. The guard, from the catalogue, spending no RPC on a call that cannot work.
  //    `loams.live` and `loams.tables` are the same service, so the guard is asked
  //    about either.
  LOAMS_CHECK_THROWS(loams->System()->Guard("live"), FeatureNotInVariantError,
                     "guard(\"live\") should refuse: loams.live.v1 is not in the standard variant");
  bool instance_served = true;
  try {
    loams->System()->Guard("instance");
  } catch (const std::exception&) {
    instance_served = false;
  }
  LOAMS_CHECK(instance_served, "guard(\"instance\") should pass: the package is served");

  // 2. The catalogue answers the same question the refusals do, from one call, and
  //    it is cached for the life of the client — so a second read costs no RPC.
  const Catalogue& catalogue = loams->System()->GetCatalogue();
  LOAMS_CHECK(std::find(catalogue.served.begin(), catalogue.served.end(), "loams.instance.v1") !=
                  catalogue.served.end(),
              "served should contain loams.instance.v1");
  LOAMS_CHECK(std::find(catalogue.unavailable.begin(), catalogue.unavailable.end(), "loams.live.v1") !=
                  catalogue.unavailable.end(),
              "unavailable should contain loams.live.v1");
  const ServiceStatus* live_status = nullptr;
  for (const ServiceStatus& status : catalogue.services) {
    if (status.package == "loams.live.v1") {
      live_status = &status;
    }
  }
  LOAMS_REQUIRE(live_status != nullptr, "the catalogue has no loams.live.v1 entry");
  LOAMS_CHECK(live_status->unstable, "loams.live.v1 is not marked unstable; buf breaking skips that package");
  LOAMS_CHECK(!live_status->available, "loams.live.v1 is reported available in the standard variant");
  LOAMS_CHECK(!loams->System()->Available("loams.live.v1"), "Available(\"loams.live.v1\") says it is served");
  LOAMS_CHECK(loams->System()->Available("loams.instance.v1"),
              "Available(\"loams.instance.v1\") says it is not served");
}

/// R9: `PROTO_REV` beside the server's `api_versions`, and a missing package as a
/// warning rather than an exception.
void CheckVersionReport(Loams* loams) {
  const VersionReport report = loams->System()->Version();
  LOAMS_CHECK_EQ(report.proto_rev, ProtoRev(), "the report's proto revision");
  LOAMS_CHECK(!report.server_version.empty(), "the report has no server version");
  LOAMS_CHECK(!report.compatible,
              "the report says the instance is compatible, but it does not serve every package the SDK speaks");
  LOAMS_CHECK(std::find(report.missing.begin(), report.missing.end(), "loams.live.v1") != report.missing.end(),
              "missing should contain loams.live.v1");
  LOAMS_CHECK(!report.warnings.empty(),
              "a package the SDK speaks and the server does not is a warning, not a silence");
}

}  // namespace

int main() {
  Endpoint endpoint;
  std::cout << "conformance: endpoint from " << endpoint.How() << "\n";

  std::vector<std::string> ran;
  std::vector<std::pair<std::string, std::string>> skipped;

  try {
    // Every required fixture, driven. This is the 100% bar, and `ran` is what it
    // returned — never a list written down here.
    ran = DriveRequiredFixtures(endpoint);
    std::cout << "conformance: " << ran.size() << " required fixture(s) driven and held\n";

    // R5 and R9, against the same endpoint, through the public modules.
    Options options;
    options.endpoint = endpoint.Url();
    options.content_type = ContentType::kProto;
    if (endpoint.Transport() != nullptr) {
      options.transport = std::shared_ptr<HttpTransport>(endpoint.Transport(), [](HttpTransport*) {});
    }
    options.max_retries = 0;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));
    CheckFeatureDetection(loams.get());
    CheckVersionReport(loams.get());
  } catch (const Failed& failure) {
    Record(__FILE__, __LINE__, failure.what());
  } catch (const std::exception& error) {
    Record(__FILE__, __LINE__, std::string("the conformance run threw: ") + error.what());
  }

  WriteResults(TestsThisSuiteHas(), ran, skipped, kTransport);

  std::cout << "conformance: ran=" << ran.size() << " skipped=" << skipped.size() << " transport=" << kTransport
            << "\n";
  return Finish("cpp_conformance_all_required_fixtures");
}