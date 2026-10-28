# frozen_string_literal: true

# The corpus's Ruby workflows (DX1 TSDK-ruby): the scenarios of
# scenarios.json except the Go-only ones (workflow_task_failure_then_fix,
# schedule_triggered, reset_to_first_workflow_task), as in TypeScript. Each
# class's workflow name is its scenario name, so the generator and the
# replayer agree on it. A change here changes the histories: regenerate the
# Ruby corpus in the same pull request.
require 'temporalio/api'
require 'temporalio/cancellation'
# Workflow::Definition refers to Temporalio::VersioningBehavior without
# requiring it (gem 1.9.0); the worker and the client load it, a bare
# Definition::Info.from_class (the unit test, the replayer) does not.
require 'temporalio/common_enums'
require 'temporalio/error'
require 'temporalio/retry_policy'
require 'temporalio/search_attributes'
require 'temporalio/workflow'
require_relative 'activities'

module Corpus
  module Workflows
    A = Corpus::Activities
    W = Temporalio::Workflow
    ONCE = Temporalio::RetryPolicy.new(max_attempts: 1)
    # A Keyword attribute, as the Go generator registers it.
    KEYWORD = Temporalio::SearchAttributes::Key.new(
      'CorpusKeyword', Temporalio::SearchAttributes::IndexedValueType::KEYWORD
    )

    # "TIMEOUT_TYPE_<kind>" for an activity that timed out, as TypeScript
    # reports it; anything else is "not a timeout: ...".
    def self.timeout_kind(err)
      if err.is_a?(Temporalio::Error::ActivityError) && err.cause.is_a?(Temporalio::Error::TimeoutError)
        name = Temporalio::Api::Enums::V1::TimeoutType.lookup(err.cause.type)
        return (name || 'TIMEOUT_TYPE_UNSPECIFIED').to_s
      end
      "not a timeout: #{err.message}"
    end

    # A workflow that waits for the `finish` signal.
    module Finishable
      def self.included(base)
        base.class_eval do
          workflow_signal name: 'finish'
          def finish
            @finished = true
          end
        end
      end

      def wait_finish
        W.wait_condition { @finished }
      end
    end

    # --- DX1 ------------------------------------------------------------------

    class Hello < W::Definition
      workflow_name 'hello'

      def execute(name)
        W.execute_activity(A::Greet, name, start_to_close_timeout: 30, retry_policy: ONCE)
      end
    end

    class TwoActivitiesSequential < W::Definition
      workflow_name 'two_activities_sequential'

      def execute
        a = W.execute_activity(A::Greet, 'one', start_to_close_timeout: 30, retry_policy: ONCE)
        b = W.execute_activity(A::Greet, 'two', start_to_close_timeout: 30, retry_policy: ONCE)
        "#{a}, #{b}"
      end
    end

    class ActivityParallelFanout10 < W::Definition
      workflow_name 'activity_parallel_fanout_10'

      def execute
        futures = Array.new(10) do |i|
          W::Future.new { W.execute_activity(A::Greet, "n#{i}", start_to_close_timeout: 30, retry_policy: ONCE) }
        end
        W::Future.all_of(*futures).wait
        futures.map(&:result)
      end
    end

    class SignalThenComplete < W::Definition
      workflow_name 'signal_then_complete'

      workflow_signal name: 'go'
      def go(value)
        @value = value
      end

      def execute
        W.wait_condition { !@value.nil? }
        "got #{@value}"
      end
    end

    class QueryWhileRunning < W::Definition
      include Finishable
      workflow_name 'query_while_running'

      workflow_query name: 'state'
      def state
        @state
      end

      def execute
        @state = 'waiting'
        wait_finish
        @state = 'done'
      end
    end

    class CancelRequested < W::Definition
      workflow_name 'cancel_requested'

      def execute
        W.sleep(3600)
        nil
      end
    end

    class Terminated < W::Definition
      include Finishable
      workflow_name 'terminated'

      def execute
        wait_finish
        nil
      end
    end

    class WorkflowFailedApplicationError < W::Definition
      workflow_name 'workflow_failed_application_error'

      def execute
        raise Temporalio::Error::ApplicationError.new('boom', 'detail', type: 'CorpusBoom', non_retryable: true)
      end
    end

    class ActivityHeartbeatDetails < W::Definition
      workflow_name 'activity_heartbeat_details'

      def execute
        W.execute_activity(A::HeartbeatThrice, start_to_close_timeout: 30, heartbeat_timeout: 10, retry_policy: ONCE)
      end
    end

    # --- DX2 ------------------------------------------------------------------

    class TimerShort < W::Definition
      workflow_name 'timer_short'

      def execute
        W.sleep(1)
        nil
      end
    end

    class TimerCancelled < W::Definition
      workflow_name 'timer_cancelled'

      def execute
        cancel, cancel_proc = Temporalio::Cancellation.new
        long = W::Future.new { W.sleep(3600, cancellation: cancel) }
        W.sleep(0.1)
        cancel_proc.call
        begin
          long.wait
        rescue StandardError => e
          return 'cancelled' if Temporalio::Error.canceled?(e)

          raise
        end
        raise Temporalio::Error::ApplicationError.new('the long timer was not cancelled', non_retryable: true)
      end
    end

    class ActivityRetryBackoff3Attempts < W::Definition
      workflow_name 'activity_retry_backoff_3_attempts'

      def execute
        W.execute_activity(
          A::FailTwice,
          start_to_close_timeout: 10,
          retry_policy: Temporalio::RetryPolicy.new(initial_interval: 1, backoff_coefficient: 2, max_attempts: 3)
        )
      end
    end

    class ActivityStartToCloseTimeout < W::Definition
      workflow_name 'activity_start_to_close_timeout'

      def execute
        W.execute_activity(A::SleepFor, 5000, start_to_close_timeout: 1, retry_policy: ONCE)
        'no timeout'
      rescue Temporalio::Error::ActivityError => e
        Workflows.timeout_kind(e)
      end
    end

    class ActivityScheduleToStartTimeout < W::Definition
      workflow_name 'activity_schedule_to_start_timeout'

      def execute
        W.execute_activity(
          A::Greet, 'nobody',
          task_queue: 'corpus-no-worker', schedule_to_start_timeout: 1, start_to_close_timeout: 1, retry_policy: ONCE
        )
        'no timeout'
      rescue Temporalio::Error::ActivityError => e
        Workflows.timeout_kind(e)
      end
    end

    class ActivityHeartbeatTimeout < W::Definition
      workflow_name 'activity_heartbeat_timeout'

      def execute
        W.execute_activity(A::SleepFor, 5000, start_to_close_timeout: 30, heartbeat_timeout: 1, retry_policy: ONCE)
        'no timeout'
      rescue Temporalio::Error::ActivityError => e
        Workflows.timeout_kind(e)
      end
    end

    class SignalWithStart < W::Definition
      workflow_name 'signal_with_start'

      workflow_signal name: 'go'
      def go(value)
        @value = value
      end

      def execute
        W.wait_condition { !@value.nil? }
        "started with #{@value}"
      end
    end

    class ChildGreeter < W::Definition
      workflow_name 'child_greeter'

      def execute(name)
        W.execute_activity(A::Greet, name, start_to_close_timeout: 30, retry_policy: ONCE)
      end
    end

    class ChildSleeper < W::Definition
      workflow_name 'child_sleeper'

      def execute
        W.sleep(3600)
        nil
      end
    end

    class ChildWorkflow < W::Definition
      workflow_name 'child_workflow'

      def execute
        W.execute_child_workflow(ChildGreeter, 'child', id: "#{W.info.workflow_id}-child")
      end
    end

    class ChildWorkflowCancelled < W::Definition
      workflow_name 'child_workflow_cancelled'

      def execute
        cancel, cancel_proc = Temporalio::Cancellation.new
        child = W.start_child_workflow(ChildSleeper, id: "#{W.info.workflow_id}-child", cancellation: cancel)
        cancel_proc.call
        begin
          child.result
        rescue StandardError => e
          return 'child cancelled' if Temporalio::Error.canceled?(e)

          raise
        end
        raise Temporalio::Error::ApplicationError.new('the child was not cancelled', non_retryable: true)
      end
    end

    class ContinueAsNew3 < W::Definition
      workflow_name 'continue_as_new_3'

      def execute(n)
        raise W::ContinueAsNewError.new(n + 1) if n < 3

        n
      end
    end

    class IdReuseAllowDuplicateFailedOnly < W::Definition
      workflow_name 'id_reuse_allow_duplicate_failed_only'

      def execute(fail)
        raise Temporalio::Error::ApplicationError.new('first run fails', type: 'CorpusFirstRun', non_retryable: true) if fail

        'second run'
      end
    end

    class IdConflictFail < W::Definition
      include Finishable
      workflow_name 'id_conflict_fail'

      def execute
        wait_finish
        nil
      end
    end

    class LocalActivity < W::Definition
      workflow_name 'local_activity'

      def execute
        W.execute_local_activity(A::Greet, 'local', start_to_close_timeout: 10)
      end
    end

    class SideEffectAndVersionMarker < W::Definition
      workflow_name 'side_effect_and_version_marker'

      # Ruby has no SideEffect (workflow code runs on a deterministic fiber
      # scheduler, as TypeScript's is deterministic by construction):
      # patched writes the version marker only.
      def execute
        "version #{W.patched('corpus-change') ? 1 : 0}"
      end
    end

    # --- DX3 ------------------------------------------------------------------

    class SearchAttributesUpsert < W::Definition
      workflow_name 'search_attributes_upsert'

      def execute
        W.upsert_search_attributes(KEYWORD.value_set('first'))
        W.sleep(0.1)
        W.upsert_search_attributes(KEYWORD.value_set('second'))
        nil
      end
    end

    class Memo < W::Definition
      workflow_name 'memo'

      def execute
        W.upsert_memo({ 'step' => 'upserted', 'n' => 2 })
        nil
      end
    end

    class LargePayload1mb < W::Definition
      workflow_name 'large_payload_1mb'

      def execute
        W.execute_activity(A::Large, 1 << 20, start_to_close_timeout: 30, retry_policy: ONCE).length
      end
    end

    # --- DX4 ------------------------------------------------------------------

    # The `increment` update with a validator that refuses a negative step.
    module Updates
      def self.included(base)
        base.class_eval do
          include Finishable

          workflow_update name: 'increment'
          def increment(by)
            @total = (@total || 0) + by
          end

          workflow_update_validator(:increment)
          def validate_increment(by)
            raise Temporalio::Error::ApplicationError, 'increment must not be negative' if by.negative?
          end

          def execute
            @total ||= 0
            wait_finish
            "total #{@total}"
          end
        end
      end
    end

    class UpdateAcceptedCompleted < W::Definition
      include Updates
      workflow_name 'update_accepted_completed'
    end

    class UpdateRejected < W::Definition
      include Updates
      workflow_name 'update_rejected'
    end

    ALL = [
      Hello, TwoActivitiesSequential, ActivityParallelFanout10, SignalThenComplete, QueryWhileRunning,
      CancelRequested, Terminated, WorkflowFailedApplicationError, ActivityHeartbeatDetails,
      TimerShort, TimerCancelled, ActivityRetryBackoff3Attempts, ActivityStartToCloseTimeout,
      ActivityScheduleToStartTimeout, ActivityHeartbeatTimeout, SignalWithStart, ChildGreeter, ChildSleeper,
      ChildWorkflow, ChildWorkflowCancelled, ContinueAsNew3, IdReuseAllowDuplicateFailedOnly, IdConflictFail,
      LocalActivity, SideEffectAndVersionMarker,
      SearchAttributesUpsert, Memo, LargePayload1mb,
      UpdateAcceptedCompleted, UpdateRejected
    ].freeze
  end
end
