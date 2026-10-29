package dev.loams.conformance.temporal.corpus;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.SerializationFeature;
import com.google.protobuf.ByteString;
import com.google.protobuf.Timestamp;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import io.temporal.api.common.v1.WorkflowExecution;
import io.temporal.api.enums.v1.EventType;
import io.temporal.api.enums.v1.IndexedValueType;
import io.temporal.api.enums.v1.WorkflowExecutionStatus;
import io.temporal.api.enums.v1.WorkflowIdConflictPolicy;
import io.temporal.api.enums.v1.WorkflowIdReusePolicy;
import io.temporal.api.history.v1.History;
import io.temporal.api.history.v1.HistoryEvent;
import io.temporal.api.operatorservice.v1.AddSearchAttributesRequest;
import io.temporal.api.workflow.v1.WorkflowExecutionInfo;
import io.temporal.api.workflowservice.v1.DescribeWorkflowExecutionRequest;
import io.temporal.api.workflowservice.v1.DescribeWorkflowExecutionResponse;
import io.temporal.api.workflowservice.v1.GetWorkflowExecutionHistoryRequest;
import io.temporal.api.workflowservice.v1.GetWorkflowExecutionHistoryResponse;
import io.temporal.api.workflowservice.v1.ListWorkflowExecutionsRequest;
import io.temporal.api.workflowservice.v1.ListWorkflowExecutionsResponse;
import io.temporal.api.workflowservice.v1.RegisterNamespaceRequest;
import io.temporal.api.workflowservice.v1.ResetWorkflowExecutionRequest;
import io.temporal.api.workflowservice.v1.ResetWorkflowExecutionResponse;
import io.temporal.client.UpdateOptions;
import io.temporal.client.WorkflowClient;
import io.temporal.client.WorkflowClientOptions;
import io.temporal.client.WorkflowExecutionAlreadyStarted;
import io.temporal.client.WorkflowFailedException;
import io.temporal.client.WorkflowOptions;
import io.temporal.client.WorkflowStub;
import io.temporal.client.WorkflowUpdateException;
import io.temporal.client.WorkflowUpdateStage;
import io.temporal.client.schedules.Schedule;
import io.temporal.client.schedules.ScheduleActionExecution;
import io.temporal.client.schedules.ScheduleActionExecutionStartWorkflow;
import io.temporal.client.schedules.ScheduleActionResult;
import io.temporal.client.schedules.ScheduleActionStartWorkflow;
import io.temporal.client.schedules.ScheduleClient;
import io.temporal.client.schedules.ScheduleClientOptions;
import io.temporal.client.schedules.ScheduleHandle;
import io.temporal.client.schedules.ScheduleOptions;
import io.temporal.client.schedules.ScheduleSpec;
import io.temporal.client.schedules.ScheduleState;
import io.temporal.failure.ApplicationFailure;
import io.temporal.serviceclient.OperatorServiceStubs;
import io.temporal.serviceclient.OperatorServiceStubsOptions;
import io.temporal.serviceclient.WorkflowServiceStubs;
import io.temporal.serviceclient.WorkflowServiceStubsOptions;
import io.temporal.worker.Worker;
import io.temporal.worker.WorkerFactory;
import io.temporal.worker.WorkerOptions;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.TimeUnit;

/**
 * The Java half of the history corpus (plan DX1, lane TSDK-java): a worker for {@link Workflows}
 * and a driver per scenario, with the fixed workflow ID corpus-java-&lt;scenario&gt;, then the
 * exporter, as the Go generator (workers/go/main.go) does it.
 *
 * <pre>
 *   Generate generate --address HOST:PORT --namespace NS --out DIR [--scenarios a,b] [--no-search-attribute]
 *   Generate export   --address HOST:PORT --namespace NS --out DIR --scenarios a,b [--prefix corpus-java-]
 * </pre>
 *
 * <p>generate registers the namespace (72 h retention) and, unless --no-search-attribute, the
 * corpus's keyword search attribute, both idempotently; runs each scenario; and exports every run
 * of its workflow ID as DIR/&lt;scenario&gt;.json (protojson, what the SDK replayers read) and
 * DIR/&lt;scenario&gt;.binpb (the History message), or &lt;scenario&gt;.run&lt;k&gt;.* oldest first
 * when there are several runs. It writes what each driver observed to DIR/../generate-java.json.
 *
 * <p>TEMPORAL_API_KEY, when set, is sent as {@code authorization: Bearer <key>} in plain text (the
 * Loams gateway's API keys; its compose profile has no TLS on the compose network). Unset,
 * nothing changes.
 */
