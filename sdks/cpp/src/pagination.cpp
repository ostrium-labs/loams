// The pagination iterator, as `pagination.hpp` documents it.

#include "loams/pagination.hpp"

#include <utility>
#include <vector>

#include "loams/error.hpp"

namespace loams {

struct PageIterator::Impl {
  MethodBinding binding;
  PageFetcher fetch;
  std::unique_ptr<google::protobuf::Message> request;
  /// A prototype: the iterator allocates a fresh message of this type per page
  /// and hands it to the fetcher.
  std::unique_ptr<google::protobuf::Message> response_prototype;
  std::unique_ptr<google::protobuf::Message> response;
  std::vector<std::string> items;
  std::size_t at = 0;
  int pages = 0;
  bool fetched_any = false;
  bool done = false;
  std::exception_ptr error;
};

PageIterator::PageIterator(MethodBinding binding, PageFetcher fetch,
                           std::unique_ptr<google::protobuf::Message> request,
                           std::unique_ptr<google::protobuf::Message> response)
    : impl_(std::make_unique<Impl>()) {
  impl_->binding = std::move(binding);
  impl_->fetch = std::move(fetch);
  impl_->request = std::move(request);
  impl_->response_prototype = std::move(response);
}

PageIterator::PageIterator(PageIterator&&) noexcept = default;
PageIterator& PageIterator::operator=(PageIterator&&) noexcept = default;
PageIterator::~PageIterator() = default;

PageIterator PageIterator::For(MethodBinding binding, PageFetcher fetch,
                               std::unique_ptr<google::protobuf::Message> request,
                               std::unique_ptr<google::protobuf::Message> response) {
  if (!binding.Paged()) {
    // Refused at construction rather than at the first `Next`: an iterator over an
    // unpaged call would return exactly one page, which looks to a caller exactly
    // like a complete list. Failing before any request is spent is the only point
    // at which the mistake is still cheap.
    ThrowInternal(binding.rpc, "loams: " + binding.rpc +
                                   " is not a paged call; the binding names no pagination, so an iterator over it "
                                   "would silently return one page and look like a complete list");
  }
  if (!fetch) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s page fetcher is empty");
  }
  if (!request) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s request message is empty");
  }
  if (!response) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s response prototype is empty");
  }
  return PageIterator(std::move(binding), std::move(fetch), std::move(request), std::move(response));
}

bool PageIterator::Next(google::protobuf::Message* out) {
  if (out == nullptr) {
    ThrowInternal(impl_->binding.rpc, "loams: " + impl_->binding.rpc + "'s iterator was given no message to fill");
  }
  if (impl_->done || impl_->error) {
    return false;
  }

  const google::protobuf::Reflection* const reflection = impl_->request->GetReflection();
  const google::protobuf::Descriptor* const descriptor = impl_->request->GetDescriptor();
  const google::protobuf::FieldDescriptor* const page_token =
      descriptor == nullptr ? nullptr : descriptor->FindFieldByName("page_token");
  const google::protobuf::FieldDescriptor* const page_size =
      descriptor == nullptr ? nullptr : descriptor->FindFieldByName("page_size");

  // The first pass has no items to yield: fetch a page.
  for (;;) {
    if (impl_->at < impl_->items.size()) {
      const std::string& item = impl_->items[impl_->at];
      ++impl_->at;
      if (!out->ParseFromString(item)) {
        impl_->error = std::make_exception_ptr(
            TransportError(impl_->binding.rpc, "loams: " + impl_->binding.rpc + "'s item did not parse"));
        return false;
      }
      return true;
    }
    if (impl_->fetched_any && impl_->pages > 0 && impl_->items.empty()) {
      // The last page came back with no `next_page_token`: the end of the list.
      impl_->done = true;
      return false;
    }

    // Ask for the next page. The token from the **previous** response is written
    // into the request, which is the whole of AIP-158's threading and the thing a
    // paged iterator exists to get right.
    if (page_token != nullptr && impl_->fetched_any) {
      const google::protobuf::Reflection* const response_reflection = impl_->response->GetReflection();
      const google::protobuf::Descriptor* const response_descriptor = impl_->response->GetDescriptor();
      const google::protobuf::FieldDescriptor* const next =
          response_descriptor == nullptr ? nullptr
                                         : response_descriptor->FindFieldByName(impl_->binding.next_page_token_field);
      if (next == nullptr) {
        impl_->error = std::make_exception_ptr(InternalError(
            Code::kInternal, impl_->binding.rpc, Reason::kInternal, std::string(), {}, std::string(),
            "loams: " + impl_->binding.rpc + "'s response has no field \"" + impl_->binding.next_page_token_field +
                "\", which the binding named as its next page token",
            std::string()));
        return false;
      }
      const std::string token = response_reflection->GetString(*impl_->response, next);
      if (token.empty()) {
        impl_->done = true;
        return false;
      }
      reflection->SetString(impl_->request.get(), page_token, token);
      if (page_size != nullptr) {
        // Left as the caller set it: `page_size` is the caller's choice and the
        // iterator must not silently turn a "give me everything" page into a
        // fixed size.
      }
    }

    // A **fresh** message per page: the fetcher fills it by assignment, and a
    // reused one would carry the previous page's items into the next read, which
    // looks exactly like a server that ignored the token.
    impl_->response.reset(impl_->response_prototype->New());
    if (impl_->response == nullptr) {
      impl_->error = std::make_exception_ptr(
          TransportError(impl_->binding.rpc, "loams: " + impl_->binding.rpc + "'s response could not be allocated"));
      return false;
    }
    ++impl_->pages;
    try {
      impl_->fetch(*impl_->request, impl_->response.get());
    } catch (...) {
      impl_->error = std::current_exception();
      return false;
    }
    impl_->fetched_any = true;

    // Pull the items out of the response as **serialized messages**, one at a
    // time, so the iterator needs no knowledge of the item type: the response
    // declares a repeated message field, and that is all it reads.
    const google::protobuf::Reflection* const response_reflection = impl_->response->GetReflection();
    const google::protobuf::Descriptor* const response_descriptor = impl_->response->GetDescriptor();
    const google::protobuf::FieldDescriptor* const items_field =
        response_descriptor == nullptr ? nullptr : response_descriptor->FindFieldByName(impl_->binding.items_field);
    if (items_field == nullptr || !items_field->is_repeated() ||
        items_field->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_MESSAGE) {
      impl_->error = std::make_exception_ptr(InternalError(
          Code::kInternal, impl_->binding.rpc, Reason::kInternal, std::string(), {}, std::string(),
          "loams: " + impl_->binding.rpc + "'s response has no repeated message field \"" + impl_->binding.items_field +
              "\", which the binding named as its items",
          std::string()));
      return false;
    }
    impl_->items.clear();
    impl_->at = 0;
    const int count = response_reflection->FieldSize(*impl_->response, items_field);
    impl_->items.reserve(static_cast<std::size_t>(count));
    for (int index = 0; index < count; ++index) {
      std::string serialized;
      // `GetRepeatedMessage` is the indexed accessor and returns a reference;
      // `GetMessage` takes a `MessageFactory*` in that slot and would silently be
      // the wrong overload.
      if (response_reflection->GetRepeatedMessage(*impl_->response, items_field, index)
              .SerializeToString(&serialized)) {
        impl_->items.push_back(std::move(serialized));
      }
    }
  }
}

std::exception_ptr PageIterator::Error() const { return impl_->error; }

int PageIterator::PagesFetched() const { return impl_->pages; }

}  // namespace loams