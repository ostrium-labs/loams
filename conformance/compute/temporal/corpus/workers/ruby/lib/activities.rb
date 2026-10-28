# frozen_string_literal: true

# The corpus's Ruby activities (DX1 TSDK-ruby), named as the Go and
# TypeScript ones so the three languages' histories carry the same activity
# types. A change here changes the histories: regenerate the corpus in the
# same pull request.
require 'temporalio/activity'
require 'temporalio/error'

module Corpus
  module Activities
    class Greet < Temporalio::Activity::Definition
      activity_name 'greet'

      def execute(name)
        "hello #{name}"
      end
    end

    class HeartbeatThrice < Temporalio::Activity::Definition
      activity_name 'heartbeat_thrice'

      def execute
        (1..3).each { |i| Temporalio::Activity::Context.current.heartbeat(i) }
        3
      end
    end

    class FailTwice < Temporalio::Activity::Definition
      activity_name 'fail_twice'

      def execute
        attempt = Temporalio::Activity::Context.current.info.attempt
        raise Temporalio::Error::ApplicationError.new("attempt #{attempt} fails", type: 'Transient') if attempt < 3

        attempt
      end
    end

    class SleepFor < Temporalio::Activity::Definition
      activity_name 'sleep_for'

      # Sleeps without heartbeating: the start-to-close and heartbeat timeout
      # scenarios time it out. A cancel raises into this thread (the
      # default activity_cancel_raise), so the sleep ends early.
      def execute(ms)
        sleep(ms / 1000.0)
        nil
      end
    end

    class Large < Temporalio::Activity::Definition
      activity_name 'large'

      def execute(n)
        'x' * n
      end
    end

    ALL = [Greet, HeartbeatThrice, FailTwice, SleepFor, Large].freeze
  end
end
