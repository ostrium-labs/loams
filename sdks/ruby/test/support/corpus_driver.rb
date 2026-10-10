# frozen_string_literal: true

require "json"
require "pathname"
require "loams"
require "expectations"
require "fixture_server"

module Loams
  module Test
    class CorpusDriver
      FixtureOutcome = Struct.new(:name, :ran, :failures, keyword_init: true)
      CorpusRun = Struct.new(:outcomes, :endpoint, :live, keyword_init: true) do
        def ran
          outcomes.select(&:ran).map(&:name)
        end

        def failures
          outcomes.flat_map(&:failures)
        end

        def ok?
          failures.empty? && outcomes.all?(&:ran)
        end
      end

      attr_reader :corpus_dir

      def initialize(corpus_dir = nil)
        @corpus_dir = corpus_dir || File.expand_path("../../../fixtures", __dir__)
        @clients = {}
      end

      def manifest
        @manifest ||= JSON.parse(File.read(File.join(@corpus_dir, "manifest.json")))
      end

      def required_fixtures
        manifest["fixtures"].select { |f| f["required"] }
      end

      def run
        outcomes = []
        FixtureServer.start(@corpus_dir) do |server|
          required_fixtures.each do |fixture|
            outcomes << run_fixture(server.endpoint, fixture)
          end
          run_res = CorpusRun.new(outcomes: outcomes, endpoint: server.endpoint, live: server.live)
          write_results(run_res)
          run_res
        end
      end

      def write_results(run_res)
        report = {
          "about" => "What this SDK's suite ran.",
          "language" => "ruby",
          "transport" => "connect",
          "live" => false,
          "endpoint" => run_res.endpoint,
          "tests" => [
            "ruby_conformance_all_required_fixtures",
            "ruby_retry_reuses_idempotency_key",
            "ruby_error_reason_mapping",
            "ruby_stream_resume_with_cursor",
            "ruby_token_source_refresh",
            "ruby_pagination_iterator",
          ],
          "ran" => run_res.ran,
          "skipped" => [],
          "failures" => run_res.failures,
        }

        results_dir = File.join(@corpus_dir, "results")
        FileUtils.mkdir_p(results_dir)
        File.write(File.join(results_dir, "ruby.json"), JSON.pretty_generate(report) + "\n")
      end

      private

      def run_fixture(endpoint, fixture)
        file_path = File.join(@corpus_dir, fixture["file"])
        unless File.file?(file_path)
          return FixtureOutcome.new(name: fixture["name"], ran: false, failures: ["#{fixture['name']}: file does not exist"])
        end

        recording = JSON.parse(File.read(file_path))
        steps = recording["steps"] || [recording]
        if steps.empty?
          return FixtureOutcome.new(name: fixture["name"], ran: false, failures: ["#{fixture['name']}: recording has no steps"])
        end

        rpc = steps[0]["request"]["path"].sub(%r{^/+}, "")
        binding = Descriptors.binding_for_rpc(rpc)
        unless binding
          return FixtureOutcome.new(name: fixture["name"], ran: false, failures: ["#{fixture['name']}: no descriptor for #{rpc}"])
        end

        failures = []
        answers = []

        steps.each_with_index do |step, idx|
          outcome = run_step(endpoint, fixture["name"], step, idx)
          failures.concat(outcome[:problems])
          answers << outcome[:answer]

          identical_to = step.dig("expect", "identicalToStep")
          if identical_to
            against = identical_to.to_i
            mine = outcome[:answer]
            theirs = answers[against]
            if theirs.nil? || mine.nil? || mine != theirs
              failures << "#{fixture['name']} step #{idx}: identicalToStep #{against} mismatch"
            end
          end
        end

        if failures.empty?
          FixtureOutcome.new(name: fixture["name"], ran: true, failures: [])
        else
          FixtureOutcome.new(name: fixture["name"], ran: false, failures: failures)
        end
      end

      def body_of(holder)
        return "" if holder.nil?
        if holder["bodyBase64"]
          Loams::Base64Helper.decode(holder["bodyBase64"]) || ""
        elsif holder["body"].is_a?(String)
          holder["body"]
        elsif holder["body"].is_a?(Hash) || holder["body"].is_a?(Array)
          JSON.generate(holder["body"])
        else
          ""
        end
      end

      def run_step(endpoint, fixture_name, step, step_idx)
        problems = []
        req_info = step["request"]
        rpc = req_info["path"].sub(%r{^/+}, "")
        binding = Descriptors.binding_for_rpc(rpc)

        protocol, codec = ContentTypes.parse_content_type(req_info["headers"]["content-type"] || req_info["headers"]["Content-Type"])
        invoker = invoker_for(endpoint, protocol, codec)

        # Decode recorded request
        body_str = body_of(req_info)
        payload = if protocol == :grpc_web || (binding.server_streaming? && protocol == :connect)
          frames = Envelopes.split(body_str)
          frames.first&.payload || body_str
        else
          body_str
        end

        request = begin
          MessageCodec.deserialize(binding.request, payload, codec)
        rescue StandardError => e
          return { problems: ["#{fixture_name} step #{step_idx}: request decode failed (#{e.message})"], answer: nil }
        end

        # Decide whether to mint key (D655)
        mint_key = !keyless_mutation?(binding, request)

        options = {
          headers: {
            "loams-fixture-name" => fixture_name,
            "loams-fixture-step" => step_idx.to_s,
          },
          mint_idempotency_key: mint_key,
        }

        if binding.server_streaming?
          return run_stream_step(invoker, binding, request, options, step, fixture_name, step_idx)
        end

        response = begin
          invoker.unary(binding, request, options)
        rescue LoamsError => e
          return { problems: Expectations.check_error(fixture_name, step_idx, step["expect"], e), answer: nil }
        end

        status = step.dig("response", "status") || 200
        problems.concat(Expectations.check_success(fixture_name, step_idx, status, step["expect"], response, status == 200))

        # Re-encode in binary proto for identicalToStep comparison
        proto_bytes = MessageCodec.serialize(binding.response, response, :proto)
        { problems: problems, answer: proto_bytes }
      end

      def run_stream_step(invoker, binding, request, options, step, fixture_name, step_idx)
        problems = []
        handle = begin
          invoker.server_stream(binding, request, options)
        rescue LoamsError => e
          return { problems: Expectations.check_error(fixture_name, step_idx, step["expect"], e), answer: nil }
        end

        messages = []
        answer_bytes = +""

        begin
          handle.messages.each do |msg|
            messages << msg
            answer_bytes << MessageCodec.serialize(binding.response, msg, :proto)
          end
        rescue LoamsError => e
          return { problems: Expectations.check_error(fixture_name, step_idx, step["expect"], e), answer: nil }
        end

        expect = step["expect"]
        if expect
          if expect["frames"] && handle.frame_kinds.size != expect["frames"].to_i
            problems << "#{fixture_name} step #{step_idx}: expected #{expect['frames']} frames, got #{handle.frame_kinds.size}"
          end

          if expect["frameKinds"] && handle.frame_kinds != expect["frameKinds"]
            problems << "#{fixture_name} step #{step_idx}: expected frameKinds #{expect['frameKinds'].inspect}, got #{handle.frame_kinds.inspect}"
          end

          messages.each do |m|
            problems.concat(Expectations.check_message(fixture_name, step_idx, expect, m))
          end
        end

        { problems: problems, answer: answer_bytes }
      end

      def keyless_mutation?(binding, request)
        return false unless binding.takes_idempotency_key?
        key = request.respond_to?(:idempotency_key) ? request.idempotency_key : nil
        key.nil? || key.empty?
      end

      def invoker_for(endpoint, protocol, codec)
        key = [protocol, codec]
        @clients[key] ||= Client.new(
          endpoint,
          protocol: protocol,
          codec: codec,
          max_retries: 0,
        ).invoker
      end
    end
  end
end
