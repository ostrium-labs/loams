package dev.loams.conformance.temporal.corpus;

import io.temporal.activity.ActivityOptions;
import io.temporal.activity.LocalActivityOptions;
import io.temporal.common.RetryOptions;
import io.temporal.common.SearchAttributeKey;
import io.temporal.failure.ActivityFailure;
import io.temporal.failure.ApplicationFailure;
import io.temporal.failure.CanceledFailure;
import io.temporal.failure.ChildWorkflowFailure;
import io.temporal.failure.TimeoutFailure;
import io.temporal.workflow.Async;
import io.temporal.workflow.CancellationScope;
import io.temporal.workflow.ChildWorkflowOptions;
import io.temporal.workflow.Promise;
import io.temporal.workflow.QueryMethod;
import io.temporal.workflow.SignalMethod;
import io.temporal.workflow.UpdateMethod;
import io.temporal.workflow.UpdateValidatorMethod;
import io.temporal.workflow.Workflow;
import io.temporal.workflow.WorkflowInterface;
import io.temporal.workflow.WorkflowMethod;
import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The corpus's Java workflows (plan DX1, lane TSDK-java): one workflow interface per scenario,
 * registered under the scenario's name, so the generator and the replayer agree on what each
 * history runs. They do what the Go workflows (workers/go/scenarios) do, in Java's idiom. A change
 * here changes the histories: regenerate (generate-java.sh) in the same pull request.
 */
public final class Workflows {
  private Workflows() {}

  /**
   * How many workflow tasks of workflow_task_failure_then_fix throw before the "fix": the
   * generator sets it to 1, so exactly one WorkflowTaskFailed is recorded and the retried task
   * completes. The replayer leaves it at 0. It is process state on purpose (not a side effect).
   */
  public static final AtomicInteger FAIL_WORKFLOW_TASKS = new AtomicInteger();

  /** Every workflow implementation, the children included: the worker and the replayer register these. */
  public static final Class<?>[] IMPLEMENTATIONS = {
    HelloImpl.class,
    TwoActivitiesSequentialImpl.class,
    ActivityParallelFanout10Impl.class,
    SignalThenCompleteImpl.class,
    QueryWhileRunningImpl.class,
    CancelRequestedImpl.class,
    TerminatedImpl.class,
    WorkflowFailedApplicationErrorImpl.class,
    ActivityHeartbeatDetailsImpl.class,
    TimerShortImpl.class,
    TimerCancelledImpl.class,
    ActivityRetryBackoff3AttemptsImpl.class,
    ActivityStartToCloseTimeoutImpl.class,
    ActivityScheduleToStartTimeoutImpl.class,
    ActivityHeartbeatTimeoutImpl.class,
    WorkflowTaskFailureThenFixImpl.class,
    SignalWithStartImpl.class,
    ChildGreeterImpl.class,
    ChildSleeperImpl.class,
    ChildWorkflowImpl.class,
    ChildWorkflowCancelledImpl.class,
    ContinueAsNew3Impl.class,
    IdReuseAllowDuplicateFailedOnlyImpl.class,
    IdConflictFailImpl.class,
    LocalActivityImpl.class,
    SideEffectAndVersionMarkerImpl.class,
    SearchAttributesUpsertImpl.class,
    MemoImpl.class,
    LargePayload1mbImpl.class,
    UpdateAcceptedCompletedImpl.class,
    UpdateRejectedImpl.class,
    ScheduleTriggeredImpl.class,
    ResetToFirstWorkflowTaskImpl.class,
  };

  // --- Helpers ------------------------------------------------------------------------

  private static final RetryOptions ONCE = RetryOptions.newBuilder().setMaximumAttempts(1).build();

  /** One attempt, 30 s start-to-close: the Go scenarios' ao(). */
  static CorpusActivities activities() {
    return Workflow.newActivityStub(
        CorpusActivities.class,
        ActivityOptions.newBuilder()
            .setStartToCloseTimeout(Duration.ofSeconds(30))
            .setRetryOptions(ONCE)
            .build());
  }

  static CorpusActivities activities(ActivityOptions options) {
    return Workflow.newActivityStub(CorpusActivities.class, options);
  }

  /** The timeout type of an activity failure, as the Go scenarios report it. */
  static String timeoutKind(ActivityFailure err) {
    if (err.getCause() instanceof TimeoutFailure t) {
      return t.getTimeoutType().name();
    }
    return "not a timeout: " + err.getCause();
  }

  // --- DX1 ----------------------------------------------------------------------------