public final class Generate {
  private Generate() {}

  static final Duration SCENARIO_TIMEOUT = Duration.ofMinutes(3);

  /** What a driver saw, written to generate-java.json. */
  static final class Observation {
    String scenario;
    String workflowId;
    int runs;
    String result;
    final List<String> expected = new ArrayList<>();

    Map<String, Object> toJson() {
      Map<String, Object> m = new LinkedHashMap<>();
      m.put("scenario", scenario);
      m.put("workflow_id", workflowId);
      m.put("runs", runs);
      if (result != null) {
        m.put("result", result);
      }
      if (!expected.isEmpty()) {
        m.put("expected", expected);
      }
      return m;
    }
  }

  /** Parsed command-line flags. */
  record Args(
      String command,
      String address,
      String namespace,
      Path out,
      List<String> scenarios,
      String prefix,
      boolean searchAttribute) {

    static Args parse(String[] argv) {
      if (argv.length < 1) {
        throw new IllegalArgumentException(
            "usage: Generate generate|export --address A --namespace N --out DIR [--scenarios a,b]"
                + " [--prefix P] [--no-search-attribute]");
      }
      Map<String, String> flags = new HashMap<>();
      boolean searchAttribute = true;
      for (int i = 1; i < argv.length; i++) {
        String a = argv[i];
        if (a.equals("--no-search-attribute")) {
          searchAttribute = false;
        } else if (a.startsWith("--") && i + 1 < argv.length) {
          flags.put(a.substring(2), argv[++i]);
        } else {
          throw new IllegalArgumentException("Generate: unknown argument " + a);
        }
      }
      String only = flags.getOrDefault("scenarios", "");
      List<String> scenarios =
          only.isEmpty() ? List.of() : Arrays.asList(only.split(","));
      return new Args(
          argv[0],
          flags.getOrDefault("address", "temporal-frontend:7233"),
          flags.getOrDefault("namespace", "corpus"),
          Path.of(flags.getOrDefault("out", "histories/java")),
          scenarios,
          flags.getOrDefault("prefix", Scenarios.ID_PREFIX),
          searchAttribute);
    }

    /** The scenarios to generate: all of them, or those named, in plan order. */
    List<String> pick() {
      List<String> out = new ArrayList<>();
      // "All" is decided once: the set empties as names are matched.
      boolean all = scenarios.isEmpty();
      Set<String> want = new LinkedHashSet<>(scenarios);
      for (Scenarios.Scenario s : Scenarios.ALL) {
        if (all || want.remove(s.name())) {
          out.add(s.name());
        }
      }
      if (!want.isEmpty()) {
        throw new IllegalArgumentException("Generate: unknown scenarios " + want);
      }
      return out;
    }
  }

  public static void main(String[] argv) throws Exception {
    int code = 0;
    try {
      run(Args.parse(argv));
    } catch (Exception e) {
      e.printStackTrace();
      code = 1;
    }
    // The SDK's gRPC and worker threads are not daemons.
    System.exit(code);
  }

  static WorkflowServiceStubsOptions stubOptions(String address) {
    WorkflowServiceStubsOptions.Builder b = WorkflowServiceStubsOptions.newBuilder().setTarget(address);
    String key = System.getenv("TEMPORAL_API_KEY");
    if (key != null && !key.isEmpty()) {
      // addApiKey would turn TLS on; the gateway's compose network is plain text.
      b.addApiKey(() -> key).setEnableHttps(false);
    }
    return b.build();
  }

