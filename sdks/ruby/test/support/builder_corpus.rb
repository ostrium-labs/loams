# frozen_string_literal: true

require "json"
require "loams"

module Loams
  module Test
    # Drives the builders from the builder corpus's neutral notation
    # (`sdks/fixtures/builders/README.md`): `{"op": verb, "args": [...]}`,
    # where an argument with an `op` is itself a call and a trailing plain
    # object is the call's options.
    module BuilderCorpus
      FUSIONS = %w[rrf dbsf weighted].freeze
      SORTS = %w[score pk field].freeze
      # Calls whose trailing object is an argument, not options.
      NO_OPTIONS = %w[ids].freeze

      # The corpus directory: `LOAMS_FIXTURES_DIR`, or `sdks/fixtures`.
      def self.dir
        ENV["LOAMS_FIXTURES_DIR"] || File.expand_path("../../../fixtures", __dir__)
      end

      def self.cases
        Dir[File.join(dir, "builders", "*.json")].sort.map { |f| JSON.parse(File.read(f)) }
      end

      def self.build(c)
        case c.fetch("builder")
        when "schema"
          c["spec"].reduce(Loams::Schema.new) { |s, call| apply(s, call) }.to_h
        when "filter"
          evaluate(c["spec"]).to_h
        when "search"
          spec = c["spec"]
          spec["calls"].reduce(Loams::Search.on(spec["on"])) { |s, call| apply(s, call) }.build
        else raise "unknown builder #{c["builder"]}"
        end
      end

      # A nested call: a filter, a fusion or a sort key.
      def self.evaluate(call)
        op = call.fetch("op")
        target = if FUSIONS.include?(op) then Loams::Fusion
        elsif SORTS.include?(op) then Loams::Sort
        else Loams::Filter
        end
        apply(target, call)
      end

      def self.apply(target, call)
        op = call.fetch("op")
        args = call.fetch("args", []).map { |a| a.is_a?(Hash) && a.key?("op") ? evaluate(a) : a }
        kw = {}
        if !NO_OPTIONS.include?(op) && args.last.is_a?(Hash)
          kw = args.pop.transform_keys(&:to_sym)
        end
        target.public_send(op, *args, **kw)
      end

      # Whether two proto3 JSON values mean the same message (or Struct).
      def self.same?(message_name, a, b)
        if message_name == "google.protobuf.Struct"
          normalize(a) == normalize(b)
        else
          klass = Loams::Descriptors.message(message_name).msgclass
          klass.decode_json(JSON.generate(a)) == klass.decode_json(JSON.generate(b))
        end
      end

      def self.normalize(v)
        case v
        when Hash then v.to_h { |k, x| [k, normalize(x)] }
        when Array then v.map { |x| normalize(x) }
        when Integer then v.to_f
        else v
        end
      end
    end
  end
end