  @WorkflowInterface
  public interface Hello {
    @WorkflowMethod(name = "hello")
    String run(String name);
  }

  public static class HelloImpl implements Hello {
    @Override
    public String run(String name) {
      return activities().greet(name);
    }
  }

  @WorkflowInterface
  public interface TwoActivitiesSequential {
    @WorkflowMethod(name = "two_activities_sequential")
    String run();
  }

  public static class TwoActivitiesSequentialImpl implements TwoActivitiesSequential {
    @Override
    public String run() {
      CorpusActivities a = activities();
      String one = a.greet("one");
      String two = a.greet("two");
      return one + ", " + two;
    }
  }

  @WorkflowInterface
  public interface ActivityParallelFanout10 {
    @WorkflowMethod(name = "activity_parallel_fanout_10")
    List<String> run();
  }

  public static class ActivityParallelFanout10Impl implements ActivityParallelFanout10 {
    @Override
    public List<String> run() {
      CorpusActivities a = activities();
      List<Promise<String>> futures = new ArrayList<>();
      for (int i = 0; i < 10; i++) {
        String name = "n" + i;
        futures.add(Async.function(a::greet, name));
      }
      List<String> out = new ArrayList<>();
      for (Promise<String> f : futures) {
        out.add(f.get());
      }
      return out;
    }
  }

  @WorkflowInterface
  public interface SignalThenComplete {
    @WorkflowMethod(name = "signal_then_complete")
    String run();

    @SignalMethod(name = Scenarios.SIGNAL_GO)
    void go(String value);
  }

  public static class SignalThenCompleteImpl implements SignalThenComplete {
    private String value;

    @Override
    public String run() {
      Workflow.await(() -> value != null);
      return "got " + value;
    }

    @Override
    public void go(String value) {
      this.value = value;
    }
  }

  @WorkflowInterface
  public interface QueryWhileRunning {
    @WorkflowMethod(name = "query_while_running")
    String run();

    @QueryMethod(name = Scenarios.QUERY_STATE)
    String state();

    @SignalMethod(name = Scenarios.SIGNAL_FINISH)
    void finish();
  }

  public static class QueryWhileRunningImpl implements QueryWhileRunning {
    private String state = "waiting";
    private boolean done;

    @Override
    public String run() {
      Workflow.await(() -> done);
      state = "done";
      return state;
    }

    @Override
    public String state() {
      return state;
    }

    @Override
    public void finish() {
      done = true;
    }
  }

  @WorkflowInterface
  public interface CancelRequested {
    @WorkflowMethod(name = "cancel_requested")
    void run();
  }

  public static class CancelRequestedImpl implements CancelRequested {
    @Override
    public void run() {
      // Cancelled while it sleeps: the workflow closes as cancelled.
      Workflow.sleep(Duration.ofHours(1));
    }
  }

  @WorkflowInterface
  public interface Terminated {
    @WorkflowMethod(name = "terminated")
    void run();

    @SignalMethod(name = Scenarios.SIGNAL_FINISH)
    void finish();
  }

  public static class TerminatedImpl implements Terminated {
    private boolean done;

    @Override
    public void run() {
      Workflow.await(() -> done);
    }

    @Override
    public void finish() {
      done = true;
    }
  }

  @WorkflowInterface
  public interface WorkflowFailedApplicationError {
    @WorkflowMethod(name = "workflow_failed_application_error")
    void run();
  }

  public static class WorkflowFailedApplicationErrorImpl implements WorkflowFailedApplicationError {
    @Override
    public void run() {
      throw ApplicationFailure.newNonRetryableFailure("boom", "CorpusBoom", "detail");
    }
  }

  @WorkflowInterface
  public interface ActivityHeartbeatDetails {
    @WorkflowMethod(name = "activity_heartbeat_details")
    int run();
  }

  public static class ActivityHeartbeatDetailsImpl implements ActivityHeartbeatDetails {
    @Override
    public int run() {
      return activities(
              ActivityOptions.newBuilder()
                  .setStartToCloseTimeout(Duration.ofSeconds(30))
                  .setHeartbeatTimeout(Duration.ofSeconds(10))
                  .setRetryOptions(ONCE)
                  .build())
          .heartbeatThrice();
    }
  }

  // --- DX2 ----------------------------------------------------------------------------

  @WorkflowInterface
  public interface TimerShort {
    @WorkflowMethod(name = "timer_short")
    void run();
  }

  public static class TimerShortImpl implements TimerShort {
    @Override
    public void run() {
      Workflow.sleep(Duration.ofSeconds(1));
    }
  }

