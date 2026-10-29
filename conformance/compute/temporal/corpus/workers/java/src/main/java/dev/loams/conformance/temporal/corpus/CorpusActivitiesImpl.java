package dev.loams.conformance.temporal.corpus;

import io.temporal.activity.Activity;
import io.temporal.failure.ApplicationFailure;

/** The corpus's activities, as the Go and TypeScript ones behave. */
public class CorpusActivitiesImpl implements CorpusActivities {
  @Override
  public String greet(String name) {
    return "hello " + name;
  }

  @Override
  public int heartbeatThrice() {
    for (int i = 1; i <= 3; i++) {
      Activity.getExecutionContext().heartbeat(i);
    }
    return 3;
  }

  @Override
  public int failTwice() {
    int attempt = Activity.getExecutionContext().getInfo().getAttempt();
    if (attempt < 3) {
      throw ApplicationFailure.newFailure("attempt " + attempt + " fails", "Transient");
    }
    return attempt;
  }

  @Override
  public void sleepFor(long millis) {
    try {
      Thread.sleep(millis);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw Activity.wrap(e);
    }
  }

  @Override
  public String large(int n) {
    return "x".repeat(n);
  }
}
