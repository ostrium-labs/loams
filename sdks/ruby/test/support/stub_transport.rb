# frozen_string_literal: true

require "loams"

module Loams
  module Test
    class StubTransport
      attr_reader :requests

      Seen = Struct.new(:rpc, :method, :path, :headers, :body, keyword_init: true)
      Answer = Struct.new(:status, :headers, :body, keyword_init: true)

      def initialize
        @answers = []
        @requests = []
      end

      def answer(status, content_type, body, headers = {})
        hdrs = { "content-type" => content_type }
        headers.each { |k, v| hdrs[k.downcase] = v }
        @answers << Answer.new(status: status, headers: hdrs, body: body.b)
        self
      end

      def answer_message(descriptor, message, codec = :proto, protocol = :connect)
        ct = ContentTypes.content_type_for(protocol, codec)
        body = MessageCodec.serialize(descriptor, message, codec)
        answer(200, ct, body)
      end

      def answer_connect_error(status, code, message, details = "")
        detail_json = if details.empty?
          ""
        else
          %Q(,"details":[{"type":"loams.errors.v1.ErrorInfo","value":"#{details}"}])
        end
        body = %Q({"code":"#{code}","message":"#{message}"#{detail_json}})
        answer(status, ContentTypes::CONNECT_UNARY_JSON, body)
      end

      def answer_stream(*payloads, end_error: "{}")
        body = +""
        payloads.each do |p|
          body << Envelopes.wrap(p, Envelopes::FLAG_DATA)
        end
        body << Envelopes.wrap(end_error, Envelopes::FLAG_END_STREAM)
        answer(200, ContentTypes::CONNECT_STREAM_PROTO, body)
      end

      def call_unary(request)
        @requests << Seen.new(
          rpc: request.rpc,
          method: request.method,
          path: request.path,
          headers: request.headers,
          body: request.body,
        )

        ans = @answers.shift
        raise "StubTransport has no more queued answers" if ans.nil?

        Response.new(
          status: ans.status,
          headers: ans.headers,
          body: ans.body,
        )
      end

      def client(max_retries: 3, token_source: nil)
        Client.new(
          "http://127.0.0.1:4400",
          transport: self,
          max_retries: max_retries,
          token_source: token_source,
        )
      end
    end
  end
end