  @WorkflowInterface
  public interface TimerCancelled {
    @WorkflowMethod(name = "timer_cancelled")
    String run();
  }

  public static class TimerCancelledImpl implements TimerCancelled {
    @Override
    public String run() {
      List<Promise<Void>> timer = new ArrayList<>();
      CancellationScope scope =
          Workflow.newCancellationScope(() -> timer.add(Workflow.newTimer(Duration.ofHours(1))));
      scope.run();
      Workflow.sleep(Duration.ofMillis(100));
      scope.cancel();
      try {
        timer.get(0).get();
      } catch (CanceledFailure e) {
        return "cancelled";
      }
      throw ApplicationFailure.newNonRetryableFailure("the long timer was not cancelled", "CorpusBug");
    }
  }

  @WorkflowInterface
  public interface ActivityRetryBackoff3Attempts {
    @WorkflowMethod(name = "activity_retry_backoff_3_attempts")
    int run();
  }

  public static class ActivityRetryBackoff3AttemptsImpl implements ActivityRetryBackoff3Attempts {
    @Override
    public int run() {
      return activities(
              ActivityOptions.newBuilder()
                  .setStartToCloseTimeout(Duration.ofSeconds(10))
                  .setRetryOptions(
                      RetryOptions.newBuilder()
                          .setInitialInterval(Duration.ofSeconds(1))
                          .setBackoffCoefficient(2)
                          .setMaximumAttempts(3)
                          .build())
                  .build())
          .failTwice();
    }
  }

  @WorkflowInterface
  public interface ActivityStartToCloseTimeout {
    @WorkflowMethod(name = "activity_start_to_close_timeout")
    String run();
  }

  public static class ActivityStartToCloseTimeoutImpl implements ActivityStartToCloseTimeout {
    @Override
    public String run() {
      try {
        activities(
                ActivityOptions.newBuilder()
                    .setStartToCloseTimeout(Duration.ofSeconds(1))
                    .setRetryOptions(ONCE)
                    .build())
            .sleepFor(5000);
        return "no timeout";
      } catch (ActivityFailure e) {
        return timeoutKind(e);
      }
    }
  }

  @WorkflowInterface
  public interface ActivityScheduleToStartTimeout {
    @WorkflowMethod(name = "activity_schedule_to_start_timeout")
    String run();
  }

  public static class ActivityScheduleToStartTimeoutImpl implements ActivityScheduleToStartTimeout {
    @Override
    public String run() {
      try {
        activities(
                ActivityOptions.newBuilder()
                    .setTaskQueue(Scenarios.NO_WORKER_QUEUE)
                    .setScheduleToStartTimeout(Duration.ofSeconds(1))
                    .setStartToCloseTimeout(Duration.ofSeconds(1))
                    .setRetryOptions(ONCE)
                    .build())
            .greet("nobody");
        return "no timeout";
      } catch (ActivityFailure e) {
        return timeoutKind(e);
      }
    }
  }

  @WorkflowInterface
  public interface ActivityHeartbeatTimeout {
    @WorkflowMethod(name = "activity_heartbeat_timeout")
    String run();
  }

  public static class ActivityHeartbeatTimeoutImpl implements ActivityHeartbeatTimeout {
    @Override
    public String run() {
      try {
        activities(
                ActivityOptions.newBuilder()
                    .setStartToCloseTimeout(Duration.ofSeconds(30))
                    .setHeartbeatTimeout(Duration.ofSeconds(1))
                    .setRetryOptions(ONCE)
                    .build())
            .sleepFor(5000);
        return "no timeout";
      } catch (ActivityFailure e) {
        return timeoutKind(e);
      }
    }
  }

  @WorkflowInterface
  public interface WorkflowTaskFailureThenFix {
    @WorkflowMethod(name = "workflow_task_failure_then_fix")
    String run();
  }

  public static class WorkflowTaskFailureThenFixImpl implements WorkflowTaskFailureThenFix {
    @Override
    public String run() {
      // A plain RuntimeException (not a TemporalFailure) fails the workflow task, not the
      // workflow; the server retries the task and the counter lets the retry through.
      if (FAIL_WORKFLOW_TASKS.decrementAndGet() >= 0) {
        throw new IllegalStateException("corpus: the first workflow task fails");
      }
      return activities().greet("after the fix");
    }
  }

  @WorkflowInterface
  public interface SignalWithStart {
    @WorkflowMethod(name = "signal_with_start")
    String run();

    @SignalMethod(name = Scenarios.SIGNAL_GO)
    void go(String value);
  }

