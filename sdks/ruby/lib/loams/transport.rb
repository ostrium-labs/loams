# frozen_string_literal: true

require "net/http"
require "uri"

module Loams
  Request = Struct.new(:method, :path, :headers, :body, :rpc, keyword_init: true)
  Response = Struct.new(:status, :headers, :body, :stream, keyword_init: true)

  class HttpTransport
    def initialize(endpoint)
      @endpoint = endpoint.to_s.sub(%r{/+$}, "")
      @uri = URI.parse(@endpoint)
    end

    def call_unary(request)
      url = URI.join(@endpoint + "/", request.path.sub(%r{^/+}, ""))
      http = Net::HTTP.new(url.host, url.port)
      http.use_ssl = (url.scheme == "https")
      http.read_timeout = 30
      http.open_timeout = 5

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
