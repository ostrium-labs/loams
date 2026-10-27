# frozen_string_literal: true

require "json"
require "fileutils"
require "loams"
require "builder_corpus"
require "fixture_server"

module Loams
  module Test
    # Replays the recorded app scenario (`sdks/fixtures/recorded/app/*.json`,
    # `app-scenario.md`) through the public facade against the fixture server.
    #
    # Every step names its facade call and carries the recorded request and
    # answer. The request goes through `Client#call` (the facade's own path:
    # namespace, idempotency key, retries, errors) with the recorded body, and a
    # step that names a builder case is first checked against that builder's
    # output, so the replay proves the builder too. The answer is compared with
    # the recording, the fixture's `volatile` fields masked; a refusal is
    # compared by its code.
    module AppReplay
      def self.dir
        Loams::Test::BuilderCorpus.dir
      end

      def self.fixture(name)
        JSON.parse(File.read(File.join(dir, "recorded", "app", "#{name}.json")))
      end

      def self.available?(name)
        File.exist?(File.join(dir, "recorded", "app", "#{name}.json"))
      end

      # Runs every step of `name`; returns the failures.
      def self.replay(endpoint, name)
        fixture = fixture(name)
        volatile = fixture["volatile"] || []
        client = Loams::Client.new(endpoint, codec: :json, max_retries: 0)
        failures = []
        fixture["steps"].each_with_index do |step, index|
          label = "#{name} step #{index} (#{step["call"]})"
          body = step.dig("request", "body")
          request = body.nil? || body.empty? ? {} : JSON.parse(body)
          if (case_name = step["builder"])
            problem = check_builder(case_name, request)
            failures << "#{label}: #{problem}" if problem
          end
          headers = { "loams-fixture-name" => name, "loams-fixture-step" => index.to_s }
          expected = step["response"]
          begin
            answer = client.call(step["call"], request, headers: headers)
            if expected["status"] != 200
              failures << "#{label}: answered, but the recording is a #{expected.dig("body", "code")}"
              next
            end
            got = JSON.parse(answer.class.encode_json(answer))
            diff = compare(mask(expected["body"], volatile), mask(got, volatile))
            failures << "#{label}: #{diff}" if diff
          rescue Loams::LoamsError => e
            want = expected.dig("body", "code")
            if expected["status"] == 200
              failures << "#{label}: refused (#{e.code}) where the recording answered"
            elsif e.code.to_s != want
              failures << "#{label}: refused with #{e.code}, the recording with #{want}"
            end
          end
        end
        failures
      end

      # The builder case's output against the part of the request it builds.
      def self.check_builder(case_name, request)
        path = File.join(dir, "builders", "#{case_name}.json")
        return "no builder case #{case_name}" unless File.exist?(path)

        c = JSON.parse(File.read(path))
        built = BuilderCorpus.build(c)
        part = case c["builder"]
        when "schema" then request["schema"]
        when "filter" then request["where"] || request["filter"]
        else request.reject { |k, _| k == "namespace" }
        end
        return nil if BuilderCorpus.same?(c["message"], part, built)

        "the #{case_name} builder builds #{JSON.generate(built)}, the recording sent #{JSON.generate(part)}"
      end

      def self.mask(value, volatile)
        case value
        when Hash then value.reject { |k, _| volatile.include?(k) }.transform_values { |v| mask(v, volatile) }
        when Array then value.map { |v| mask(v, volatile) }
        else value
        end
      end

      # nil when equal; numbers compare to float32 precision.
      def self.compare(want, got, at = "$")
        case want
        when Hash
          return "#{at}: not an object" unless got.is_a?(Hash)

          (want.keys | got.keys).each do |k|
            problem = compare(want[k], got[k], "#{at}.#{k}")
            return problem if problem
          end
          nil
        when Array
          return "#{at}: #{got.inspect} is not #{want.length} items" unless got.is_a?(Array) && got.length == want.length

          want.each_with_index do |w, i|
            problem = compare(w, got[i], "#{at}[#{i}]")
            return problem if problem
          end
          nil
        when Numeric
          ok = got.is_a?(Numeric) && (want - got).abs <= 1e-6 * [1.0, want.abs].max
          ok ? nil : "#{at}: #{got.inspect}, recorded #{want.inspect}"
        else
          want == got ? nil : "#{at}: #{got.inspect}, recorded #{want.inspect}"
        end
      end

      # Adds `names` to `sdks/fixtures/results/ruby.json`'s `ran`.
      def self.record(names)
        path = File.join(dir, "results", "ruby.json")
        FileUtils.mkdir_p(File.dirname(path))
        report = File.exist?(path) ? JSON.parse(File.read(path)) : { "language" => "ruby", "transport" => "connect", "ran" => [] }
        report["ran"] = ((report["ran"] || []) | names).sort
        File.write(path, JSON.pretty_generate(report) + "\n")
      end
    end
  end
end
