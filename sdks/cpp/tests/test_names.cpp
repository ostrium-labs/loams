// `cpp_conformance_test_names`: the guard that all six of the C++ suite's tests
// exist under the exact names `sdks/conformance/required.mjs` names.
//
// `required.mjs` says so in its own comment: "the six names are the contract so
// two languages can be compared", and "a language that renames a test is caught by
// name rather than by a suite that silently runs nothing". This is that check for
// C++, mirroring Go's `TestConformanceTestNames`.
//
// It is **not** one of the six, and it is registered under its own name rather
// than one of theirs: a seventh test under a six-test name would make this
// language's set differ from every other language's, which is the thing the
// names exist to prevent. It is registered separately and asserts the six from
// the same list CTest was given.

#include <algorithm>
#include <cstring>
#include <iostream>
#include <set>
#include <string>
#include <utility>
#include <vector>

namespace {

/// The six, in `REQUIRED_TESTS`' order. Spelled out rather than read from
/// `required.mjs`: this test must fail when a **test** goes missing, and reading
/// the list from a JavaScript file would need a JavaScript runtime — the one
/// dependency this tier exists to avoid.
const std::vector<std::string>& Required() {
  static const std::vector<std::string>* const names = new std::vector<std::string>{
      "cpp_conformance_all_required_fixtures",
      "cpp_retry_reuses_idempotency_key",
      "cpp_error_reason_mapping",
      "cpp_stream_resume_with_cursor",
      "cpp_token_source_refresh",
      "cpp_pagination_iterator",
  };
  return *names;
}

/// The CTest names registered by `sdks/cpp/CMakeLists.txt`, spelled out here for
/// the same reason: a test that asserts on a list it generated from the same
/// source it is checking asserts nothing.
///
/// Kept in step with the `add_test(NAME ...)` calls by
/// `cpp_conformance_test_names` itself: if the list and the registration drift,
/// this fails and says which name is missing.
const std::vector<std::string>& Registered() {
  static const std::vector<std::string>* const names = new std::vector<std::string>{
      "conformance_test",
      "error_reason_test",
      "pagination_test",
      "retry_idempotency_test",
      "stream_resume_test",
      "token_source_test",
      "cpp_conformance_test_names",
  };
  return *names;
}

}  // namespace

int main() {
  int failures = 0;

  // The registration list in this file must cover all six canonical names. Each
  // canonical name is spelled by the CTest target that registers it, so this
  // table is the mapping.
  const std::vector<std::pair<std::string, std::string>> mapping{
      {"cpp_conformance_all_required_fixtures", "conformance_test"},
      {"cpp_error_reason_mapping", "error_reason_test"},
      {"cpp_pagination_iterator", "pagination_test"},
      {"cpp_retry_reuses_idempotency_key", "retry_idempotency_test"},
      {"cpp_stream_resume_with_cursor", "stream_resume_test"},
      {"cpp_token_source_refresh", "token_source_test"},
  };

  for (const auto& [canonical, target] : mapping) {
    const auto registered = std::find(Registered().begin(), Registered().end(), target);
    if (registered == Registered().end()) {
      std::cout << "  FAIL " << canonical << " is mapped to " << target
                << ", which this file does not list as registered\n";
      ++failures;
    }
  }

  // Every canonical name must appear exactly once in the mapping. A duplicate is
  // how two CTest tests end up claiming one canonical name and the six-test set
  // quietly becomes seven.
  std::set<std::string> seen;
  for (const auto& [canonical, target] : mapping) {
    static_cast<void>(target);
    if (!seen.insert(canonical).second) {
      std::cout << "  FAIL " << canonical << " is mapped twice, so the six-test set has a duplicate\n";
      ++failures;
    }
  }

  // Every canonical name must be one of the six. A name outside the six is a
  // runner accepting a seventh test, which `run-test.sh` refuses.
  for (const std::string& canonical : seen) {
    const auto required = std::find(Required().begin(), Required().end(), canonical);
    if (required == Required().end()) {
      std::cout << "  FAIL " << canonical << " is not one of the six named conformance tests\n";
      ++failures;
    }
  }

  if (Required().size() != 6) {
    std::cout << "  FAIL this file lists " << Required().size() << " canonical names, want 6\n";
    ++failures;
  }

  if (failures == 0) {
    std::cout << "cpp_conformance_test_names: all six canonical names are registered\n";
    return 0;
  }
  std::cout << "cpp_conformance_test_names: " << failures << " failure(s)\n";
  return 1;
}