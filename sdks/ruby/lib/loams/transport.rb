# frozen_string_literal: true

require "net/http"
require "uri"

module Loams
  Request = Struct.new(:method, :path, :headers, :body, :rpc, :timeout, keyword_init: true)
  Response = Struct.new(:status, :headers, :body, :stream, keyword_init: true)

  class HttpTransport
    def initialize(endpoint)
      @endpoint = endpoint.to_s
      # Trailing slashes trimmed by index: `sub(%r{/+$}, "")` backtracks
      # quadratically on a long run of slashes (CodeQL rb/polynomial-redos).
      cut = @endpoint.length
      cut -= 1 while cut.positive? && @endpoint[cut - 1] == "/"
      @endpoint = @endpoint[0, cut]
      @uri = URI.parse(@endpoint)
    end

    def call_unary(request)
      url = URI.join(@endpoint + "/", request.path.sub(%r{^/+}, ""))
      http = Net::HTTP.new(url.host, url.port)
      http.use_ssl = (url.scheme == "https")
      # R12: the call's remaining budget, or no limit when it has none.
      http.read_timeout = request.timeout
      http.open_timeout = request.timeout ? [request.timeout, 5].min : 5
      http.write_timeout = request.timeout if request.timeout

      req = Net::HTTPGenericRequest.new(
        request.method || "POST",
        true,
        true,
        url.request_uri,
        request.headers || {}
      )
      req.body = request.body if request.body

      res = http.request(req)

      headers = {}
      res.each_header { |k, v| headers[k.downcase] = v }

      Response.new(
        status: res.code.to_i,
        headers: headers,
        body: res.body,
      )
    end

    def call_stream(request, &block)
      url = URI.join(@endpoint + "/", request.path.sub(%r{^/+}, ""))
      http = Net::HTTP.new(url.host, url.port)
      http.use_ssl = (url.scheme == "https")
      http.read_timeout = 60
      http.open_timeout = 5

      req = Net::HTTPGenericRequest.new(
        request.method || "POST",
        true,
        true,
        url.request_uri,
        request.headers || {}
      )
      req.body = request.body if request.body

      if block_given?
        http.request(req) do |res|
          headers = {}
          res.each_header { |k, v| headers[k.downcase] = v }
          yield res.code.to_i, headers, res
        end
      else
        res = http.request(req)
        headers = {}
        res.each_header { |k, v| headers[k.downcase] = v }
        Response.new(
          status: res.code.to_i,
          headers: headers,
          body: res.body,
        )
      end
    end
  end
end
