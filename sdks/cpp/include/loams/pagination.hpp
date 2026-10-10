// Pagination (design §44 §7.4, D617; runtime contract R6).
//
// AIP-158: `page_size` and `page_token` in, `next_page_token` out. A generated
// binding says which two fields those are, so the iterator is one function for
// every paged RPC rather than one per list RPC — and one that works for an RPC
// that does not exist yet, which is the situation today: **no RPC is paged on
// any server** until API1 Task 2 brings `ListCollections`.
//
// In C++ the iterator is a `std::iterator`-shaped value the caller drives:
//
//     loams::PageIterator iterator = client.Collections().ListAll(request);
//     loams::Collection collection;
//     while (iterator.Next(&collection)) {
//       use(collection);
//     }
//     if (auto error = iterator.Error()) {
//       std::rethrow_exception(error);
//     }
//
// which is the same `Next`/`Error` split `loams::MessageStream` makes, and for
// the same reason: an iterator that returned an error would force every caller to
// write the awkward half of the loop, and a caller who skips the check sees a
// silently short page sequence — which for a paginated list looks exactly like
// the end of the list.
//
// **No RPC is paged yet**, so the end-to-end half of `cpp_pagination_iterator`
// runs against a stub transport rather than a fixture: a fixture for an RPC the
// server does not serve would be a test of the stub rather than of the SDK. What
// is pinned is the SDK's half — the token threading, the stop condition, and what
// happens when a binding is not paged.

#ifndef LOAMS_PAGINATION_HPP
#define LOAMS_PAGINATION_HPP

#include <exception>
#include <functional>
#include <memory>
#include <optional>
#include <string>

#include <google/protobuf/message.h>

#include "loams/call.hpp"

namespace loams {

/// Fetches one page. A generated module method satisfies it, so the iterator
/// drives the same code path an application does.
using PageFetcher = std::function<void(const google::protobuf::Message& request, google::protobuf::Message* response)>;

/// Every item of a paged call, following the tokens to the end.
class PageIterator {
 public:
  PageIterator() = default;
  PageIterator(MethodBinding binding, PageFetcher fetch, std::unique_ptr<google::protobuf::Message> request,
               std::unique_ptr<google::protobuf::Message> response);
  PageIterator(PageIterator&&) noexcept;
  PageIterator& operator=(PageIterator&&) noexcept;
  PageIterator(const PageIterator&) = delete;
  PageIterator& operator=(const PageIterator&) = delete;
  ~PageIterator();

  /// The next item. Returns false at the end of the list **or** on failure;
  /// `Error` says which.
  bool Next(google::protobuf::Message* out);

  /// Why iteration stopped, or null if it stopped at the end of the list.
  std::exception_ptr Error() const;

  /// How many pages have been fetched. A caller that wants to bound the work
  /// rather than to stop iterating reads this.
  int PagesFetched() const;

  /// Builds an iterator for a binding whose pagination fields the caller names.
  /// Throws `LoamsError` (`internal`) when `binding` is not paged: an iterator
  /// over an unpaged call would silently return exactly one page, which looks
  /// like a complete list.
  ///
  /// `response` is a prototype — only its **type** is read, and the iterator
  /// allocates a fresh message per page from it. It is a separate argument because
  /// the request's type cannot supply it: `Request::New()` builds another request,
  /// and an iterator that handed a fetcher a `ListApprovalsRequest` where a
  /// `ListApprovalsResponse` was expected would fail every page for a reason that
  /// reads like a wire problem.
  static PageIterator For(MethodBinding binding, PageFetcher fetch,
                          std::unique_ptr<google::protobuf::Message> request,
                          std::unique_ptr<google::protobuf::Message> response);

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace loams

#endif  // LOAMS_PAGINATION_HPP