# frozen_string_literal: true

# The Ruby half of the corpus (DX1 TSDK-ruby): a worker for lib/workflows.rb
# and a starter that drives each scenario with the fixed workflow ID
# corpus-ruby-<scenario>. The Go exporter writes the histories afterwards
# (`corpus-go export -prefix corpus-ruby-`), so every language's files come
# from one exporter.
#
#   ruby generate.rb [all|worker|starter] <address> <namespace> <out.json> [scenario,...]
#
#   all      the worker and the starter in one process (the default; what
#            generate-ruby.sh and the gateway's Ruby leg run)
#   worker   the worker only, until SIGINT or SIGTERM
#   starter  the starter only, against a worker running elsewhere
#
# CORPUS_REGISTER_NAMESPACE=1 registers the namespace (72 h retention) and
# the Keyword search attribute CorpusKeyword first, as the Go generator does;
# generate-ruby.sh sets it against upstream Temporal. The gateway's leg does
# not (its namespaces are bootstrapped, and its DX1 scenarios need no custom
# search attribute).
#
# TEMPORAL_API_KEY, when set, is sent as `authorization: Bearer <key>`. TLS
# stays off: the SDK would otherwise switch TLS on because a key is set, and
# the gateway's profile listens in plaintext on the stack's network.
require 'json'
require 'temporalio/client'
require 'temporalio/worker'
require_relative 'lib/activities'
require_relative 'lib/scenarios'
require_relative 'lib/workflows'

