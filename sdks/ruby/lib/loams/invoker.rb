# frozen_string_literal: true

require "loams/codec"
require "loams/envelopes"
require "loams/errors"
require "loams/idempotency"
require "loams/retry"
require "loams/token_source"
require "loams/streams"

module Loams
  class CallInvoker
    attr_reader :transport, :protocol, :codec, :token_source, :retry_policy

    def initialize(
      transport:,
      protocol: :connect,
      codec: :proto,
      token_source: nil,
      retry_policy: nil
    )
      @transport = transport
      @protocol = protocol
      @codec = codec
      @token_source = token_source.is_a?(TokenSource) ? token_source : (token_source ? TokenSource.new(token_source) : nil)
      @retry_policy = retry_policy || RetryPolicy.new
    end

    def unary(binding, request, options = {})
      # Decide idempotency key
      mint = options.fetch(:mint_idempotency_key, true)
      key = Idempotency.apply(binding, request, options[:idempotency_key], mint: mint)

      attempt = 0
      refreshed_token = false

      loop do
        attempt += 1

        # Build headers
        ct = ContentTypes.content_type_for(@protocol, @codec, streaming: false)
        headers = { "content-type" => ct }

        if (tok = current_token)
          headers["authorization"] = "Bearer #{tok}"
        end

        if key && !key.empty?
          headers["idempotency-key"] = key
        end

        if options[:headers]
          options[:headers].each { |k, v| headers[k.to_s.downcase] = v.to_s }
        end

        # Serialize body
        body = MessageCodec.serialize(binding.request, request, @codec)
        if @protocol == :grpc_web
          body = Envelopes.wrap(body, Envelopes::FLAG_DATA)
        end

        req = Request.new(
          method: "POST",
          path: "/#{binding.rpc}",
          headers: headers,
          body: body,
          rpc: binding.rpc,
        )

        res = begin
          @transport.call_unary(req)
        rescue StandardError => e
          raise ErrorParser.build_error(:unknown, e.message, nil, nil, binding.rpc)
        end

        # Handle gRPC-Web trailers in headers
        if @protocol == :grpc_web && (res.headers["grpc-status"] || res.headers["Grpc-Status"])
          status_num = (res.headers["grpc-status"] || res.headers["Grpc-Status"]).to_i
          if status_num != 0
            err = ErrorParser.parse_grpc_trailers(res.headers, res.status, binding.rpc)
            if can_refresh_and_retry?(err, refreshed_token)
              refreshed_token = true
              next
            end
            if can_retry?(err, binding, request, options, attempt)
              sleep(@retry_policy.backoff_duration(attempt))
              next
            end
            raise err
          end
        end

        # Non-200 HTTP response
        if res.status != 200
          err = ErrorParser.parse_connect_error(res.body, res.status, binding.rpc)

          if can_refresh_and_retry?(err, refreshed_token)
            refreshed_token = true
            next
          end

          if can_retry?(err, binding, request, options, attempt)
            sleep(@retry_policy.backoff_duration(attempt))
            next
          end

          raise err
        end

        # Check gRPC-Web response payload
        res_body = res.body
        if @protocol == :grpc_web
          frames = Envelopes.split(res_body)
          trailer_frame = frames.find(&:trailers?)
          if trailer_frame
            headers = {}
            trailer_frame.payload.each_line do |line|
              if line.include?(":")
                k, v = line.split(":", 2)
                headers[k.strip.downcase] = v.strip
              end
            end
            status_num = (headers["grpc-status"] || headers["Grpc-Status"]).to_i
            if status_num != 0
              err = ErrorParser.parse_grpc_trailers(headers, res.status, binding.rpc)
              if can_refresh_and_retry?(err, refreshed_token)
                refreshed_token = true
                next
              end
              if can_retry?(err, binding, request, options, attempt)
                sleep(@retry_policy.backoff_duration(attempt))
                next
              end
              raise err
            end
          end

          data_frame = frames.find(&:data?)
          res_body = data_frame ? data_frame.payload : ""
        end

        return MessageCodec.deserialize(binding.response, res_body, @codec)
      end
    end

    def server_stream(binding, request, options = {}, resume_policy = nil)
      ct = ContentTypes.content_type_for(@protocol, @codec, streaming: true)
      
      stream_proc = proc do |cursor|
        if cursor && !cursor.empty?
          if request.respond_to?(:resume_cursor=)
            request.resume_cursor = cursor
          elsif request.respond_to?(:cursor=)
            request.cursor = cursor
          elsif request.is_a?(Hash)
            request[:resume_cursor] = cursor
          end
        end

        headers = { "content-type" => ct }
        if (tok = current_token)
          headers["authorization"] = "Bearer #{tok}"
        end
        if options[:headers]
          options[:headers].each { |k, v| headers[k.to_s.downcase] = v.to_s }
        end

        body = MessageCodec.serialize(binding.request, request, @codec)
        body = Envelopes.wrap(body, Envelopes::FLAG_DATA)

        req = Request.new(
          method: "POST",
          path: "/#{binding.rpc}",
          headers: headers,
          body: body,
          rpc: binding.rpc,
        )

        res = @transport.call_unary(req)
        if res.status != 200
          raise ErrorParser.parse_connect_error(res.body, res.status, binding.rpc)
        end

        Envelopes.split(res.body)
      end

      initial_frames = stream_proc.call(nil)
      handle = ServerStreamHandle.new(initial_frames, &stream_proc)
      handle.instance_variable_set(:@decode_proc, proc { |payload| MessageCodec.deserialize(binding.response, payload, @codec) })
      handle
    end

    private

    def current_token(force_refresh: false)
      @token_source&.token(force_refresh: force_refresh)
    end

    def can_refresh_and_retry?(err, already_refreshed)
      return false if already_refreshed || @token_source.nil?

      if err.code == :unauthenticated || err.reason == :token_expired || err.http_status == 401
        @token_source.refresh!
        true
      else
        false
      end
    end

    def can_retry?(err, binding, request, options, attempt)
      max = options[:max_retries] || @retry_policy.max_retries
      return false if attempt >= max + 1
      return false unless err.retryable?

      @retry_policy.retryable_call?(binding, request, options)
    end
  end
end
