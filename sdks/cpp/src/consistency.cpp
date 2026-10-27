// Consistency tokens and the session store, as `consistency.hpp` documents them.

#include "loams/consistency.hpp"

#include <algorithm>
#include <limits>

#include "loams/error.hpp"

namespace loams {
namespace {

constexpr char kPrefix[] = "v1:";

[[noreturn]] void Invalid(const std::string& text) {
  ThrowClientSide(Code::kInvalidArgument, "", "invalid_consistency_token",
                  "loams: invalid consistency token \"" + text + "\"");
}

/// A canonical decimal (no sign, no leading zero except `0`) of at most `max`.
std::optional<std::uint64_t> Canonical(const std::string& text, std::uint64_t max) {
  if (text.empty() || (text.size() > 1 && text[0] == '0')) {
    return std::nullopt;
  }
  std::uint64_t value = 0;
  for (const char c : text) {
    if (c < '0' || c > '9') {
      return std::nullopt;
    }
    const auto digit = static_cast<std::uint64_t>(c - '0');
    if (value > (std::numeric_limits<std::uint64_t>::max() - digit) / 10) {
      return std::nullopt;
    }
    value = value * 10 + digit;
  }
  if (value > max) {
    return std::nullopt;
  }
  return value;
}

std::optional<ConsistencyToken::Item> ParseItem(const std::string& item) {
  if (item.empty() || item[0] != 's') {
    return std::nullopt;
  }
  const std::size_t p = item.find("/p");
  if (p == std::string::npos) {
    return std::nullopt;
  }
  const std::size_t at = item.find('@', p + 2);
  if (at == std::string::npos) {
    return std::nullopt;
  }
  const auto stream = Canonical(item.substr(1, p - 1), std::numeric_limits<std::uint64_t>::max());
  const auto partition = Canonical(item.substr(p + 2, at - p - 2), std::numeric_limits<std::uint32_t>::max());
  const auto offset = Canonical(item.substr(at + 1), std::numeric_limits<std::uint64_t>::max());
  if (!stream || !partition || !offset) {
    return std::nullopt;
  }
  return ConsistencyToken::Item{*stream, static_cast<std::uint32_t>(*partition), *offset};
}

}  // namespace

ConsistencyToken::ConsistencyToken(std::vector<Item> items) : present_(true), items_(std::move(items)) {
  text_ = kPrefix;
  for (std::size_t i = 0; i < items_.size(); ++i) {
    if (i > 0) {
      text_ += ',';
    }
    text_ += "s" + std::to_string(items_[i].stream) + "/p" + std::to_string(items_[i].partition) + "@" +
             std::to_string(items_[i].offset);
  }
}

ConsistencyToken ConsistencyToken::Parse(const std::string& text) {
  if (text.rfind(kPrefix, 0) != 0) {
    Invalid(text);
  }
  const std::string body = text.substr(sizeof(kPrefix) - 1);
  std::vector<Item> items;
  if (!body.empty()) {
    std::size_t start = 0;
    while (true) {
      const std::size_t comma = body.find(',', start);
      const std::optional<Item> item =
          ParseItem(body.substr(start, comma == std::string::npos ? std::string::npos : comma - start));
      if (!item) {
        Invalid(text);
      }
      items.push_back(*item);
      if (comma == std::string::npos) {
        break;
      }
      start = comma + 1;
    }
  }
  return ConsistencyToken(std::move(items));
}

bool ConsistencyToken::Valid(const std::string& text) {
  try {
    Parse(text);
    return true;
  } catch (const LoamsError&) {
    return false;
  }
}

ConsistencyToken ConsistencyToken::Normalized() const {
  if (!present_) {
    return *this;
  }
  std::vector<Item> sorted = items_;
  std::sort(sorted.begin(), sorted.end(), [](const Item& a, const Item& b) {
    if (a.stream != b.stream) return a.stream < b.stream;
    if (a.partition != b.partition) return a.partition < b.partition;
    return a.offset < b.offset;
  });
  std::vector<Item> out;
  for (const Item& item : sorted) {
    if (!out.empty() && out.back().stream == item.stream && out.back().partition == item.partition) {
      out.back() = item;
    } else {
      out.push_back(item);
    }
  }
  return ConsistencyToken(std::move(out));
}

ConsistencyToken ConsistencyToken::Merge(const ConsistencyToken& other) const {
  if (!other.present_) {
    return Normalized();
  }
  if (!present_) {
    return other.Normalized();
  }
  std::vector<Item> all = items_;
  all.insert(all.end(), other.items_.begin(), other.items_.end());
  return ConsistencyToken(std::move(all)).Normalized();
}

std::optional<std::uint64_t> ConsistencyToken::Offset(std::uint64_t stream, std::uint32_t partition) const {
  std::optional<std::uint64_t> best;
  for (const Item& item : items_) {
    if (item.stream == stream && item.partition == partition && (!best || item.offset > *best)) {
      best = item.offset;
    }
  }
  return best;
}

void SessionTokenStore::Observe(const ConsistencyToken& token, const std::string& scope) {
  if (!token.Present()) {
    return;
  }
  std::lock_guard<std::mutex> const guard(mutex_);
  auto found = tokens_.find(scope);
  if (found == tokens_.end()) {
    tokens_.emplace(scope, token.Normalized());
  } else {
    found->second = found->second.Merge(token);
  }
}

void SessionTokenStore::ObserveText(const std::string& text, const std::string& scope) {
  if (text.empty() || !ConsistencyToken::Valid(text)) {
    return;
  }
  Observe(ConsistencyToken::Parse(text), scope);
}

ConsistencyToken SessionTokenStore::Merged(const std::string& scope) const {
  std::lock_guard<std::mutex> const guard(mutex_);
  auto found = tokens_.find(scope);
  return found == tokens_.end() ? ConsistencyToken() : found->second;
}

void SessionTokenStore::Clear() {
  std::lock_guard<std::mutex> const guard(mutex_);
  tokens_.clear();
}

bool SessionTokenStore::Has(const std::string& scope) const {
  std::lock_guard<std::mutex> const guard(mutex_);
  return tokens_.count(scope) > 0;
}

}  // namespace loams