  static void run(Args args) throws Exception {
    WorkflowServiceStubsOptions options = stubOptions(args.address());
    WorkflowServiceStubs service = WorkflowServiceStubs.newServiceStubs(options);
    try {
      Files.createDirectories(args.out());
      switch (args.command()) {
        case "generate" -> {
          ensureNamespace(service, options, args.namespace(), args.searchAttribute());
          generate(service, args);
        }
        case "export" -> {
          if (args.scenarios().isEmpty()) {
            throw new IllegalArgumentException("Generate: export needs --scenarios");
          }
          for (String name : args.scenarios()) {
            export(service, args.namespace(), args.prefix() + name, args.out().resolve(name));
          }
        }
        default -> throw new IllegalArgumentException("Generate: unknown command " + args.command());
      }
    } finally {
      service.shutdown();
      service.awaitTermination(10, TimeUnit.SECONDS);
    }
  }

  /** Registers the namespace (72 h retention, DX1 R0.9) and the corpus's search attribute. */
  static void ensureNamespace(
      WorkflowServiceStubs service,
      WorkflowServiceStubsOptions options,
      String namespace,
      boolean searchAttribute)
      throws InterruptedException {
    try {
      service
          .blockingStub()
          .registerNamespace(
              RegisterNamespaceRequest.newBuilder()
                  .setNamespace(namespace)
                  .setWorkflowExecutionRetentionPeriod(
                      com.google.protobuf.Duration.newBuilder().setSeconds(72 * 3600).build())
                  .build());
    } catch (StatusRuntimeException e) {
      if (e.getStatus().getCode() != Status.Code.ALREADY_EXISTS) {
        throw e;
      }
    }
    if (!searchAttribute) {
      return;
    }
    OperatorServiceStubs operator =
        OperatorServiceStubs.newServiceStubs(OperatorServiceStubsOptions.newBuilder(options).build());
    try {
      // A new namespace takes a moment to reach every service's cache.
      Instant deadline = Instant.now().plus(Duration.ofMinutes(1));
      while (true) {
        try {
          operator
              .blockingStub()
              .addSearchAttributes(
                  AddSearchAttributesRequest.newBuilder()
                      .setNamespace(namespace)
                      .putSearchAttributes(
                          Scenarios.SEARCH_ATTRIBUTE, IndexedValueType.INDEXED_VALUE_TYPE_KEYWORD)
                      .build());
          break;
        } catch (StatusRuntimeException e) {
          if (e.getStatus().getCode() == Status.Code.ALREADY_EXISTS) {
            break;
          }
          if (Instant.now().isAfter(deadline)) {
            throw e;
          }
          Thread.sleep(2000);
        }
      }
    } finally {
      operator.shutdown();
    }
    Thread.sleep(3000);
  }

  static void generate(WorkflowServiceStubs service, Args args) throws Exception {
    WorkflowClient client =
        WorkflowClient.newInstance(
            service, WorkflowClientOptions.newBuilder().setNamespace(args.namespace()).build());
    WorkerFactory factory = WorkerFactory.newInstance(client);
    // One activity at a time, so the parallel fan-out's activities complete in the order they
    // were scheduled and regenerating is stable.
    Worker worker =
        factory.newWorker(
            Scenarios.QUEUE,
            WorkerOptions.newBuilder().setMaxConcurrentActivityExecutionSize(1).build());
    worker.registerWorkflowImplementationTypes(Workflows.IMPLEMENTATIONS);
    worker.registerActivitiesImplementations(new CorpusActivitiesImpl());
    factory.start();
    List<Map<String, Object>> observations = new ArrayList<>();
    try {
      for (String name : args.pick()) {
        String id = Scenarios.ID_PREFIX + name;
        Observation o;
        try {
          o = new Driver(service, client, args.namespace(), id).drive(name);
        } catch (Exception e) {
          throw new IllegalStateException(name + ": " + e, e);
        }
        o.scenario = name;
        if (o.workflowId == null) {
          o.workflowId = id;
        }
        o.runs = export(service, args.namespace(), o.workflowId, args.out().resolve(name));
        System.out.printf("corpus: %s: %d run(s), %s%n", name, o.runs, o.result);
        observations.add(o.toJson());
      }
    } finally {
      factory.shutdown();
      factory.awaitTermination(30, TimeUnit.SECONDS);
    }
    Map<String, Object> doc = new LinkedHashMap<>();
    doc.put("namespace", args.namespace());
    doc.put("language", "java");
    doc.put("scenarios", observations);
    Path parent = args.out().toAbsolutePath().getParent();
    Files.writeString(
        parent.resolve("generate-java.json"),
        new ObjectMapper().enable(SerializationFeature.INDENT_OUTPUT).writeValueAsString(doc) + "\n");
  }

