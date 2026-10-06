# frozen_string_literal: true

require "open3"
require "pathname"

module Loams
  module Test
    class FixtureServer
      attr_reader :endpoint

      def initialize(endpoint, stdin = nil, stdout = nil, wait_thr = nil)
        @endpoint = endpoint
        @stdin = stdin
        @stdout = stdout
        @wait_thr = wait_thr
      end

      def live
        false
      end

      def close
        if @wait_thr
          begin
            Process.kill("TERM", @wait_thr.pid)
            @wait_thr.join(3) || Process.kill("KILL", @wait_thr.pid)
          rescue StandardError
            # ignore
          end
        end
      end

      def self.find_repo_root(start_dir)
        dir = Pathname.new(start_dir).expand_path
        loop do
          return dir.to_s if (dir / "sdks/fixtures/manifest.json").file?
          parent = dir.parent
          break if parent == dir
          dir = parent
        end
        # Fallback to current working directory or relative
        File.expand_path("../../..", __dir__)
      end

      def self.start(corpus_dir = nil)
        if (env_ep = ENV["LOAMS_CONFORMANCE_ENDPOINT"]) && !env_ep.empty?
          return new(env_ep)
        end

        corpus_dir ||= File.expand_path("../../../fixtures", __dir__)
        repo_root = find_repo_root(corpus_dir)
        script = File.join(repo_root, "sdks/conformance/fixture-server.mjs")

        raise "fixture-server.mjs not found at #{script}" unless File.file?(script)

        cmd = ["node", script, "--port", "0", "--fixtures", corpus_dir]
        stdin, stdout, wait_thr = Open3.popen2e(*cmd, chdir: repo_root)

        endpoint = nil
        output = +""
        deadline = Time.now + 15

        while Time.now < deadline
          begin
            line = stdout.gets
            break if line.nil?
            output << line
            if line =~ %r{http://127\.0\.0\.1:\d+}
              endpoint = line.match(%r{http://127\.0\.0\.1:\d+})[0]
              break
            end
          rescue StandardError => e
            output << e.message
            break
          end
        end

        unless endpoint
          wait_thr.kill rescue nil
          raise "Failed to start fixture-server: #{output}"
        end

        server = new(endpoint, stdin, stdout, wait_thr)
        if block_given?
          begin
            yield server
          ensure
            server.close
          end
        else
          server
        end
      end
    end
  end
end
