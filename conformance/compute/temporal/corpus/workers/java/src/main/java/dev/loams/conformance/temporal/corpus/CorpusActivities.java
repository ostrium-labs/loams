package dev.loams.conformance.temporal.corpus;

import io.temporal.activity.ActivityInterface;
import io.temporal.activity.ActivityMethod;

/** The corpus's activities, under the Go and TypeScript activity type names. */
@ActivityInterface
public interface CorpusActivities {
  @ActivityMethod(name = "greet")
  String greet(String name);

  @ActivityMethod(name = "heartbeat_thrice")
  int heartbeatThrice();

  @ActivityMethod(name = "fail_twice")
  int failTwice();

  /** Sleeps without heartbeating (the timeout scenarios). */
  @ActivityMethod(name = "sleep_for")
  void sleepFor(long millis);

  @ActivityMethod(name = "large")
  String large(int n);
}
