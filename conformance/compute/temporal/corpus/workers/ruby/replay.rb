# frozen_string_literal: true

# corpus_replays_ruby (DX1 TSDK-ruby): every Ruby history in a directory
# replays with the pinned SDK's Temporalio::Worker::WorkflowReplayer and zero
# nondeterminism errors. The gateway's Ruby leg points it at the histories
# the gateway wrote; the nightly at the committed corpus.
#
#   CORPUS_HISTORIES=dir ruby replay.rb     # default: ../../histories/ruby
#
# It reads each history's binary .binpb when there is one (a binary decode
# skips fields newer than the SDK's protos; the SDK's own JSON reader
# refuses them) and the protojson .json otherwise. Each replay is given the
# workflow ID the generator used, corpus-ruby-<scenario>: child workflow IDs
# derive from it, and a gateway's started event may not carry it.
require 'temporalio/api'
require 'temporalio/worker/workflow_replayer'
require 'temporalio/workflow_history'
require_relative 'lib/scenarios'
require_relative 'lib/workflows'

module Corpus
  module Replay
    RUN = /(\.run\d+)?\.json\z/

    # A history with the workflow ID the replay should see.
    class History < Temporalio::WorkflowHistory
      def initialize(events, workflow_id)
        super(events)
        @corpus_workflow_id = workflow_id
      end

      def workflow_id
        @corpus_workflow_id
      end
    end

    module_function

    def load(json_path)
      bin = json_path.sub(/\.json\z/, '.binpb')
      proto = Temporalio::Api::History::V1::History
      msg = if File.exist?(bin)
              proto.decode(File.binread(bin))
            else
              proto.decode_json(File.read(json_path), ignore_unknown_fields: true)
            end
      History.new(msg.events.to_a, "#{ID_PREFIX}#{File.basename(json_path).sub(RUN, '')}")
    end

    def run(dir)
      files = Dir.glob(File.join(dir, '*.json')).sort
      raise "no histories in #{dir}" if files.empty?

      replayer = Temporalio::Worker::WorkflowReplayer.new(workflows: Workflows::ALL)
      failures = []
      files.each do |f|
        replayer.replay_workflow(load(f))
        puts "ok   #{File.basename(f)}"
      rescue StandardError => e
        failures << "#{File.basename(f)}: #{e.class}: #{e.message}"
        puts "FAIL #{failures.last}"
      end
      puts "corpus_replays_ruby: #{files.size - failures.size}/#{files.size} replayed"
      failures.empty? ? 0 : 1
    end
  end
end

if $PROGRAM_NAME == __FILE__
  dir = ENV.fetch('CORPUS_HISTORIES', File.expand_path('../../histories/ruby', __dir__))
  exit(Corpus::Replay.run(dir))
end
