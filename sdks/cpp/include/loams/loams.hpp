// The one include an application needs.
//
//     #include <loams/loams.hpp>
//
// Everything the runtime contract names is reachable from here: the client and
// its modules, the typed errors, the token sources, the retry policy, the
// idempotency-key helper, the stream and pagination readers, the feature
// detection and the version report.
//
// The smaller headers exist because a *consumer of a piece* should not have to
// take the whole runtime: a program that only wants `loams::UuidV7` should be
// able to include `loams/idempotency.hpp` and link no transport at all.

#ifndef LOAMS_LOAMS_HPP
#define LOAMS_LOAMS_HPP

#include "loams/base64.hpp"
#include "loams/call.hpp"
#include "loams/client.hpp"
#include "loams/consistency.hpp"
#include "loams/error.hpp"
#include "loams/facade.hpp"
#include "loams/http.hpp"
#include "loams/idempotency.hpp"
#include "loams/pagination.hpp"
#include "loams/reason.hpp"
#include "loams/retry.hpp"
#include "loams/stream.hpp"
#include "loams/system.hpp"
#include "loams/token_source.hpp"
#include "loams/version.hpp"
#include "loams/wire.hpp"

#endif  // LOAMS_LOAMS_HPP