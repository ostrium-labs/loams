package dev.loams.conformance.temporal.corpus;

import java.util.List;

/**
 * The corpus's scenarios in plan order (DX1 plan, DX0 Task 4), as the Java worker runs them. The
 * Java SDK covers every scenario of {@code scenarios.json}, the Go-only ones included: it has
 * side effects, a workflow task that fails on an exception, schedules and resets.
 * manifest.py check compares this list with scenarios.json (the {@code new Scenario("name",
 * "DXn")} lines, in order). A change to a workflow changes its histories: regenerate
 * (generate-java.sh) in the same pull request.
 */
public final class Scenarios {
  private Scenarios() {}

  /** One scenario: its name (the workflow type and the ID suffix) and its phase. */
  public record Scenario(String name, String phase) {}

  public static final List<Scenario> ALL =
      List.of(
          new Scenario("hello", "DX1"),
          new Scenario("two_activities_sequential", "DX1"),
          new Scenario("activity_parallel_fanout_10", "DX1"),
          new Scenario("signal_then_complete", "DX1"),
          new Scenario("query_while_running", "DX1"),
          new Scenario("cancel_requested", "DX1"),
          new Scenario("terminated", "DX1"),
          new Scenario("workflow_failed_application_error", "DX1"),
          new Scenario("activity_heartbeat_details", "DX1"),
          new Scenario("timer_short", "DX2"),
          new Scenario("timer_cancelled", "DX2"),
          new Scenario("activity_retry_backoff_3_attempts", "DX2"),
          new Scenario("activity_start_to_close_timeout", "DX2"),
          new Scenario("activity_schedule_to_start_timeout", "DX2"),
          new Scenario("activity_heartbeat_timeout", "DX2"),
          new Scenario("workflow_task_failure_then_fix", "DX2"),
          new Scenario("signal_with_start", "DX2"),
          new Scenario("child_workflow", "DX2"),
          new Scenario("child_workflow_cancelled", "DX2"),
          new Scenario("continue_as_new_3", "DX2"),
          new Scenario("id_reuse_allow_duplicate_failed_only", "DX2"),
          new Scenario("id_conflict_fail", "DX2"),
          new Scenario("local_activity", "DX2"),
          new Scenario("side_effect_and_version_marker", "DX2"),
          new Scenario("search_attributes_upsert", "DX3"),
          new Scenario("memo", "DX3"),
          new Scenario("large_payload_1mb", "DX3"),
          new Scenario("update_accepted_completed", "DX4"),
          new Scenario("update_rejected", "DX4"),
          new Scenario("schedule_triggered", "DX4"),
          new Scenario("reset_to_first_workflow_task", "DX4"));

  /** The task queue of the Java worker. */
  public static final String QUEUE = "corpus-java";

  /** The workflow ID prefix: corpus-java-&lt;scenario&gt;. */
  public static final String ID_PREFIX = "corpus-java-";

  /** A queue no worker polls: an activity there times out before it starts. */
  public static final String NO_WORKER_QUEUE = "corpus-no-worker";

  // Signal, query and update names the generator uses (the Go and TypeScript ones).
  public static final String SIGNAL_GO = "go";
  public static final String SIGNAL_FINISH = "finish";
  public static final String QUERY_STATE = "state";
  public static final String UPDATE_INCREMENT = "increment";

  /** The custom keyword search attribute search_attributes_upsert sets. */
  public static final String SEARCH_ATTRIBUTE = "CorpusKeyword";

  /** The scenario of a history file name: hello.json, continue_as_new_3.run2.json. */
  public static String scenarioOf(String fileName) {
    return fileName.replaceAll("(\\.run[0-9]+)?\\.json$", "");
  }
}