  /** One scenario's driver: start, signal, query, cancel, update, schedule or reset. */
  static final class Driver {
    private final WorkflowServiceStubs service;
    private final WorkflowClient client;
    private final String namespace;
    private final String id;
    private final Instant deadline = Instant.now().plus(SCENARIO_TIMEOUT);
    private final ObjectMapper json = new ObjectMapper();

    Driver(WorkflowServiceStubs service, WorkflowClient client, String namespace, String id) {
      this.service = service;
      this.client = client;
      this.namespace = namespace;
      this.id = id;
    }

    WorkflowOptions.Builder options() {
      return WorkflowOptions.newBuilder().setWorkflowId(id).setTaskQueue(Scenarios.QUEUE);
    }

    WorkflowStub stub(String type) {
      return client.newUntypedWorkflowStub(type, options().build());
    }

    long remaining() {
      return Math.max(1, Duration.between(Instant.now(), deadline).toMillis());
    }

    /** The result as JSON (what the Go and TypeScript drivers record). */
    String result(WorkflowStub stub) throws Exception {
      Object v = stub.getResult(remaining(), TimeUnit.MILLISECONDS, Object.class);
      return json.writeValueAsString(v);
    }

    /** Waits until the run's first workflow task completed, so what follows lands on a running workflow. */
    void waitStarted() throws InterruptedException {
      while (true) {
        try {
          DescribeWorkflowExecutionResponse d = describe(service, namespace, id, "");
          if (!d.hasPendingWorkflowTask() && d.getWorkflowExecutionInfo().getHistoryLength() >= 4) {
            return;
          }
        } catch (StatusRuntimeException e) {
          if (e.getStatus().getCode() != Status.Code.NOT_FOUND) {
            throw e;
          }
        }
        if (Instant.now().isAfter(deadline)) {
          throw new IllegalStateException(id + ": did not start within " + SCENARIO_TIMEOUT);
        }
        Thread.sleep(200);
      }
    }