module Corpus
  # Drives one scenario and records what it observed.
  class Driver
    def initialize(client)
      @client = client
    end

    def wait_started(id)
      handle = @client.workflow_handle(id)
      loop do
        d = handle.describe
        return if d.history_length.to_i >= 4 && d.raw_description.pending_workflow_task.nil?

        sleep(0.2)
      end
    end

    def start(name, id, *args, **extra)
      @client.start_workflow(name, *args, id:, task_queue: TASK_QUEUE, **extra)
    end

    def drive(name, id)
      o = { 'scenario' => name, 'workflow_id' => id, 'expected' => [] }
      settle = ->(handle) { o['result'] = JSON.generate(handle.result) }
      case name
      when 'hello'
        settle.(start(name, id, 'corpus'))
      when 'signal_then_complete'
        h = start(name, id)
        wait_started(id)
        h.signal('go', 's')
        settle.(h)
      when 'signal_with_start'
        op = Temporalio::Client::WithStartWorkflowOperation.new(name, id:, task_queue: TASK_QUEUE)
        settle.(@client.signal_with_start_workflow('go', 's', start_workflow_operation: op))
      when 'query_while_running'
        h = start(name, id)
        wait_started(id)
        state = h.query('state')
        raise "query answered #{state}" unless state == 'waiting'

        o['expected'] << 'query state=waiting'
        h.signal('finish')
        settle.(h)
      when 'cancel_requested', 'terminated'
        h = start(name, id)
        wait_started(id)
        name == 'cancel_requested' ? h.cancel : h.terminate('corpus terminates it')
        begin
          h.result
        rescue Temporalio::Error::WorkflowFailedError => e
          o['expected'] << "#{e.class}: #{e.cause&.class}: #{e.cause&.message}"
          return o
        end
        raise "the workflow completed instead of closing as #{name}"
      when 'workflow_failed_application_error'
        h = start(name, id)
        begin
          h.result
        rescue Temporalio::Error::WorkflowFailedError => e
          o['expected'] << "#{e.cause.class}: #{e.cause.message} (#{e.cause.type})"
          return o
        end
        raise 'want an application error'
      when 'continue_as_new_3'
        settle.(start(name, id, 0))
      when 'id_reuse_allow_duplicate_failed_only'
        policy = { id_reuse_policy: Temporalio::WorkflowIDReusePolicy::ALLOW_DUPLICATE_FAILED_ONLY }
        first = start(name, id, true, **policy)
        begin
          first.result
          raise 'the first run should fail'
        rescue Temporalio::Error::WorkflowFailedError
          nil
        end
        second = start(name, id, false, **policy)
        o['expected'] << 'the first run failed; the second started with the same ID'
        settle.(second)
      when 'id_conflict_fail'
        policy = { id_conflict_policy: Temporalio::WorkflowIDConflictPolicy::FAIL }
        h = start(name, id, **policy)
        begin
          start(name, id, **policy)
          raise 'the second start should be refused'
        rescue Temporalio::Error::WorkflowAlreadyStartedError
          o['expected'] << 'the second start was refused: WorkflowExecutionAlreadyStarted'
        end
        h.signal('finish')
        settle.(h)
      when 'memo'
        settle.(start(name, id, memo: { 'initial' => 'set at start' }))
      when 'update_accepted_completed', 'update_rejected'
        h = start(name, id)
        wait_started(id)
        by = name == 'update_rejected' ? -1 : 2
        begin
          total = h.execute_update('increment', by, id: "#{id}-update")
          o['expected'] << "update returned #{total}"
          raise 'the validator should reject a negative increment' if by.negative?
        rescue Temporalio::Error::WorkflowUpdateFailedError => e
          raise unless by.negative?

          o['expected'] << "update rejected: #{e.cause&.message || e.message}"
        end
        h.signal('finish')
        settle.(h)
      else
        settle.(start(name, id))
      end
      o
    end
  end

  module Generate
    module_function

    def connect(address, namespace)
      Temporalio::Client.connect(address, namespace, api_key: ENV.fetch('TEMPORAL_API_KEY', nil), tls: false)
    end

    # One activity at a time, as the Go generator: the parallel fan-out's
    # activities complete in the order they were scheduled, so regenerating
    # is stable.
    def worker(client)
      Temporalio::Worker.new(
        client:, task_queue: TASK_QUEUE,
        workflows: Workflows::ALL, activities: Activities::ALL,
        tuner: Temporalio::Worker::Tuner.create_fixed(activity_slots: 1)
      )
    end

    def already_exists?(err)
      err.is_a?(Temporalio::Error::RPCError) && err.code == Temporalio::Error::RPCError::Code::ALREADY_EXISTS
    end

    # The namespace and CorpusKeyword, waiting for a new namespace to reach
    # every service's cache (upstream Temporal only).
    def register(client, namespace)
      api = Temporalio::Api
      begin
        client.workflow_service.register_namespace(
          api::WorkflowService::V1::RegisterNamespaceRequest.new(
            namespace:, workflow_execution_retention_period: Google::Protobuf::Duration.new(seconds: 72 * 3600)
          )
        )
      rescue Temporalio::Error::RPCError => e
        raise unless already_exists?(e)
      end
      deadline = Time.now + 60
      begin
        client.operator_service.add_search_attributes(
          api::OperatorService::V1::AddSearchAttributesRequest.new(
            namespace:,
            search_attributes: { 'CorpusKeyword' => api::Enums::V1::IndexedValueType::INDEXED_VALUE_TYPE_KEYWORD }
          )
        )
      rescue Temporalio::Error::RPCError => e
        unless already_exists?(e)
          raise if Time.now > deadline

          sleep(2)
          retry
        end
      end
      sleep(3)
    end

    def selected(only)
      want = only&.split(',')
      SCENARIOS.map(&:first).select { |s| want.nil? || want.include?(s) }
    end

    def starter(client, namespace, out, only)
      driver = Driver.new(client)
      observations = selected(only).map do |name|
        o = driver.drive(name, "#{ID_PREFIX}#{name}")
        puts "corpus: #{name}: #{o['result']}"
        o
      end
      doc = { 'namespace' => namespace, 'language' => 'ruby', 'scenarios' => observations }
      File.write(out, "#{JSON.pretty_generate(doc)}\n")
    end

    def main(argv)
      mode = %w[all worker starter].include?(argv.first) ? argv.shift : 'all'
      address, namespace, out, only = argv
      if address.nil? || namespace.nil? || (mode != 'worker' && out.nil?)
        warn 'usage: generate.rb [all|worker|starter] <address> <namespace> <out.json> [scenarios]'
        return 2
      end
      client = connect(address, namespace)
      register(client, namespace) if ENV['CORPUS_REGISTER_NAMESPACE'] == '1'
      case mode
      when 'worker'
        worker(client).run(shutdown_signals: %w[SIGINT SIGTERM])
      when 'starter'
        starter(client, namespace, out, only)
      else
        worker(client).run { starter(client, namespace, out, only) }
      end
      0
    end
  end
end

exit(Corpus::Generate.main(ARGV.dup)) if $PROGRAM_NAME == __FILE__
