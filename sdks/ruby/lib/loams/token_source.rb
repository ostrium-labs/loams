# frozen_string_literal: true

module Loams
  class TokenSource
    def initialize(token_or_callable = nil, &block)
      @callable = block || (token_or_callable.respond_to?(:call) ? token_or_callable : nil)
      @static_token = @callable ? nil : token_or_callable.to_s
      @cached_token = nil
      @mutex = Mutex.new
    end

    def token(force_refresh: false)
      return @static_token if @static_token

      @mutex.synchronize do
        if @cached_token.nil? || force_refresh
          @cached_token = @callable.call.to_s
        end
        @cached_token
      end
    end

    def refresh!
      token(force_refresh: true)
    end
  end
end