    Observation drive(String name) throws Exception {
      Observation o = new Observation();
      switch (name) {
        case "hello" -> {
          WorkflowStub s = stub(name);
          s.start("corpus");
          o.result = result(s);
        }
        case "signal_then_complete" -> {
          WorkflowStub s = stub(name);
          s.start();
          waitStarted();
          s.signal(Scenarios.SIGNAL_GO, "s");
          o.result = result(s);
        }
        case "signal_with_start" -> {
          WorkflowStub s = stub(name);
          s.signalWithStart(Scenarios.SIGNAL_GO, new Object[] {"s"}, new Object[] {});
          o.result = result(s);
        }
        case "query_while_running" -> {
          WorkflowStub s = stub(name);
          s.start();
          waitStarted();
          String state = s.query(Scenarios.QUERY_STATE, String.class);
          if (!"waiting".equals(state)) {
            throw new IllegalStateException("query answered " + state);
          }
          o.expected.add("query state=waiting");
          s.signal(Scenarios.SIGNAL_FINISH);
          o.result = result(s);
        }
        case "cancel_requested", "terminated" -> {
          WorkflowStub s = stub(name);
          s.start();
          waitStarted();
          if (name.equals("cancel_requested")) {
            s.cancel();
          } else {
            s.terminate("corpus terminates it");
          }
          try {
            result(s);
          } catch (WorkflowFailedException e) {
            o.expected.add(String.valueOf(e.getCause()));
            return o;
          }
          throw new IllegalStateException("the workflow completed instead of closing as " + name);
        }
        case "workflow_failed_application_error" -> {
          WorkflowStub s = stub(name);
          s.start();
          try {
            result(s);
          } catch (WorkflowFailedException e) {
            if (e.getCause() instanceof ApplicationFailure af && "CorpusBoom".equals(af.getType())) {
              o.expected.add(String.valueOf(af));
              return o;
            }
            throw e;
          }
          throw new IllegalStateException("want an application error CorpusBoom");
        }
        case "workflow_task_failure_then_fix" -> {
          Workflows.FAIL_WORKFLOW_TASKS.set(1);
          try {
            WorkflowStub s = stub(name);
            s.start();
            o.expected.add("one failed workflow task before the fix");
            o.result = result(s);
          } finally {
            Workflows.FAIL_WORKFLOW_TASKS.set(0);
          }
        }
        case "continue_as_new_3" -> {
          WorkflowStub s = stub(name);
          s.start(0);
          o.result = result(s);
        }
        case "id_reuse_allow_duplicate_failed_only" -> {
          WorkflowOptions opts =
              options()
                  .setWorkflowIdReusePolicy(
                      WorkflowIdReusePolicy.WORKFLOW_ID_REUSE_POLICY_ALLOW_DUPLICATE_FAILED_ONLY)
                  .build();
          WorkflowStub first = client.newUntypedWorkflowStub(name, opts);
          first.start(true);
          try {
            result(first);
            throw new IllegalStateException("the first run should fail");
          } catch (WorkflowFailedException expected) {
            // The first run fails; the policy lets the same ID start again.
          }
          WorkflowStub second = client.newUntypedWorkflowStub(name, opts);
          second.start(false);
          o.expected.add("the first run failed; the second started with the same ID");
          o.result = result(second);
        }
        case "id_conflict_fail" -> {
          WorkflowOptions opts =
              options()
                  .setWorkflowIdConflictPolicy(
                      WorkflowIdConflictPolicy.WORKFLOW_ID_CONFLICT_POLICY_FAIL)
                  .build();
          WorkflowStub s = client.newUntypedWorkflowStub(name, opts);
          s.start();
          try {
            client.newUntypedWorkflowStub(name, opts).start();
            throw new IllegalStateException("the second start should be refused");
          } catch (WorkflowExecutionAlreadyStarted e) {
            o.expected.add("the second start was refused: WorkflowExecutionAlreadyStarted");
          }
          s.signal(Scenarios.SIGNAL_FINISH);
          o.result = result(s);
        }
        case "memo" -> {
          Map<String, Object> memo = new LinkedHashMap<>();
          memo.put("initial", "set at start");
          WorkflowStub s = client.newUntypedWorkflowStub(name, options().setMemo(memo).build());
          s.start();
          o.result = result(s);
        }
        case "update_accepted_completed", "update_rejected" -> {
          WorkflowStub s = stub(name);
          s.start();
          waitStarted();
          boolean reject = name.equals("update_rejected");
          int by = reject ? -1 : 2;
          UpdateOptions<Integer> update =
              UpdateOptions.newBuilder(Integer.class)
                  .setUpdateName(Scenarios.UPDATE_INCREMENT)
                  .setUpdateId(id + "-update")
                  .setWaitForStage(WorkflowUpdateStage.COMPLETED)
                  .build();
          try {
            Integer total = s.startUpdate(update, by).getResult();
            o.expected.add("update returned " + total);
            if (reject) {
              throw new IllegalStateException("the validator should reject a negative increment");
            }
          } catch (WorkflowUpdateException e) {
            if (!reject) {
              throw e;
            }
            o.expected.add("update rejected: " + e.getCause());
          }
          s.signal(Scenarios.SIGNAL_FINISH);
          o.result = result(s);
        }
        case "schedule_triggered" -> scheduleTriggered(name, o);
        case "reset_to_first_workflow_task" -> resetToFirstWorkflowTask(name, o);
        default -> {
          WorkflowStub s = stub(name);
          s.start();
          o.result = result(s);
        }
      }
      return o;
    }

