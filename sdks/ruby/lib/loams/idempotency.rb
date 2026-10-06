# frozen_string_literal: true

require "securerandom"

module Loams
  module Idempotency
    # Mints a standard UUIDv7
    def self.uuidv7
      now_ms = (Time.now.to_f * 1000).to_i
      rand_bytes = SecureRandom.random_bytes(10)

      time_hi = (now_ms >> 16) & 0xFFFFFFFF
      time_low = now_ms & 0xFFFF

      r1, r2, r3, r4, r5, r6, r7, r8, r9, r10 = rand_bytes.bytes
      ver_and_rand = 0x7000 | ((r1 & 0x0F) << 8) | r2
      var_and_rand = 0x80 | (r3 & 0x3F)

      format(
        "%08x-%04x-%04x-%02x%02x-%02x%02x%02x%02x%02x%02x",
        time_hi,
        time_low,
        ver_and_rand,
        var_and_rand,
        r4,
        r5, r6, r7, r8, r9, r10
      )
    end

    # Applies an idempotency key to a request message if the binding expects one
    def self.apply(binding, request, explicit_key = nil, mint: true)
      return nil unless binding.takes_idempotency_key?

      existing_key = if request.respond_to?(:idempotency_key)
        request.idempotency_key
      elsif request.is_a?(Hash)
        request[:idempotency_key] || request["idempotency_key"] || request["idempotencyKey"]
      end

      if explicit_key && !explicit_key.empty?
        set_key(request, explicit_key)
        explicit_key
      elsif existing_key && !existing_key.empty?
        existing_key
      elsif mint
        key = uuidv7
        set_key(request, key)
        key
      else
        nil
      end
    end

    def self.set_key(request, key)
      if request.respond_to?(:idempotency_key=)
        request.idempotency_key = key
      elsif request.is_a?(Hash)
        request[:idempotency_key] = key
      end
    end
  end
end
