# frozen_string_literal: true

module Loams
  class RetryPolicy
    attr_reader :max_retries, :initial_backoff_ms, :max_backoff_ms

    def initialize(max_retries: 3, initial_backoff_ms: 10, max_backoff_ms: 500)
      @max_retries = max_retries
      @initial_backoff_ms = initial_backoff_ms
      @max_backoff_ms = max_backoff_ms
    end

    def retryable_call?(binding, request, options)
      # If caller explicitly set retry policy
      return false if options[:max_retries] == 0 || @max_retries == 0

      # Check if this call is safe to retry
      if binding.retry == :safe || binding.idempotency != :none
        true
      elsif binding.takes_idempotency_key?
        # A mutation is ONLY retried if it carries an idempotency key!
        key = if request.respond_to?(:idempotency_key)
          request.idempotency_key
        elsif request.is_a?(Hash)
          request[:idempotency_key] || request["idempotency_key"]
        end
        !key.nil? && !key.empty?
      else
        false
      end
    end

    def backoff_duration(attempt)
      delay_ms = [@initial_backoff_ms * (2**(attempt - 1)), @max_backoff_ms].min
      delay_ms / 1000.0
    end
  end
end
