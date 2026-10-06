// `cpp_pagination_iterator`.
//
// Runtime contract R6: "A paged call is `page_size` in, `next_page_token` out, and
// the facade exposes both the raw page call and an iterator that follows the tokens
// to the end and yields **items**, not pages. One function serves every paged RPC,
// because the generated binding names the two fields."
//
// The half that can be reached end to end is a **stub**, and the manifest says why
// in its own words for R6: "**No paged RPC exists on any server**, so there is
// nothing to record… R6 is pinned against a stub; the end-to-end half arrives with
// API1 Task 2. Marked `required: false` precisely so no language is blocked on it."
// A fixture for an RPC the server does not serve would test the stub rather than
// the SDK.
//
// What is pinned here is the SDK's half, and each of these is a way the threading
// goes wrong:
//
//   - the token from page N goes into request N+1, so nothing is fetched twice and
//     nothing is skipped;
//   - the iterator yields **items**, not pages: a page of three items is three
//     `Next()` calls, and a caller that got pages would have to unwrap them itself;
//   - the stop condition is an **empty** `next_page_token`, not a short page — a
//     short page is legal and stopping on one truncates the list;
//   - `page_size` is the caller's choice and the iterator does not overwrite it;
//   - a binding that names no pagination is **refused at construction**, because an
//     iterator over an unpaged call silently returns one page and looks like a
//     complete list;
//   - a failure stops iteration and is reported through `Error()`, because an error
//     that a caller skips looks exactly like the end of the list.

#include "support.hpp"

#include <memory>
#include <string>
#include <vector>

#include "loams/approvals/v1/approvals.pb.h"

namespace {

using namespace loams;
using namespace loams_test;

/// The paged binding the corpus's `ListApprovals` declares: both AIP-158 fields,
/// named by the binding so one function serves every paged RPC.
MethodBinding ApprovalsBinding() {
  MethodBinding binding;
  binding.rpc = "loams.approvals.v1.ApprovalService/ListApprovals";
  binding.retry_class = RetryClass::kSafe;
  binding.items_field = "approvals";
  binding.next_page_token_field = "next_page_token";
  return binding;
}

/// One recorded page: the items it returns and the token it hands back.
struct Page {
  std::vector<std::string> ids;
  std::string next_token;
};

/// A fetcher that serves a fixed sequence of pages and records every request, so
/// the test can assert on the `page_token` each one carried.
class PageScript {
 public:
  explicit PageScript(std::vector<Page> pages) : pages_(std::move(pages)) {}

  void Fetch(const google::protobuf::Message& request, google::protobuf::Message* response) {
    auto* const typed = dynamic_cast<const approvals::v1::ListApprovalsRequest*>(&request);
    auto* const into = dynamic_cast<approvals::v1::ListApprovalsResponse*>(response);
    if (typed == nullptr || into == nullptr) {
      ThrowInternal("loams.approvals.v1.ApprovalService/ListApprovals",
                    "the page fetcher was handed the wrong message types");
    }
    requests.push_back({typed->page_token(), typed->page_size()});
    if (next_page >= pages_.size()) {
      throw std::runtime_error("the script ran out of pages");
    }
    const Page& page = pages_[next_page];
    ++next_page;
    for (const std::string& id : page.ids) {
      approvals::v1::Approval approval;
      approval.set_id(id);
      approval.set_revision(1);
      approval.set_state(approvals::v1::APPROVAL_STATE_PENDING);
      *into->add_approvals() = approval;
    }
    if (!page.next_token.empty()) {
      into->set_next_page_token(page.next_token);
    }
  }

  std::vector<std::pair<std::string, int>> requests;

 private:
  std::vector<Page> pages_;
  std::size_t next_page = 0;
};

std::unique_ptr<approvals::v1::ListApprovalsRequest> MakeRequest() {
  return std::make_unique<approvals::v1::ListApprovalsRequest>();
}

}  // namespace

