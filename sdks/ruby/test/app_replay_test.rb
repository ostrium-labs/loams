# frozen_string_literal: true

require_relative "test_helper"
require "app_replay"

# The recorded app scenario, replayed through the public facade against the
# fixture server (SDKP P0d). `check-languages.mjs --check ruby --suite app`
# reads the fixtures these record as run.
class AppReplayTest < Minitest::Test
  def replay(name)
    skip "no recording for #{name}" unless Loams::Test::AppReplay.available?(name)

    Loams::Test::FixtureServer.start(Loams::Test::AppReplay.dir) do |server|
      failures = Loams::Test::AppReplay.replay(server.endpoint, name)
      assert_empty failures, failures.join("\n")
      Loams::Test::AppReplay.record([name])
    end
  end

  # Spelled out: `check-languages.mjs` looks for each name in this file.
  def test_ruby_app_collections = replay("app_collections")
  def test_ruby_app_search = replay("app_search")
  def test_ruby_app_graph = replay("app_graph")
  def test_ruby_app_sql = replay("app_sql")
  def test_ruby_app_streams = replay("app_streams")
end