    private void scheduleTriggered(String name, Observation o) throws Exception {
      ScheduleClient schedules =
          ScheduleClient.newInstance(
              service, ScheduleClientOptions.newBuilder().setNamespace(namespace).build());
      Schedule schedule =
          Schedule.newBuilder()
              .setAction(
                  ScheduleActionStartWorkflow.newBuilder()
                      .setWorkflowType(name)
                      .setOptions(
                          WorkflowOptions.newBuilder()
                              .setWorkflowId(id + "-wf")
                              .setTaskQueue(Scenarios.QUEUE)
                              .build())
                      .build())
              .setSpec(ScheduleSpec.newBuilder().build())
              .setState(ScheduleState.newBuilder().setPaused(true).build())
              .build();
      ScheduleHandle handle =
          schedules.createSchedule(id, schedule, ScheduleOptions.newBuilder().build());
      try {
        handle.trigger();
        while (o.workflowId == null) {
          for (ScheduleActionResult r : handle.describe().getInfo().getRecentActions()) {
            ScheduleActionExecution a = r.getAction();
            if (a instanceof ScheduleActionExecutionStartWorkflow sw) {
              o.workflowId = sw.getWorkflowId();
              break;
            }
          }
          if (o.workflowId == null) {
            if (Instant.now().isAfter(deadline)) {
              throw new IllegalStateException(id + ": the schedule started nothing");
            }
            Thread.sleep(300);
          }
        }
        o.expected.add(
            "started by schedule " + id + " (the server appends the action time to the ID)");
        o.result = result(client.newUntypedWorkflowStub(o.workflowId));
      } finally {
        handle.delete();
      }
    }

    private void resetToFirstWorkflowTask(String name, Observation o) throws Exception {
      WorkflowStub s = stub(name);
      WorkflowExecution run = s.start();
      result(s);
      long first = -1;
      for (HistoryEvent e : history(service, namespace, id, run.getRunId()).getEventsList()) {
        if (e.getEventType() == EventType.EVENT_TYPE_WORKFLOW_TASK_COMPLETED) {
          first = e.getEventId();
          break;
        }
      }
      if (first < 0) {
        throw new IllegalStateException("no workflow task completed event");
      }
      ResetWorkflowExecutionResponse r =
          service
              .blockingStub()
              .resetWorkflowExecution(
                  ResetWorkflowExecutionRequest.newBuilder()
                      .setNamespace(namespace)
                      .setWorkflowExecution(
                          WorkflowExecution.newBuilder()
                              .setWorkflowId(id)
                              .setRunId(run.getRunId()))
                      .setReason("corpus: reset to the first workflow task")
                      .setWorkflowTaskFinishEventId(first)
                      .setRequestId(id + "-reset")
                      .build());
      o.expected.add("reset to event " + first);
      o.result =
          result(
              client.newUntypedWorkflowStub(
                  WorkflowExecution.newBuilder().setWorkflowId(id).setRunId(r.getRunId()).build(),
                  Optional.of(name)));
    }
  }

  // --- Export -------------------------------------------------------------------------

  static DescribeWorkflowExecutionResponse describe(
      WorkflowServiceStubs service, String namespace, String id, String runId) {
    return service
        .blockingStub()
        .describeWorkflowExecution(
            DescribeWorkflowExecutionRequest.newBuilder()
                .setNamespace(namespace)
                .setExecution(WorkflowExecution.newBuilder().setWorkflowId(id).setRunId(runId))
                .build());
  }

  /** Every event of one run, all pages. */
  static History history(WorkflowServiceStubs service, String namespace, String id, String runId) {
    History.Builder h = History.newBuilder();
    ByteString token = ByteString.EMPTY;
    do {
      GetWorkflowExecutionHistoryResponse page =
          service
              .blockingStub()
              .getWorkflowExecutionHistory(
                  GetWorkflowExecutionHistoryRequest.newBuilder()
                      .setNamespace(namespace)
                      .setExecution(WorkflowExecution.newBuilder().setWorkflowId(id).setRunId(runId))
                      .setNextPageToken(token)
                      .build());
      h.addAllEvents(page.getHistory().getEventsList());
      token = page.getNextPageToken();
    } while (!token.isEmpty());
    return h.build();
  }