int main() {
  // --- The iterator yields items, not pages, and threads the tokens --------------
  {
    PageScript script({
        // Page 1: two items, and a token.
        {{"apr_1", "apr_2"}, "page-2"},
        // Page 2: one item, and **a short page that is not the last**: one item is
        // a legal page size, so the iterator must not stop here.
        {{"apr_3"}, "page-3"},
        // Page 3: two items and an **empty** token: the end.
        {{"apr_4", "apr_5"}, ""},
    });
    PageIterator iterator = PageIterator::For(ApprovalsBinding(), [&script](const google::protobuf::Message& request,
                                                                           google::protobuf::Message* response) {
      script.Fetch(request, response);
    }, MakeRequest(), std::make_unique<approvals::v1::ListApprovalsResponse>());

    std::vector<std::string> seen;
    approvals::v1::Approval item;
    while (iterator.Next(&item)) {
      seen.push_back(item.id());
    }
    LOAMS_CHECK(!iterator.Error(), "the iteration should not have failed");
    // Five items over three pages: an iterator that yielded **pages** would have
    // produced three.
    LOAMS_REQUIRE(seen.size() == 5, "the iterator should yield five items, saw " + std::to_string(seen.size()));
    for (std::size_t at = 0; at < seen.size(); ++at) {
      LOAMS_CHECK_EQ(seen[at], "apr_" + std::to_string(at + 1), "item " + std::to_string(at));
    }
    LOAMS_CHECK_EQ(iterator.PagesFetched(), 3, "three pages were fetched");
    // And the tokens: the first request carries none, and each later one carries
    // the previous page's token.
    LOAMS_REQUIRE(script.requests.size() == 3, "three page requests");
    LOAMS_CHECK_EQ(script.requests[0].first, std::string(), "the first request carries no page_token");
    LOAMS_CHECK_EQ(script.requests[1].first, std::string("page-2"), "the second resumes from page 1's token");
    LOAMS_CHECK_EQ(script.requests[2].first, std::string("page-3"), "the third resumes from page 2's token");
  }

  // --- An empty first page ends the list ------------------------------------------
  {
    PageScript script({{{}, ""}});
    PageIterator iterator = PageIterator::For(ApprovalsBinding(), [&script](const google::protobuf::Message& request,
                                                                           google::protobuf::Message* response) {
      script.Fetch(request, response);
    }, MakeRequest(), std::make_unique<approvals::v1::ListApprovalsResponse>());
    approvals::v1::Approval item;
    LOAMS_CHECK(!iterator.Next(&item), "an empty page yields no items");
    LOAMS_CHECK(!iterator.Error(), "and it is not an error: an empty list is a list");
    LOAMS_CHECK_EQ(iterator.PagesFetched(), 1, "one page was fetched");
  }

  // --- A page with items but no token ends the list ---------------------------------
  {
    // The stop condition is an **empty** `next_page_token`, not a short page. A
    // server that returns three items and no token has nothing more.
    PageScript script({{{"apr_1", "apr_2", "apr_3"}, ""}});
    PageIterator iterator = PageIterator::For(ApprovalsBinding(), [&script](const google::protobuf::Message& request,
                                                                           google::protobuf::Message* response) {
      script.Fetch(request, response);
    }, MakeRequest(), std::make_unique<approvals::v1::ListApprovalsResponse>());
    approvals::v1::Approval item;
    std::size_t seen = 0;
    while (iterator.Next(&item)) {
      ++seen;
    }
    LOAMS_CHECK_EQ(seen, std::size_t{3}, "the three items of the only page");
    LOAMS_CHECK_EQ(iterator.PagesFetched(), 1, "and no second request, because there was no token");
  }

  // --- `page_size` is the caller's, and the iterator does not touch it ---------------
  {
    PageScript script({{{"apr_1"}, "next"}, {{"apr_2"}, ""}});
    auto request = MakeRequest();
    request->set_page_size(7);
    PageIterator iterator = PageIterator::For(ApprovalsBinding(), [&script](const google::protobuf::Message& request,
                                                                           google::protobuf::Message* response) {
      script.Fetch(request, response);
    }, std::move(request), std::make_unique<approvals::v1::ListApprovalsResponse>());
    approvals::v1::Approval item;
    while (iterator.Next(&item)) {
    }
    LOAMS_CHECK(!iterator.Error(), "the iteration should not have failed");
    LOAMS_REQUIRE(script.requests.size() == 2, "two page requests");
    // Both requests kept the caller's `page_size`. An iterator that defaulted it
    // would silently change how much the caller asked for.
    LOAMS_CHECK_EQ(script.requests[0].second, 7, "the first request kept page_size");
    LOAMS_CHECK_EQ(script.requests[1].second, 7, "the second kept it too");
  }

  // --- A failure stops the iteration and is reported ---------------------------------
  {
    // An error a caller skips looks **exactly** like the end of the list, which is
    // why `Error()` exists and why this is asserted: the second page fails.
    PageScript script({{{"apr_1"}, "page-2"}, {{"apr_2"}, ""}});
    bool fail_second = true;
    PageIterator iterator = PageIterator::For(ApprovalsBinding(),
                                              [&script, &fail_second](const google::protobuf::Message& request,
                                                                     google::protobuf::Message* response) {
      if (fail_second) {
        // Let the first page through, then fail.
        static int seen = 0;
        if (seen++ > 0) {
          throw std::runtime_error("the page could not be fetched");
        }
      }
      script.Fetch(request, response);
    }, MakeRequest(), std::make_unique<approvals::v1::ListApprovalsResponse>());
    approvals::v1::Approval item;
    std::size_t seen = 0;
    while (iterator.Next(&item)) {
      ++seen;
    }
    LOAMS_CHECK_EQ(seen, std::size_t{1}, "the first page's item arrived, and then the iteration stopped");
    LOAMS_REQUIRE(iterator.Error() != nullptr, "the failure must be reported through Error()");
    if (iterator.Error()) {
      bool threw = false;
      try {
        std::rethrow_exception(iterator.Error());
      } catch (const std::runtime_error&) {
        threw = true;
      }
      LOAMS_CHECK(threw, "the reported error is the one that happened");
    }
    fail_second = false;
  }

  // --- An unpaged binding is refused at construction ---------------------------------
  {
    // The failure this prevents: an iterator over an unpaged call returns exactly
    // one page, which a caller reads as a complete list. Failing **before** any
    // request is spent is the only point at which the mistake is still cheap.
    MethodBinding unpaged;
    unpaged.rpc = "loams.instance.v1.InstanceService/GetInstance";
    LOAMS_CHECK(!unpaged.Paged(), "GetInstance is not paged");
    LOAMS_CHECK_THROWS(
        PageIterator::For(unpaged, [](const google::protobuf::Message&, google::protobuf::Message*) {},
                          std::make_unique<approvals::v1::ListApprovalsRequest>(),
                          std::make_unique<approvals::v1::ListApprovalsResponse>()),
        InternalError, "an iterator over an unpaged binding should be refused");

    // And the bound binding **is** paged, by the names the binding carries.
    LOAMS_CHECK(ApprovalsBinding().Paged(), "ListApprovals is paged by the binding's names");
    LOAMS_CHECK_EQ(ApprovalsBinding().items_field, std::string("approvals"), "the items field");
    LOAMS_CHECK_EQ(ApprovalsBinding().next_page_token_field, std::string("next_page_token"), "the token field");
    // The fields exist on the schema the binding names, which is what makes the
    // iterator's reflection-based reading correct rather than a guess.
    const google::protobuf::Descriptor* const response =
        approvals::v1::ListApprovalsResponse::descriptor();
    LOAMS_CHECK(response->FindFieldByName("approvals") != nullptr, "the response has the field the binding names");
    LOAMS_CHECK(response->FindFieldByName("next_page_token") != nullptr, "and the token field");
    LOAMS_CHECK(approvals::v1::ListApprovalsRequest::descriptor()->FindFieldByName("page_token") != nullptr,
                "the request takes a page_token");
    LOAMS_CHECK(approvals::v1::ListApprovalsRequest::descriptor()->FindFieldByName("page_size") != nullptr,
                "and a page_size");
  }

  // --- A binding naming a field the response does not have is reported ----------------
  {
    MethodBinding wrong = ApprovalsBinding();
    wrong.items_field = "nonexistent_field";
    PageScript script({{{"apr_1"}, ""}});
    PageIterator iterator = PageIterator::For(wrong, [&script](const google::protobuf::Message& request,
                                                               google::protobuf::Message* response) {
      script.Fetch(request, response);
    }, MakeRequest(), std::make_unique<approvals::v1::ListApprovalsResponse>());
    approvals::v1::Approval item;
    LOAMS_CHECK(!iterator.Next(&item), "a binding naming a missing field yields nothing");
    LOAMS_CHECK(iterator.Error() != nullptr, "and reports why, rather than looking like an empty list");
  }

  // --- The raw page call stays reachable --------------------------------------------
  {
    // The iterator is the convenience; a caller that wants pages, or wants to stop
  // after one, does not have to use it. So the module keeps the raw call, and it
    // works on the same recorded `ListApprovals` — which the corpus records as
    // `mock_status_list_is_not_paged`, the recording that pins the fact that no
    // paged RPC exists yet.
    const std::vector<Recorded> corpus = ReadCorpus();
    const Recorded* entry = nullptr;
    for (const Recorded& candidate : corpus) {
      if (candidate.name == "mock_status_list_is_not_paged") {
        entry = &candidate;
      }
    }
    if (entry != nullptr && !entry->steps.empty()) {
      const RecordedStep& step = entry->steps.front();
      // The recording declares both fields and honours neither: it answers with a
      // list and **no** `nextPageToken`.
      LOAMS_CHECK(step.response_body.find("nextPageToken") == std::string::npos,
                  "the recording pins that ListApprovals returns no next_page_token");
    } else {
      // Not a failure: this fixture is `required: false`, and its absence is the
      // corpus's business rather than this SDK's.
      std::cout << "  note: the corpus has no mock_status_list_is_not_paged; the R6 end-to-end half is a stub "
                   "either way\n";
    }

    ScriptedTransport transport;
    ScriptedTransport::Answer answer;
    answer.status = 200;
    answer.content_type = "application/json";
    // proto3 JSON, as `application/json` carries it.
    answer.body = R"({"approvals":[{"id":"apr_1","revision":"1","state":"APPROVAL_STATE_PENDING"}]})";
    transport.AddAnswer(answer);

    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.content_type = ContentType::kJson;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    approvals::v1::ListApprovalsRequest request;
    approvals::v1::ListApprovalsResponse response;
    loams->Approvals()->ListApprovals(&request, &response);
    LOAMS_CHECK_EQ(response.approvals_size(), 1, "the raw page call returned the page");
    LOAMS_CHECK_EQ(response.next_page_token(), std::string(),
                   "and no token, which is why the end-to-end half of R6 is a stub");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1}, "one request, not a walk of the tokens");
  }

  // --- R4: the session consistency store, and why it is off by default -------------
  {
    // R4's store is a separate clause and it is **off by default**: a caller who
    // has not asked for read-your-writes must not get them.
    Options plain;
    plain.endpoint = "http://127.0.0.1:1";
    std::shared_ptr<Loams> loams = MakeLoams(std::move(plain));
    LOAMS_CHECK(!loams->Runtime()->SessionToken().Present(),
                "the session token is absent unless the caller opted in");

    Options opted_in;
    opted_in.endpoint = "http://127.0.0.1:1";
    opted_in.session_consistency = true;
    std::shared_ptr<Loams> reading_own_writes = MakeLoams(std::move(opted_in));
    LOAMS_CHECK(!reading_own_writes->Runtime()->SessionToken().Present(),
                "an opted-in store with no writes yet holds no token");

    // The store itself: one token folds in, and **two different** tokens meeting is
    // an error rather than a merge — the token's encoding is not in the protos yet,
    // and a merged token that is wrong reads stale data, which is worse than a
    // failure.
    SessionTokenStore store;
    LOAMS_CHECK(!store.Has(), "an empty store holds nothing");
    store.Observe(ConsistencyToken("v1:abc"));
    LOAMS_CHECK(store.Has(), "a token folds in");
    LOAMS_CHECK_EQ(store.Merged().Value(), std::string("v1:abc"), "and reads back");
    // The same token again is not a conflict: a server answering two writes with the
    // same token is normal.
    store.Observe(ConsistencyToken("v1:abc"));
    LOAMS_CHECK_EQ(store.Merged().Value(), std::string("v1:abc"), "the same token again is fine");
    // An absent token is ignored rather than clearing the good one.
    store.Observe(ConsistencyToken());
    LOAMS_CHECK_EQ(store.Merged().Value(), std::string("v1:abc"),
                   "an absent token must not clear the one already held");
    // Two **different** tokens is a conflict.
    bool conflicted = false;
    try {
      store.Observe(ConsistencyToken("v1:def"));
    } catch (const ConsistencyConflict& error) {
      conflicted = true;
      LOAMS_CHECK(std::string(error.what()).find("v1:abc") != std::string::npos,
                  "the conflict should name the token it holds");
      LOAMS_CHECK(std::string(error.what()).find("v1:def") != std::string::npos,
                  "and the one that arrived");
    }
    LOAMS_CHECK(conflicted, "two different tokens must be reported rather than merged into a wrong one");
    LOAMS_CHECK_EQ(store.Merged().Value(), std::string("v1:abc"), "and the held token is unchanged");
    store.Clear();
    LOAMS_CHECK(!store.Has(), "Clear forgets it");
  }

  return Finish("cpp_pagination_iterator");
}