  public static class SignalWithStartImpl implements SignalWithStart {
    private String value;

    @Override
    public String run() {
      Workflow.await(() -> value != null);
      return "started with " + value;
    }

    @Override
    public void go(String value) {
      this.value = value;
    }
  }

  @WorkflowInterface
  public interface ChildGreeter {
    @WorkflowMethod(name = "child_greeter")
    String run(String name);
  }

  public static class ChildGreeterImpl implements ChildGreeter {
    @Override
    public String run(String name) {
      return activities().greet(name);
    }
  }

  @WorkflowInterface
  public interface ChildSleeper {
    @WorkflowMethod(name = "child_sleeper")
    void run();
  }

  public static class ChildSleeperImpl implements ChildSleeper {
    @Override
    public void run() {
      Workflow.sleep(Duration.ofHours(1));
    }
  }

  static ChildWorkflowOptions childOptions() {
    return ChildWorkflowOptions.newBuilder()
        .setWorkflowId(Workflow.getInfo().getWorkflowId() + "-child")
        .build();
  }

  @WorkflowInterface
  public interface ChildWorkflow {
    @WorkflowMethod(name = "child_workflow")
    String run();
  }

  public static class ChildWorkflowImpl implements ChildWorkflow {
    @Override
    public String run() {
      return Workflow.newChildWorkflowStub(ChildGreeter.class, childOptions()).run("child");
    }
  }

  @WorkflowInterface
  public interface ChildWorkflowCancelled {
    @WorkflowMethod(name = "child_workflow_cancelled")
    String run();
  }

  public static class ChildWorkflowCancelledImpl implements ChildWorkflowCancelled {
    @Override
    public String run() {
      ChildSleeper child = Workflow.newChildWorkflowStub(ChildSleeper.class, childOptions());
      List<Promise<Void>> result = new ArrayList<>();
      CancellationScope scope =
          Workflow.newCancellationScope(() -> result.add(Async.procedure(child::run)));
      scope.run();
      // Cancel once the child has started, as the Go scenario does.
      Workflow.getWorkflowExecution(child).get();
      scope.cancel();
      try {
        result.get(0).get();
      } catch (ChildWorkflowFailure e) {
        if (e.getCause() instanceof CanceledFailure) {
          return "child cancelled";
        }
        throw e;
      } catch (CanceledFailure e) {
        return "child cancelled";
      }
      throw ApplicationFailure.newNonRetryableFailure("the child was not cancelled", "CorpusBug");
    }
  }

  @WorkflowInterface
  public interface ContinueAsNew3 {
    @WorkflowMethod(name = "continue_as_new_3")
    int run(int n);
  }

  public static class ContinueAsNew3Impl implements ContinueAsNew3 {
    @Override
    public int run(int n) {
      if (n < 3) {
        Workflow.continueAsNew(n + 1);
      }
      return n;
    }
  }

  /**
   * Fails on its first run and succeeds on the second, which the generator starts with the same ID
   * under ALLOW_DUPLICATE_FAILED_ONLY.
   */
  @WorkflowInterface
  public interface IdReuseAllowDuplicateFailedOnly {
    @WorkflowMethod(name = "id_reuse_allow_duplicate_failed_only")
    String run(boolean fail);
  }

  public static class IdReuseAllowDuplicateFailedOnlyImpl
      implements IdReuseAllowDuplicateFailedOnly {
    @Override
    public String run(boolean fail) {
      if (fail) {
        throw ApplicationFailure.newNonRetryableFailure("first run fails", "CorpusFirstRun");
      }
      return "second run";
    }
  }

  /** Runs until signalled; a second start with the same ID under conflict policy FAIL is refused in between. */
  @WorkflowInterface
  public interface IdConflictFail {
    @WorkflowMethod(name = "id_conflict_fail")
    void run();

    @SignalMethod(name = Scenarios.SIGNAL_FINISH)
    void finish();
  }

  public static class IdConflictFailImpl implements IdConflictFail {
    private boolean done;

    @Override
    public void run() {
      Workflow.await(() -> done);
    }

    @Override
    public void finish() {
      done = true;
    }
  }

  @WorkflowInterface
  public interface LocalActivity {
    @WorkflowMethod(name = "local_activity")
    String run();
  }

  public static class LocalActivityImpl implements LocalActivity {
    @Override
    public String run() {
      CorpusActivities local =
          Workflow.newLocalActivityStub(
              CorpusActivities.class,
              LocalActivityOptions.newBuilder().setStartToCloseTimeout(Duration.ofSeconds(10)).build());
      return local.greet("local");
    }
  }

