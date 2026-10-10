// The session consistency store, as `consistency.hpp` documents it.

#include "loams/consistency.hpp"

namespace loams {

void SessionTokenStore::Observe(const ConsistencyToken& token) {
  // An absent token is ignored: a response that carries none is saying nothing
  // about consistency, and treating that as "the token is now empty" would drop
  // a good token.
  if (!token.Present()) {
    return;
  }
  std::lock_guard<std::mutex> const guard(mutex_);
  if (!token_.Present()) {
    token_ = token;
    return;
  }
  if (token_ != token) {
    // Two different tokens meeting is an **error**, not a merge. §44 §7.4 says
    // the merge is "max offset per stream and partition", which needs the `v1:`
    // encoding parsed, and the encoding is not in the protos yet. A store that
    // merged them anyway — say, by keeping the newer string — would produce a
    // token that names a consistency level the caller never asked for, and a
    // silently-wrong consistency token reads stale data, which is worse than a
    // failure.
    throw ConsistencyConflict(token_.Value(), token.Value());
  }
}

ConsistencyToken SessionTokenStore::Merged() const {
  std::lock_guard<std::mutex> const guard(mutex_);
  return token_;
}

void SessionTokenStore::Clear() {
  std::lock_guard<std::mutex> const guard(mutex_);
  token_ = ConsistencyToken();
}

bool SessionTokenStore::Has() const {
  std::lock_guard<std::mutex> const guard(mutex_);
  return token_.Present();
}

}  // namespace loams