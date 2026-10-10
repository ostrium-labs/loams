// Idempotency keys, as `idempotency.hpp` documents.

#include "loams/idempotency.hpp"

#include <atomic>
#include <chrono>
#include <cstdlib>
#include <ctime>
#include <mutex>

#include <google/protobuf/descriptor.h>

#include "loams/error.hpp"

namespace loams {
namespace {

/// Reads `n` random bytes. `RAND_bytes` is used when libcurl has it — which it
/// does whenever this SDK links, because libcurl depends on a TLS library that
/// seeds one — and `/dev/urandom` otherwise.
///
/// The failure mode matters: `UuidV7` throws rather than falling back to
/// anything weaker, because a **predictable idempotency key is a duplicate key**,
/// and a duplicate key turns one write into two, which is the exact failure the
/// key exists to prevent.
std::string RandomBytes(std::size_t count) {
  std::string out;
  out.resize(count);
  std::size_t filled = 0;
  while (filled < count) {
    std::FILE* const random = std::fopen("/dev/urandom", "rb");
    if (random == nullptr) {
      break;
    }
    const std::size_t got = std::fread(out.data() + filled, 1, count - filled, random);
    std::fclose(random);
    if (got == 0) {
      break;
    }
    filled += got;
  }
  if (filled != count) {
    ThrowInternal("", "loams: cannot read the platform entropy source, so no UUIDv7 can be minted");
  }
  return out;
}

/// The 12-bit counter within the millisecond, seeded randomly so two UUIDs
/// minted in the same millisecond by different processes do not collide, and
/// advanced under a mutex so two threads in one process do not either.
std::uint16_t& CounterFor(std::chrono::system_clock::time_point now) {
  static std::mutex mutex;
  static std::chrono::system_clock::time_point last{};
  static std::uint16_t counter = 0;
  static const std::uint16_t seeded =
      static_cast<std::uint16_t>(static_cast<unsigned>(RandomBytes(2)[0]) << 8 |
                                 static_cast<unsigned char>(RandomBytes(2)[1])) &
      0x0fff;

  std::lock_guard<std::mutex> const guard(mutex);
  if (last != now) {
    last = now;
    // A new millisecond reseeds, so the counter's low bits carry no information
    // across milliseconds and two UUIDs in different milliseconds cannot collide
    // through it.
    counter = static_cast<std::uint16_t>((seeded + 1) & 0x0fff);
  } else {
    counter = static_cast<std::uint16_t>((counter + 1) & 0x0fff);
  }
  return counter;
}

void WriteHexByte(std::string* out, unsigned byte) {
  static constexpr char kDigits[] = "0123456789abcdef";
  out->push_back(kDigits[(byte >> 4) & 0x0f]);
  out->push_back(kDigits[byte & 0x0f]);
}

int HexDigit(char character) {
  if (character >= '0' && character <= '9') return character - '0';
  if (character >= 'a' && character <= 'f') return character - 'a' + 10;
  return -1;
}

}  // namespace

std::string UuidV7() {
  const auto now = std::chrono::system_clock::now();
  const auto millis =
      std::chrono::duration_cast<std::chrono::milliseconds>(now.time_since_epoch()).count();

  unsigned char bytes[16] = {};
  // 48 bits of Unix milliseconds, **big-endian**. Read out with shifts rather
  // than by dividing: dividing keeps the fractional bits of the lower digits and
  // truncates the carry, which puts the wrong byte in.
  auto bits = static_cast<unsigned long long>(millis);
  for (int index = 5; index >= 0; --index) {
    bytes[index] = static_cast<unsigned char>(bits & 0xff);
    bits >>= 8;
  }
  const std::uint16_t counter = CounterFor(now);
  bytes[6] = static_cast<unsigned char>(((bytes[6] & 0x0f) | 0x70));  // version 7
  bytes[7] = static_cast<unsigned char>(((counter >> 8) & 0x0f));
  bytes[8] = static_cast<unsigned char>(((counter & 0x0f) << 4) | (bytes[8] & 0x0f));
  bytes[8] = static_cast<unsigned char>((bytes[8] & 0x3f) | 0x80);  // variant 10

  const std::string random = RandomBytes(8);
  for (int index = 0; index < 8; ++index) {
    bytes[8 + index] = static_cast<unsigned char>(random[static_cast<std::size_t>(index)]);
  }
  // The variant bits live in byte 8, so the random fill must not clobber them:
  // re-apply them after the copy.
  bytes[8] = static_cast<unsigned char>((bytes[8] & 0x3f) | 0x80);

  std::string out;
  out.reserve(36);
  for (int index = 0; index < 4; ++index) WriteHexByte(&out, bytes[index]);
  out.push_back('-');
  for (int index = 4; index < 6; ++index) WriteHexByte(&out, bytes[index]);
  out.push_back('-');
  for (int index = 6; index < 8; ++index) WriteHexByte(&out, bytes[index]);
  out.push_back('-');
  for (int index = 8; index < 10; ++index) WriteHexByte(&out, bytes[index]);
  out.push_back('-');
  for (int index = 10; index < 16; ++index) WriteHexByte(&out, bytes[index]);
  return out;
}

std::optional<long long> UuidV7Millis(std::string_view value) {
  if (value.size() != 36) {
    return std::nullopt;
  }
  if (value[8] != '-' || value[13] != '-' || value[18] != '-' || value[23] != '-') {
    return std::nullopt;
  }
  if (value[14] != '7') {
    return std::nullopt;
  }
  const char variant = value[19];
  if (variant != '8' && variant != '9' && variant != 'a' && variant != 'b') {
    return std::nullopt;
  }
  // 48 bits of timestamp is **twelve** hex digits: the first eight characters plus
  // the next group of four, with the two hyphens skipped. Reading only the first
  // eight returns a number around 2^25, which is January 1970; reading sixteen
  // silently includes the version and the counter.
  long long millis = 0;
  for (std::size_t index = 0; index < 8; ++index) {
    const int digit = HexDigit(value[index]);
    if (digit < 0) return std::nullopt;
    millis = (millis << 4) | digit;
  }
  for (std::size_t index = 9; index < 13; ++index) {
    const int digit = HexDigit(value[index]);
    if (digit < 0) return std::nullopt;
    millis = (millis << 4) | digit;
  }
  return millis;
}

bool DeclaresIdempotencyKey(const google::protobuf::Message& message) {
  const google::protobuf::Descriptor* const descriptor = message.GetDescriptor();
  if (descriptor == nullptr) {
    return false;
  }
  const google::protobuf::FieldDescriptor* const field = descriptor->FindFieldByName(kIdempotencyKeyField);
  return field != nullptr && field->cpp_type() == google::protobuf::FieldDescriptor::CPPTYPE_STRING;
}

std::optional<std::string> ReadIdempotencyKey(const google::protobuf::Message& request) {
  const google::protobuf::Reflection* const reflection = request.GetReflection();
  const google::protobuf::Descriptor* const descriptor = request.GetDescriptor();
  if (reflection == nullptr || descriptor == nullptr) {
    return std::nullopt;
  }
  const google::protobuf::FieldDescriptor* const field = descriptor->FindFieldByName(kIdempotencyKeyField);
  if (field == nullptr || field->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING) {
    return std::nullopt;
  }
  if (field->is_repeated()) {
    return std::nullopt;
  }
  // `has_field` distinguishes present-and-empty from absent, which is what proto3
  // `optional` means: a caller who wrote an empty key means "no key", and the
  // runtime honours that by minting one.
  if (field->has_presence() && !reflection->HasField(request, field)) {
    return std::nullopt;
  }
  return reflection->GetString(request, field);
}

KeyedRequest ApplyIdempotencyKey(const google::protobuf::Message& request, const std::string& supplied,
                                  bool declared) {
  // Neither the binding nor the schema declares the field, so the request is left
  // exactly as the caller wrote it. Inventing a field the schema does not know
  // would be a crash at serialisation time, and the call would then not be
  // retryable — which is the safe direction, but for the wrong reason.
  if (!declared && !DeclaresIdempotencyKey(request)) {
    return KeyedRequest{nullptr, false};
  }
  const std::optional<std::string> current = ReadIdempotencyKey(request);
  if (current.has_value() && !current->empty()) {
    // The caller wrote their own key: `Keyed` is true and the request is
    // unchanged. Making a retry yours rather than the SDK's is sometimes right,
    // because the key is what your storage dedupes on.
    return KeyedRequest{nullptr, true};
  }

  // A **copy**, always. Mutating the caller's message in place would be visible
  // to a caller who reuses it, and would race a concurrent `SerializeToString`
  // on it. The copy is what makes a second attempt safe.
  std::unique_ptr<google::protobuf::Message> clone(request.New());
  if (clone == nullptr) {
    return KeyedRequest{nullptr, false};
  }
  clone->CopyFrom(request);

  const google::protobuf::Reflection* const reflection = clone->GetReflection();
  const google::protobuf::Descriptor* const descriptor = clone->GetDescriptor();
  const google::protobuf::FieldDescriptor* const field =
      descriptor == nullptr ? nullptr : descriptor->FindFieldByName(kIdempotencyKeyField);
  if (reflection == nullptr || field == nullptr ||
      field->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING || field->is_repeated()) {
    // Present but not settable as a scalar string. Returning unkeyed is
    // correct: the call is then not retryable, which is the safe direction.
    return KeyedRequest{nullptr, false};
  }

  const std::string key = supplied.empty() ? UuidV7() : supplied;
  reflection->SetString(clone.get(), field, key);
  return KeyedRequest{std::move(clone), true};
}

}  // namespace loams