  @WorkflowInterface
  public interface SideEffectAndVersionMarker {
    @WorkflowMethod(name = "side_effect_and_version_marker")
    String run();
  }

  public static class SideEffectAndVersionMarkerImpl implements SideEffectAndVersionMarker {
    @Override
    public String run() {
      int v = Workflow.getVersion("corpus-change", Workflow.DEFAULT_VERSION, 1);
      int n = Workflow.sideEffect(Integer.class, () -> 42);
      return "version " + v + " side effect " + n;
    }
  }

  // --- DX3 ----------------------------------------------------------------------------

  static final SearchAttributeKey<String> CORPUS_KEYWORD =
      SearchAttributeKey.forKeyword(Scenarios.SEARCH_ATTRIBUTE);

  @WorkflowInterface
  public interface SearchAttributesUpsert {
    @WorkflowMethod(name = "search_attributes_upsert")
    void run();
  }

  public static class SearchAttributesUpsertImpl implements SearchAttributesUpsert {
    @Override
    public void run() {
      Workflow.upsertTypedSearchAttributes(CORPUS_KEYWORD.valueSet("first"));
      Workflow.sleep(Duration.ofMillis(100));
      Workflow.upsertTypedSearchAttributes(CORPUS_KEYWORD.valueSet("second"));
    }
  }

  @WorkflowInterface
  public interface Memo {
    @WorkflowMethod(name = "memo")
    void run();
  }

  public static class MemoImpl implements Memo {
    @Override
    public void run() {
      // Insertion order, so the command is the same on every run and replay.
      Map<String, Object> memo = new LinkedHashMap<>();
      memo.put("step", "upserted");
      memo.put("n", 2);
      Workflow.upsertMemo(memo);
    }
  }

  @WorkflowInterface
  public interface LargePayload1mb {
    @WorkflowMethod(name = "large_payload_1mb")
    int run();
  }

  public static class LargePayload1mbImpl implements LargePayload1mb {
    @Override
    public int run() {
      return activities().large(1 << 20).length();
    }
  }

  // --- DX4 ----------------------------------------------------------------------------

  /** The update scenarios' state: a running total, a validator refusing negatives, and finish. */
  public abstract static class Updates {
    private int total;
    private boolean done;

    public String run() {
      Workflow.await(() -> done);
      return "total " + total;
    }

    public int increment(int by) {
      total += by;
      return total;
    }

    public void validateIncrement(int by) {
      if (by < 0) {
        throw new IllegalArgumentException("increment must not be negative");
      }
    }

    public void finish() {
      done = true;
    }
  }

  @WorkflowInterface
  public interface UpdateAcceptedCompleted {
    @WorkflowMethod(name = "update_accepted_completed")
    String run();

    @UpdateMethod(name = Scenarios.UPDATE_INCREMENT)
    int increment(int by);

    @UpdateValidatorMethod(updateName = Scenarios.UPDATE_INCREMENT)
    void validateIncrement(int by);

    @SignalMethod(name = Scenarios.SIGNAL_FINISH)
    void finish();
  }

  public static class UpdateAcceptedCompletedImpl extends Updates
      implements UpdateAcceptedCompleted {}

  @WorkflowInterface
  public interface UpdateRejected {
    @WorkflowMethod(name = "update_rejected")
    String run();

    @UpdateMethod(name = Scenarios.UPDATE_INCREMENT)
    int increment(int by);

    @UpdateValidatorMethod(updateName = Scenarios.UPDATE_INCREMENT)
    void validateIncrement(int by);

    @SignalMethod(name = Scenarios.SIGNAL_FINISH)
    void finish();
  }

  public static class UpdateRejectedImpl extends Updates implements UpdateRejected {}

  @WorkflowInterface
  public interface ScheduleTriggered {
    @WorkflowMethod(name = "schedule_triggered")
    String run();
  }

  public static class ScheduleTriggeredImpl implements ScheduleTriggered {
    @Override
    public String run() {
      return activities().greet("schedule");
    }
  }

  @WorkflowInterface
  public interface ResetToFirstWorkflowTask {
    @WorkflowMethod(name = "reset_to_first_workflow_task")
    String run();
  }

  public static class ResetToFirstWorkflowTaskImpl implements ResetToFirstWorkflowTask {
    @Override
    public String run() {
      CorpusActivities a = activities();
      String before = a.greet("before reset");
      String after = a.greet("after");
      return before + ", " + after;
    }
  }
}
