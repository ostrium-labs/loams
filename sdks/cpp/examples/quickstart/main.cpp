// The C++ quickstart.
//
// Compiled by `sdks/cpp/CMakeLists.txt`, so a change that breaks it breaks the
// build rather than sitting in the tree broken until someone runs it by hand.
//
// It runs against a real instance:
//     export LOAMS_ENDPOINT=http://127.0.0.1:8080
//     export LOAMS_API_KEY=...            # optional; GetInstance needs no auth
//     ./quickstart
//
// What it shows, in the order the runtime contract names:
//   - `GetInstance`, the first call any client makes, with no auth;
//   - feature detection from the catalogue, before calling something that may not
//     be there (R5);
//   - a structured-reason error, branched on by type rather than by message (R8);
//   - a paged iterator, which yields **items** rather than pages (R6).

#include <cstdlib>
#include <iostream>
#include <string>

#include "loams/loams.hpp"

namespace {

/// Reads an environment variable, or a fallback.
std::string Env(const char* name, const std::string& fallback) {
  const char* const value = std::getenv(name);
  return (value == nullptr || value[0] == '\0') ? fallback : std::string(value);
}

}  // namespace

int main() {
  const std::string endpoint = Env("LOAMS_ENDPOINT", "http://127.0.0.1:8080");

  // One client. `EnvToken()` reads `LOAMS_API_KEY`, then `LOAMS_TOKEN`, then sends
  // nothing — which is what `GetInstance` needs, and what keeps this example
  // runnable against a fresh `loams dev`.
  auto loams = loams::MakeLoams({.endpoint = endpoint, .token_source = loams::EnvToken()});

  // 1. What this instance is. Needs no auth, which is why it is first.
  try {
    loams::instance::v1::GetInstanceResponse info;
    loams->Instance()->GetInstance(&info);
    std::cout << "instance " << info.name() << " " << info.server_version() << "\n";
    std::cout << "  api_versions:";
    for (const std::string& package : info.api_versions()) {
      std::cout << " " << package;
    }
    std::cout << "\n";
  } catch (const loams::LoamsError& error) {
    // The `reason` is the stable branch; the message is for a person.
    std::cerr << "GetInstance failed: code=" << loams::ToString(error.CodeValue())
              << " reason=" << (error.UnknownReason().empty() ? std::string(loams::ToString(error.ReasonValue()))
                                                             : error.UnknownReason())
              << " rpc=" << error.Rpc() << "\n"
              << "  " << error.what() << "\n";
    if (!error.Hint().empty()) {
      std::cerr << "  hint: " << error.Hint() << "\n";
    }
    return 1;
  }

  // 2. Feature detection **before** calling. A package this binary does not serve
  //    answers `unimplemented` with reason `feature_not_in_variant`; the guard asks
  //    the catalogue instead, and costs no RPC once it is cached.
  std::cout << "  live served: " << (loams->System()->Available("live") ? "yes" : "no") << "\n";
  try {
    loams->System()->Guard("live");
    std::cout << "  the live module is available\n";
  } catch (const loams::FeatureNotInVariantError& error) {
    // The **same** type a call would raise, so one `catch` covers "the guard said
    // no" and "the server refused".
    std::cout << "  the live module is not in this variant (variant="
              << (error.Variant().empty() ? std::string("<not reported>") : error.Variant()) << ")\n";
  }

  // 3. A structured-reason error, branched on by type. On a build with no
  //    authentication yet this is `unimplemented` with reason `not_implemented`.
  try {
    loams::instance::v1::WhoAmIResponse who;
    loams->Instance()->WhoAmI(&who);
    std::cout << "  signed in as " << who.principal().id() << "\n";
  } catch (const loams::TokenExpiredError&) {
    std::cerr << "the token expired and one refresh did not fix it\n";
  } catch (const loams::UnauthenticatedError& error) {
    std::cerr << "not signed in: reason="
              << (error.UnknownReason().empty() ? std::string(loams::ToString(error.ReasonValue()))
                                                 : error.UnknownReason())
              << "\n";
  } catch (const loams::LoamsError& error) {
    std::cerr << "WhoAmI failed: code=" << loams::ToString(error.CodeValue())
              << " reason=" << loams::ToString(error.ReasonValue()) << "\n";
  }

  // 4. A paged list, through the iterator. It yields **items**, not pages, and it
  //    follows the tokens to the end. Note that no RPC in the public protos is
  //    paged yet (`ListCollections` arrives with API1 Task 2), so against a current
  //    server this returns one page and stops — which is the server's state, not a
  //    mistake in the SDK.
  try {
    auto fetch = [&loams](const google::protobuf::Message& request, google::protobuf::Message* response) {
      // The iterator hands the request by reference; the module method takes a
      // pointer, so a copy is made here rather than the module being given a
      // second, near-identical entry point that only exists for the iterator.
      const auto& typed = static_cast<const loams::approvals::v1::ListApprovalsRequest&>(request);
      auto* const copy = new loams::approvals::v1::ListApprovalsRequest(typed);
      loams->Approvals()->ListApprovals(copy, static_cast<loams::approvals::v1::ListApprovalsResponse*>(response));
      delete copy;
    };
    auto iterator = loams::PageIterator::For(
        loams::BindingFor("loams.approvals.v1.ApprovalService/ListApprovals"), fetch,
        std::make_unique<loams::approvals::v1::ListApprovalsRequest>(),
        std::make_unique<loams::approvals::v1::ListApprovalsResponse>());
    loams::approvals::v1::Approval approval;
    std::size_t count = 0;
    while (iterator.Next(&approval)) {
      std::cout << "  approval " << approval.id() << " is "
                << loams::approvals::v1::ApprovalState_Name(approval.state()) << "\n";
      ++count;
    }
    // `Error()` is not optional to read: a caller who skips it sees a silently
    // short list, which for a paginated list looks exactly like the end of it.
    if (iterator.Error()) {
      std::rethrow_exception(iterator.Error());
    }
    std::cout << "  " << count << " approval(s) over " << iterator.PagesFetched() << " page(s)\n";
  } catch (const loams::LoamsError& error) {
    std::cerr << "listing approvals failed: code=" << loams::ToString(error.CodeValue())
              << " reason=" << loams::ToString(error.ReasonValue()) << "\n";
  }

  std::cout << "proto revision " << loams::ProtoRev() << " against " << loams->Runtime()->Endpoint() << "\n";
  return 0;
}