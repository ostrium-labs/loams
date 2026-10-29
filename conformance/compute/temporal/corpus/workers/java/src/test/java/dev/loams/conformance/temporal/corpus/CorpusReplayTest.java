package dev.loams.conformance.temporal.corpus;

import static org.junit.jupiter.api.Assumptions.assumeTrue;

import io.temporal.common.WorkflowExecutionHistory;
import io.temporal.testing.WorkflowReplayer;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.stream.Stream;
import org.junit.jupiter.api.DynamicTest;
import org.junit.jupiter.api.TestFactory;

/**
 * corpus_replays_java (and, on the gateway's histories, gateway_histories_replay_java): every Java
 * history replays with the pinned SDK's WorkflowReplayer and zero nondeterminism errors. One
 * dynamic test per history file, so the surefire report names the one that fails.
 *
 * <p>CORPUS_HISTORIES is the directory (default: the committed corpus, ../../histories/java);
 * CORPUS_WORKFLOW_ID_PREFIX the generator's ID prefix (default corpus-java-). The replayer is given
 * the original workflow ID: child workflow IDs derive from it. Without CORPUS_HISTORIES, a missing committed corpus skips the test (it lands
 * after the first generation); a directory named by CORPUS_HISTORIES must exist.
 */
class CorpusReplayTest {
  static Path dir() {
    String env = System.getenv("CORPUS_HISTORIES");
    return Path.of(env != null && !env.isEmpty() ? env : "../../histories/java");
  }

  static String prefix() {
    String env = System.getenv("CORPUS_WORKFLOW_ID_PREFIX");
    return env != null && !env.isEmpty() ? env : Scenarios.ID_PREFIX;
  }

  @TestFactory
  Stream<DynamicTest> corpus_replays_java() throws IOException {
    Path dir = dir();
    String env = System.getenv("CORPUS_HISTORIES");
    if (env != null && !env.isEmpty()) {
      // Named explicitly (the gateway leg): a missing directory is a failure.
      if (!Files.isDirectory(dir)) {
        throw new AssertionError("no Java histories at " + dir.toAbsolutePath());
      }
    } else {
      assumeTrue(Files.isDirectory(dir), "no committed Java corpus at " + dir.toAbsolutePath());
    }
    List<Path> files;
    try (Stream<Path> s = Files.list(dir)) {
      files = s.filter(p -> p.getFileName().toString().endsWith(".json")).sorted().toList();
    }
    if (files.isEmpty()) {
      throw new AssertionError("no histories in " + dir.toAbsolutePath());
    }
    Class<?>[] impls = Workflows.IMPLEMENTATIONS;
    Class<?>[] more = Arrays.copyOfRange(impls, 1, impls.length);
    return files.stream()
        .map(
            f ->
                DynamicTest.dynamicTest(
                    f.getFileName().toString(),
                    () -> {
                      String name = Scenarios.scenarioOf(f.getFileName().toString());
                      String json = Files.readString(f, StandardCharsets.UTF_8);
                      WorkflowExecutionHistory history =
                          WorkflowExecutionHistory.fromJson(json, prefix() + name);
                      WorkflowReplayer.replayWorkflowExecution(history, impls[0], more);
                    }));
  }
}
