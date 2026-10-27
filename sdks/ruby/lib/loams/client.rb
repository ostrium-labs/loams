# frozen_string_literal: true

require "json"
require "loams/descriptors"
require "loams/transport"
require "loams/invoker"
require "loams/pagination"
require "loams/consistency"
require "loams/app"

module Loams
  class Client
    attr_reader :invoker, :endpoint, :namespace, :session

    # The app modules (SDKP §2), hand-written over the descriptor set's
    # bindings. They take the place of the descriptor-derived module of the
    # same name, whose raw calls stay reachable through `client.module(name)`.
    APP_MODULES = {
      "namespaces" => App::Namespaces,
      "collections" => App::Collections,
      "documents" => App::Documents,
      "search" => App::SearchModule,
      "vector" => App::VectorModule,
      "graph" => App::Graph,
      "sql" => App::Sql,
      "streams" => App::Streams,
    }.freeze

    # A client from the environment (R1): `LOAMS_ENDPOINT`, then
    # `LOAMS_API_KEY` or `LOAMS_TOKEN`, then `LOAMS_NAMESPACE`. Keyword
    # arguments win over the environment.
    def self.from_env(env = ENV, **options)
      endpoint = env["LOAMS_ENDPOINT"]
      if endpoint.nil? || endpoint.empty?
        raise InvalidArgumentError.new("LOAMS_ENDPOINT is not set", code: :invalid_argument)
      end

      key = env["LOAMS_API_KEY"]
      token = env["LOAMS_TOKEN"]
      defaults = {}
      defaults[:api_key] = key unless key.nil? || key.empty?
      defaults[:token] = token if defaults[:api_key].nil? && !(token.nil? || token.empty?)
      ns = env["LOAMS_NAMESPACE"]
      defaults[:namespace] = ns unless ns.nil? || ns.empty?
      new(endpoint, **defaults, **options)
    end

    # +api_key+ is sent as `Authorization: Bearer` (R1), as are +token+ and
    # +token_source+. +namespace+ is the default namespace (R11). +timeout+
    # is the unary default in seconds, nil for none (R12).
    # +session_consistency+ turns on the session store (R4).
    def initialize(
      endpoint = "http://127.0.0.1:4400",
      protocol: :connect,
      codec: :proto,
      api_key: nil,
      token: nil,
      token_source: nil,
      namespace: nil,
      timeout: CallInvoker::DEFAULT_TIMEOUT,
      session_consistency: false,
      transport: nil,
      max_retries: 3
    )
      @endpoint = endpoint
      @namespace = namespace&.to_s
      @session = session_consistency ? SessionStore.new : nil
      tokens = token_source || api_key || token
      retries = RetryPolicy.new(max_retries: max_retries)
      tp = transport || HttpTransport.new(endpoint)

      @invoker = CallInvoker.new(
        transport: tp,
        protocol: protocol,
        codec: codec,
        token_source: tokens,
        retry_policy: retries,
        timeout: timeout,
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

      APP_MODULES.each do |name, klass|
        app = klass.new(self)
        define_singleton_method(name) { app }
      end
    end

    def module(name)
      @modules[name.to_s]
    end

    # Any facade call by its facade-index name (`documents.write`,
    # `instance.get_instance`), over its request's proto3 JSON or message.
    # `headers:`, `timeout:` and `idempotency_key:` are the call's options.
    def call(name, request = {}, **opts)
      mod, call = name.split(".", 2)
      if App::CALLS.key?([mod, call])
        (@raw ||= App::Raw.new(self)).call(name, request, **opts)
      else
        client = @modules[mod] or raise ArgumentError, "no module #{mod}"
        binding = client.module_binding.calls.find { |b| Descriptors.snake(b.facade_name) == call }
        raise ArgumentError, "no call #{name}" if binding.nil?

        message = request.is_a?(Hash) ? binding.request.msgclass.decode_json(JSON.generate(request)) : request
        client.public_send(call, message, opts)
      end
    end

    # A handle on one collection (SDKP §2.8).
    def collection(name, namespace: nil)
      CollectionHandle.new(self, name, namespace: namespace)
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
