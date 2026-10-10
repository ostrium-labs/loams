# frozen_string_literal: true

require "loams/descriptors"
require "loams/transport"
require "loams/invoker"
require "loams/pagination"

module Loams
  class Client
    attr_reader :invoker, :endpoint

    def initialize(
      endpoint = "http://127.0.0.1:4400",
      protocol: :connect,
      codec: :proto,
      token: nil,
      token_source: nil,
      transport: nil,
      max_retries: 3
    )
      @endpoint = endpoint
      tokens = token_source || token
      retries = RetryPolicy.new(max_retries: max_retries)
      tp = transport || HttpTransport.new(endpoint)

      @invoker = CallInvoker.new(
        transport: tp,
        protocol: protocol,
        codec: codec,
        token_source: tokens,
        retry_policy: retries,
      )

      # Build module clients
      @modules = {}
      Descriptors.modules.each do |mod|
        client = ModuleClient.new(mod, @invoker)
        @modules[mod.name] = client
        # Define accessor method
        method_name = Descriptors.snake(mod.name)
        define_singleton_method(method_name) { client }
      end
    end

    def module(name)
      @modules[name.to_s]
    end

    class ModuleClient
      attr_reader :module_binding, :invoker

      def initialize(module_binding, invoker)
        @module_binding = module_binding
        @invoker = invoker

        # Define call methods on the module client
        module_binding.calls.each do |call_binding|
          call_name = Descriptors.snake(call_binding.facade_name)
          define_singleton_method(call_name) do |request = nil, options = {}|
            request ||= call_binding.request.msgclass.new
            if call_binding.server_streaming?
              invoker.server_stream(call_binding, request, options)
            else
              invoker.unary(call_binding, request, options)
            end
          end
        end
      end
    end
  end
end