  private record Run(String runId, Timestamp start) {}

  /**
   * Every run of a workflow ID, oldest first, from visibility. A server without visibility (the
   * Loams gateway before DX3 answers ListWorkflowExecutions UNIMPLEMENTED): the current run only,
   * once it has closed.
   */
  static List<String> runsOf(WorkflowServiceStubs service, String namespace, String id)
      throws InterruptedException {
    Instant deadline = Instant.now().plus(Duration.ofSeconds(30));
    String query = "WorkflowId = '" + id + "'";
    try {
      service
          .blockingStub()
          .listWorkflowExecutions(
              ListWorkflowExecutionsRequest.newBuilder()
                  .setNamespace(namespace)
                  .setQuery(query)
                  .setPageSize(1)
                  .build());
    } catch (StatusRuntimeException e) {
      if (e.getStatus().getCode() != Status.Code.UNIMPLEMENTED) {
        throw e;
      }
      while (true) {
        WorkflowExecutionInfo info = describe(service, namespace, id, "").getWorkflowExecutionInfo();
        if (info.getStatus() != WorkflowExecutionStatus.WORKFLOW_EXECUTION_STATUS_RUNNING) {
          return List.of(info.getExecution().getRunId());
        }
        if (Instant.now().isAfter(deadline)) {
          throw new IllegalStateException(id + ": still running");
        }
        Thread.sleep(200);
      }
    }
    while (true) {
      List<Run> runs = new ArrayList<>();
      ByteString token = ByteString.EMPTY;
      do {
        ListWorkflowExecutionsResponse page =
            service
                .blockingStub()
                .listWorkflowExecutions(
                    ListWorkflowExecutionsRequest.newBuilder()
                        .setNamespace(namespace)
                        .setQuery(query)
                        .setNextPageToken(token)
                        .build());
        for (WorkflowExecutionInfo e : page.getExecutionsList()) {
          runs.add(new Run(e.getExecution().getRunId(), e.getStartTime()));
        }
        token = page.getNextPageToken();
      } while (!token.isEmpty());
      // Visibility is eventually consistent: wait until every run is closed.
      boolean closed = true;
      for (Run r : runs) {
        try {
          if (describe(service, namespace, id, r.runId()).getWorkflowExecutionInfo().getStatus()
              == WorkflowExecutionStatus.WORKFLOW_EXECUTION_STATUS_RUNNING) {
            closed = false;
          }
        } catch (StatusRuntimeException e) {
          closed = false;
        }
      }
      if (!runs.isEmpty() && closed) {
        runs.sort(
            Comparator.comparingLong((Run r) -> r.start().getSeconds())
                .thenComparingInt(r -> r.start().getNanos()));
        return runs.stream().map(Run::runId).toList();
      }
      if (Instant.now().isAfter(deadline)) {
        throw new IllegalStateException(id + ": " + runs.size() + " runs listed, not all closed");
      }
      Thread.sleep(1000);
    }
  }

  /** Exports every run of a workflow ID as base.json/base.binpb (or base.run&lt;k&gt;.*). */
  static int export(WorkflowServiceStubs service, String namespace, String id, Path base)
      throws IOException, InterruptedException {
    List<String> runs = runsOf(service, namespace, id);
    for (int i = 0; i < runs.size(); i++) {
      History h = history(service, namespace, id, runs.get(i));
      String name = base.getFileName() + (runs.size() > 1 ? ".run" + (i + 1) : "");
      Path dir = base.toAbsolutePath().getParent();
      write(h, id, dir.resolve(name + ".json"), dir.resolve(name + ".binpb"));
    }
    return runs.size();
  }

  /** The protojson the SDK's own WorkflowExecutionHistory writes, and the binary History. */
  static void write(History h, String workflowId, Path json, Path binpb) throws IOException {
    String text = new io.temporal.common.WorkflowExecutionHistory(h, workflowId).toJson(true);
    Files.writeString(json, text + "\n", StandardCharsets.UTF_8);
    Files.write(binpb, h.toByteArray());
  }
}
