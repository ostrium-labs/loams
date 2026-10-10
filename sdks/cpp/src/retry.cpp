// Retry policy and backoff, as `retry.hpp` documents.

#include "loams/retry.hpp"

#include <algorithm>
#include <random>

namespace loams {
namespace {

/// One thread's generator. `std::mt19937` seeded from `std::random_device`, which
/// is what makes the jitter **full jitter** rather than a fixed sequence every
/// process shares: a client that retried at `100ms, 137ms, 4ms` on every run
/// would still collide with every other client doing the same.
///
/// A generator per call would be a syscall per retry for no benefit, and a
/// global one would need a lock on every attempt. Per-thread is both cheap and
/// independent, which is the property jitter is for.
std::mt19937& Generator() {
  static thread_local std::mt19937 generator = [] {
    std::random_device seed;
    return std::mt19937(seed());
  }();
  return generator;
}

}  // namespace

bool IsRetryableCode(Code code) {
  // D610: exactly these three. `aborted` is deliberately absent — a concurrent
  // write won, and retrying immediately is how two clients fight over the same
  // row. `internal` and `data_loss` are absent because a retry cannot fix them.
  return code == Code::kUnavailable || code == Code::kDeadlineExceeded || code == Code::kResourceExhausted;
}

int BackoffMs(int attempt, int server_delay_ms) {
  if (server_delay_ms > 0) {
    // A server-sent `RetryInfo.retry_delay` replaces the computed backoff, up to
    // 30 s (R2). No proto carries one yet, so nothing reaches this today; the
    // path exists so the numbers are already right when a proto does.
    return std::min(server_delay_ms, kMaxServerDelayMs);
  }
  if (attempt < 0) {
    attempt = 0;
  }
  int ceiling = kMaxDelayMs;
  // `kBaseDelayMs << attempt` is a signed shift. A caller that passed 64 to mean
  // "a lot" would shift into the sign bit and produce a **negative** ceiling,
  // and `uniform_int_distribution(0, negative)` is undefined behaviour. Clamped
  // at 16, past which `100 << 16` is already far above the 2 s cap.
  if (attempt < 16) {
    const int grown = kBaseDelayMs << attempt;
    if (grown < ceiling) {
      ceiling = grown;
    }
  }
  std::uniform_int_distribution<int> jitter(0, ceiling);
  return jitter(Generator());
}

bool ShouldRetry(const std::exception& error, bool retry_safe, int attempt, int max_retries) {
  if (attempt >= max_retries) {
    return false;
  }
  if (!retry_safe) {
    return false;
  }
  return IsRetryableCode(CodeOf(error));
}

}  // namespace loams