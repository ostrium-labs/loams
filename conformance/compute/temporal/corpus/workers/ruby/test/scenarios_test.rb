# frozen_string_literal: true

# Unit tests for the Ruby corpus worker (no server): its scenario list is
# scenarios.json's without the Go-only ones, every scenario has a workflow
# of that name, and the replayer derives the generator's workflow IDs.
require 'json'
require 'minitest/autorun'
require_relative '../lib/scenarios'
require_relative '../lib/workflows'
require_relative '../replay'

class ScenariosTest < Minitest::Test
  SCENARIOS_JSON = File.expand_path('../../../scenarios.json', __dir__)

  def test_matches_scenarios_json
    want = JSON.parse(File.read(SCENARIOS_JSON))['scenarios']
               .reject { |s| s['go_only'] }
               .map { |s| [s['name'], s['phase']] }
    assert_equal want, Corpus::SCENARIOS
  end

  def test_every_scenario_has_a_workflow
    names = Corpus::Workflows::ALL.map { |w| Temporalio::Workflow::Definition::Info.from_class(w).name }
    Corpus::SCENARIOS.each { |(name, _)| assert_includes names, name }
    assert_equal names.uniq, names
  end

  def test_replay_workflow_id_from_file_name
    assert_equal 'continue_as_new_3', 'continue_as_new_3.run2.json'.sub(Corpus::Replay::RUN, '')
    assert_equal 'hello', 'hello.json'.sub(Corpus::Replay::RUN, '')
    assert_equal 'corpus-ruby-hello', "#{Corpus::ID_PREFIX}hello"
  end

  def test_timeout_kind_names_the_proto_enum
    err = Temporalio::Error::ActivityError.new(
      'activity timed out', scheduled_event_id: 5, started_event_id: 6, identity: 'x',
      activity_type: 'sleep_for', activity_id: '1', retry_state: nil
    )
    assert_match(/\Anot a timeout: /, Corpus::Workflows.timeout_kind(err))
  end
end
