package dev.loams;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import org.junit.runner.Result;
import org.junit.runner.notification.Failure;

/**
 * The suite's entry point: JUnit's runner with a name filter.
 *
 * <p>JUnit 4 has no {@code --filter} on the command line, and {@code sdks/conformance} names the
 * six canonical tests individually ({@code run-test.sh <language> <test>}). A script that
 * <em>pretended</em> to filter and quietly ran everything would be the worst outcome available: the
 * harness would report a green {@code java_retry_reuses_idempotency_key} whether or not that test
 * existed. So the filter is real, and it is implemented here rather than faked in the shell.
 *
 * <pre>{@code
 * java dev.loams.RunTests                                  # the whole suite
 * java dev.loams.RunTests -t pagination                    # methods containing "pagination"
 * java dev.loams.RunTests -m PaginationTest                # one class
 * }</pre>
 *
 * <p>A filter that matches <b>nothing</b> is a failure, not a quiet pass. That is the whole reason
 * this class exists rather than a shell pipeline.
 */
public final class RunTests {

    private RunTests() {}

    /** The suite, in the order {@code build.sh} runs it. */
    static final List<String> SUITE =
            List.of(
                    "dev.loams.ConformanceTest",
                    "dev.loams.RetryIdempotencyTest",
                    "dev.loams.ErrorReasonMappingTest",
                    "dev.loams.StreamResumeTest",
                    "dev.loams.TokenSourceTest",
                    "dev.loams.PaginationTest",
                    "dev.loams.RuntimeContractTest");

    public static void main(String[] args) throws Exception {
        String methodFilter = null;
        List<String> classes = new ArrayList<>();

        for (int i = 0; i < args.length; i++) {
            switch (args[i]) {
                case "-t" -> {
                    if (++i >= args.length) {
                        System.err.println("-t needs a method-name substring");
                        System.exit(2);
                    }
                    methodFilter = args[i];
                }
                case "-m" -> {
                    if (++i >= args.length) {
                        System.err.println("-m needs a class name");
                        System.exit(2);
                    }
                    classes.add(args[i]);
                }
                default -> {
                    System.err.println("usage: RunTests [-t <method substring>] [-m <class>]...");
                    System.exit(2);
                }
            }
        }
        if (classes.isEmpty()) {
            classes.addAll(SUITE);
        }

        List<Result> results = new ArrayList<>();
        for (String className : classes) {
            Class<?> type = Class.forName(className);
            if (methodFilter == null) {
                results.add(new JUnitCoreRunner().run(type));
                continue;
            }
            // A filtered run is a run of individual methods, so each request names one. That is
            // slower than a class run and is the point: a method that does not exist is not run.
            for (java.lang.reflect.Method method : type.getMethods()) {
                if (!isTest(method) || !method.getName().contains(methodFilter)) {
                    continue;
                }
                results.add(new JUnitCoreRunner().run(type, method.getName()));
            }
        }

        // A filter that matched nothing is the failure mode this class exists to prevent.
        if (methodFilter != null && results.isEmpty()) {
            System.err.println(
                    "no test method in "
                            + classes
                            + " contains \""
                            + methodFilter
                            + "\". A filter that matches nothing would otherwise look like a"
                            + " passing run of a test that does not exist.");
            System.exit(1);
        }

        int run = 0;
        int failed = 0;
        for (Result result : results) {
            run += result.getRunCount();
            failed += result.getFailureCount();
            for (Failure failure : result.getFailures()) {
                System.err.println();
                System.err.println(failure.getTestHeader());
                System.err.println(failure.getMessage());
                failure.getException().printStackTrace(System.err);
            }
        }
        System.out.println();
        System.out.println(
                "Tests run: " + run + (methodFilter == null ? ", Failures: " + failed : ", filtered by \"" + methodFilter + "\", Failures: " + failed));
        if (failed > 0) {
            System.out.println("FAILURES!!!");
            System.exit(1);
        }
        System.out.println("OK");
    }

    /** Whether a method is a JUnit 4 {@code @Test}. */
    private static boolean isTest(java.lang.reflect.Method method) {
        return Arrays.stream(method.getAnnotations())
                .anyMatch(
                        annotation ->
                                annotation.annotationType()
                                        .getName()
                                        .equals("org.junit.Test"));
    }

    /** A thin wrapper so the loop above reads as one line per run. */
    private static final class JUnitCoreRunner {

        Result run(Class<?> type) {
            return org.junit.runner.JUnitCore.runClasses(type);
        }

        Result run(Class<?> type, String methodName) {
            return new org.junit.runner.JUnitCore()
                    .run(org.junit.runner.Request.method(type, methodName));
        }
    }
}