# frozen_string_literal: true

require "google/protobuf"
require "json"

module Loams
  module ContentTypes
    CONNECT_UNARY_JSON   = "application/json"
    CONNECT_UNARY_PROTO  = "application/proto"
    CONNECT_STREAM_JSON  = "application/connect+json"
    CONNECT_STREAM_PROTO = "application/connect+proto"
    GRPC_PROTO           = "application/grpc-web+proto"
    GRPC_JSON            = "application/grpc-web+json"
    GRPC_TEXT            = "application/grpc-web-text"

    def self.content_type_for(protocol, codec, streaming: false)
      if protocol == :grpc_web
        codec == :json ? GRPC_JSON : GRPC_PROTO
      elsif streaming
        codec == :json ? CONNECT_STREAM_JSON : CONNECT_STREAM_PROTO
      else
        codec == :json ? CONNECT_UNARY_JSON : CONNECT_UNARY_PROTO
      end
    end

    def self.parse_content_type(ct)
      return [:connect, :proto] if ct.nil? || ct.empty?

      base = ct.split(";").first.strip.downcase
      case base
      when CONNECT_UNARY_JSON, CONNECT_STREAM_JSON
        [:connect, :json]
      when CONNECT_UNARY_PROTO, CONNECT_STREAM_PROTO
        [:connect, :proto]
      when GRPC_JSON
        [:grpc_web, :json]
      when GRPC_PROTO, GRPC_TEXT
        [:grpc_web, :proto]
      else
        [:connect, :proto]
      end
    end
  end

  module MessageCodec
    def self.serialize(descriptor, message, codec)
      return message if message.is_a?(String)

      msgclass = descriptor.is_a?(Google::Protobuf::Descriptor) ? descriptor.msgclass : descriptor

      if codec == :json
        if message.is_a?(Hash)
          message = msgclass.new(message)
        end
        msgclass.encode_json(message)
      else
        if message.is_a?(Hash)
          message = msgclass.new(message)
        end
        msgclass.encode(message)
      end
    end

    def self.deserialize(descriptor, payload, codec)
      return payload if payload.nil?

      msgclass = descriptor.is_a?(Google::Protobuf::Descriptor) ? descriptor.msgclass : descriptor
      payload = payload.to_s

      if codec == :json
        payload = "{}" if payload.empty?
        msgclass.decode_json(payload)
      else
        msgclass.decode(payload.b)
      end
    end
  end
end
