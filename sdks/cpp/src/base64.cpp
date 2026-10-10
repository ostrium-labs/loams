// Base64 and hex, as `base64.hpp` documents.

#include "loams/base64.hpp"

#include <array>

namespace loams {
namespace {

constexpr char kAlphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The value of a base64 character, or -1. **Not** a lookup indexed by `char`:
/// `char` is signed on the platforms this SDK builds on, so a byte above 0x7F
/// would index off the front of the table.
int ValueOf(char character) {
  if (character >= 'A' && character <= 'Z') return character - 'A';
  if (character >= 'a' && character <= 'z') return character - 'a' + 26;
  if (character >= '0' && character <= '9') return character - '0' + 52;
  if (character == '+') return 62;
  if (character == '/') return 63;
  return -1;
}

bool IsSpace(char character) {
  return character == ' ' || character == '\n' || character == '\r' || character == '\t' || character == '\f' ||
         character == '\v';
}

int HexValue(char character) {
  if (character >= '0' && character <= '9') return character - '0';
  if (character >= 'a' && character <= 'f') return character - 'a' + 10;
  if (character >= 'A' && character <= 'F') return character - 'A' + 10;
  return -1;
}

}  // namespace

std::string Base64Encode(std::string_view bytes) {
  std::string out;
  out.reserve(((bytes.size() + 2) / 3) * 4);
  std::size_t at = 0;
  while (at + 2 < bytes.size()) {
    const auto group = (static_cast<unsigned>(static_cast<unsigned char>(bytes[at])) << 16) |
                       (static_cast<unsigned>(static_cast<unsigned char>(bytes[at + 1])) << 8) |
                       static_cast<unsigned>(static_cast<unsigned char>(bytes[at + 2]));
    out.push_back(kAlphabet[(group >> 18) & 0x3f]);
    out.push_back(kAlphabet[(group >> 12) & 0x3f]);
    out.push_back(kAlphabet[(group >> 6) & 0x3f]);
    out.push_back(kAlphabet[group & 0x3f]);
    at += 3;
  }
  const std::size_t left = bytes.size() - at;
  if (left == 1) {
    const auto group = static_cast<unsigned>(static_cast<unsigned char>(bytes[at])) << 16;
    out.push_back(kAlphabet[(group >> 18) & 0x3f]);
    out.push_back(kAlphabet[(group >> 12) & 0x3f]);
    out.push_back('=');
    out.push_back('=');
  } else if (left == 2) {
    const auto group = (static_cast<unsigned>(static_cast<unsigned char>(bytes[at])) << 16) |
                       (static_cast<unsigned>(static_cast<unsigned char>(bytes[at + 1])) << 8);
    out.push_back(kAlphabet[(group >> 18) & 0x3f]);
    out.push_back(kAlphabet[(group >> 12) & 0x3f]);
    out.push_back(kAlphabet[(group >> 6) & 0x3f]);
    out.push_back('=');
  }
  return out;
}

std::optional<std::string> Base64Decode(std::string_view text) {
  // Whitespace is skipped rather than rejected: HTTP header folding and a
  // line-wrapped `grpc-status-details-bin` both put it there and neither changes
  // the value. Anything **else** outside the alphabet is rejected, because a
  // decoder that skips invalid characters turns a truncated detail into a
  // shorter but still-plausible one — which is how a `reason` goes missing
  // without an error.
  std::array<int, 4> group{};
  int filled = 0;
  std::string out;
  out.reserve((text.size() / 4) * 3);
  for (const char character : text) {
    if (IsSpace(character)) continue;
    if (character == '=') {
      // Padding ends the stream. Anything after it that is not whitespace is a
      // malformed body, so it is rejected rather than ignored.
      break;
    }
    const int value = ValueOf(character);
    if (value < 0) {
      return std::nullopt;
    }
    group[static_cast<std::size_t>(filled)] = value;
    ++filled;
    if (filled == 4) {
      const auto packed = static_cast<unsigned>((group[0] << 18) | (group[1] << 12) | (group[2] << 6) | group[3]);
      out.push_back(static_cast<char>((packed >> 16) & 0xff));
      out.push_back(static_cast<char>((packed >> 8) & 0xff));
      out.push_back(static_cast<char>(packed & 0xff));
      filled = 0;
    }
  }
  // A partial group is legal only without padding, and only if it is 2 or 3
  // characters: 1 character is one leftover sextet with nothing to pair it with.
  if (filled == 1) {
    return std::nullopt;
  }
  if (filled == 2 || filled == 3) {
    const auto packed = static_cast<unsigned>(group[0] << 18) | (group[1] << 12) |
                         (filled == 3 ? static_cast<unsigned>(group[2] << 6) : 0u);
    out.push_back(static_cast<char>((packed >> 16) & 0xff));
    if (filled == 3) {
      out.push_back(static_cast<char>((packed >> 8) & 0xff));
    }
  }
  return out;
}

std::string ToHex(std::string_view bytes) {
  static constexpr char kDigits[] = "0123456789abcdef";
  std::string out;
  out.reserve(bytes.size() * 2);
  for (const char character : bytes) {
    const auto byte = static_cast<unsigned char>(character);
    out.push_back(kDigits[byte >> 4]);
    out.push_back(kDigits[byte & 0x0f]);
  }
  return out;
}

std::optional<std::string> FromHex(std::string_view text) {
  if (text.size() % 2 != 0) {
    return std::nullopt;
  }
  std::string out;
  out.reserve(text.size() / 2);
  for (std::size_t at = 0; at < text.size(); at += 2) {
    const int high = HexValue(text[at]);
    const int low = HexValue(text[at + 1]);
    if (high < 0 || low < 0) {
      return std::nullopt;
    }
    out.push_back(static_cast<char>((high << 4) | low));
  }
  return out;
}

}  // namespace loams