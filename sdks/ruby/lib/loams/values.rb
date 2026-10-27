# frozen_string_literal: true

require "json"

module Loams
  # Conversions between Ruby values and the messages the app modules send and
  # answer.
  module Values
    # A message as plain Ruby: Hashes with the proto's `snake_case` keys, a
    # `Struct` as a Hash, 64-bit integers as Strings (proto3 JSON). nil stays
    # nil.
    def self.to_ruby(message)
      return nil if message.nil?
      return message.map { |m| to_ruby(m) } if message.respond_to?(:to_a) && !message.respond_to?(:to_proto)
      return message unless message.class.respond_to?(:encode_json)

      JSON.parse(message.class.encode_json(message, preserve_proto_fieldnames: true, emit_defaults: false))
    end

    # A primary key as a `DocumentId`'s JSON: a non-negative Integer is
    # `uint`, a String is `string`, and `{uuid: …}` (or any Hash naming its
    # arm) passes through.
    def self.document_id(id)
      case id
      when Integer
        raise InvalidArgumentError.new("a document id is a non-negative integer, a string or {uuid: …}", code: :invalid_argument) if id.negative?

        { "uint" => id.to_s }
      when String, Symbol then { "string" => id.to_s }
      when Hash then id.transform_keys(&:to_s)
      else
        if id.class.respond_to?(:encode_json)
          JSON.parse(id.class.encode_json(id))
        else
          raise InvalidArgumentError.new("a document id is a non-negative integer, a string or {uuid: …}, not #{id.class}", code: :invalid_argument)
        end
      end
    end

    # A `DocumentId` message (or its JSON) back as a Ruby value: an Integer,
    # a String, or `{"uuid" => …}`.
    def self.from_document_id(id)
      json = id.is_a?(Hash) ? id : to_ruby(id)
      return nil if json.nil? || json.empty?
      return Integer(json["uint"]) if json.key?("uint")
      return json["string"] if json.key?("string")

      json
    end

    # A Ruby value as a `loams.graph.v1.Value`'s JSON, for GQL parameters.
    def self.graph_value(value)
      case value
      when nil then { "null" => "NULL_VALUE" }
      when true, false then { "boolean" => value }
      when Integer then value.negative? || value <= 0x7FFF_FFFF_FFFF_FFFF ? { "int64" => value.to_s } : { "uint64" => value.to_s }
      when Float then { "float64" => value }
      when String, Symbol then { "string" => value.to_s }
      when Array then { "list" => { "values" => value.map { |v| graph_value(v) } } }
      when Hash then { "map" => { "entries" => value.to_h { |k, v| [k.to_s, graph_value(v)] } } }
      else raise InvalidArgumentError.new("no graph value for #{value.class}", code: :invalid_argument)
      end
    end

    # A `loams.graph.v1.Value` (message or JSON) as a Ruby value. Nodes,
    # relationships, paths and temporal values stay as their JSON Hash.
    def self.from_graph_value(value)
      json = value.is_a?(Hash) ? value : to_ruby(value)
      return nil if json.nil? || json.empty?

      kind, inner = json.first
      case kind
      when "null" then nil
      when "boolean", "float64", "string" then inner
      when "int64", "uint64" then Integer(inner)
      when "list" then Array(inner["values"]).map { |v| from_graph_value(v) }
      when "map" then (inner["entries"] || {}).to_h { |k, v| [k, from_graph_value(v)] }
      else json
      end
    end
  end

  # `Loams.to_h(message)`: see `Values.to_ruby`.
  def self.to_h(message)
    Values.to_ruby(message)
  end
end
