package dev.loams.test

import kotlin.system.exitProcess

/**
 * The test harness (SDK2 Task 5).
 *
 * ## Why this exists rather than JUnit
 *
 * Three reasons, and the third is the one that matters:
 *
 *  1. **No resolver.** `build.sh` is `kotlinc` plus `java` plus one pinned jar,
 *     because there is no Gradle or Maven on the machines this SDK is developed
 *     on (`build.sh` says so). JUnit 5 would add a launcher jar and a module
 *     path to that list for six test names.
 *  2. **A green run of zero tests is the failure mode `run-test.sh` already had
 *     to be taught to distrust.** `D651` records `dotnet test --filter` printing
 *     "No test matches the given testcase filter" and **exiting 0**. A harness
 *     that prints how many tests it ran, and **exits 2 when it ran none**, makes
 *     that class of bug impossible here rather than checked for elsewhere.
 *  3. **A filter that matches nothing is a failure here**, for the same reason. If
 *     somebody renames `kotlin_pagination_iterator` by one character and asks for
 *     the contract name, this runner says so and exits non-zero.
 *
 * ## The shape
 *
 * A test is a named block registered by name. `RunTests` runs every one whose
 * name contains a filter, and reports the failures per test rather than stopping
 * at the first, so one run learns about all of them.
 */
object Harness {
    /** One registered test, and what happened when it ran. */
    class Case(val name: String, val body: () -> Unit)

    private val cases = mutableListOf<Case>()

    /** The six canonical names, in the order `REQUIRED_TESTS` states them. */
    val REQUIRED_TEST_NAMES: List<String> = listOf(
        "kotlin_conformance_all_required_fixtures",
        "kotlin_retry_reuses_idempotency_key",
        "kotlin_error_reason_mapping",
        "kotlin_stream_resume_with_cursor",
        "kotlin_token_source_refresh",
        "kotlin_pagination_iterator",
    )

    /** Registers a test under `name`. The name is what `run-test.sh` filters on. */
    fun test(name: String, body: () -> Unit) {
        cases.add(Case(name, body))
    }

    /** Every registered test name, sorted. */
    fun names(): List<String> = cases.map { it.name }.sorted()

    /**
     * Runs the tests whose name contains `filters`, or all of them when `filters`
     * is empty.
     *
     * Returns the process exit code, so `RunTests` is a two-line `main` and the
     * policy — which is the part with an opinion — is here to be tested.
     */
    fun run(filters: List<String>): Int {
        val selected = if (filters.isEmpty()) {
            cases
        } else {
            cases.filter { case -> filters.any { case.name.contains(it) } }
        }

        // The anti-vacuity rule, stated once and applied here rather than trusted
        // to: a run that selected nothing has proved nothing, whatever its exit
        // code would otherwise be.
        if (selected.isEmpty()) {
            System.err.println("harness: no test matched ${filters.joinToString(", ")}")
            System.err.println("harness: the ${cases.size} registered tests are:")
            for (name in names()) {
                System.err.println("  $name")
            }
            return 2
        }

        var failed = 0
        val started = System.nanoTime()
        for (case in selected) {
            val problems = mutableListOf<String>()
            val caseStarted = System.nanoTime()
            try {
                case.body()
            } catch (thrown: Throwable) {
                problems.add(thrown.toString())
                thrown.stackTrace.take(6).forEach { frame ->
                    problems.add("      at $frame")
                }
            }
            val millis = (System.nanoTime() - caseStarted) / 1_000_000
            if (problems.isEmpty()) {
                println("  ok    ${case.name}  (${millis}ms)")
            } else {
                failed++
                println("  FAIL  ${case.name}  (${millis}ms)")
                problems.forEach { problem -> println("        $problem") }
            }
        }
        val totalMillis = (System.nanoTime() - started) / 1_000_000
        println("")
        println("${selected.size} test(s), $failed failed, ${totalMillis}ms")
        println("")
        return if (failed == 0) 0 else 1
    }
}

/** Thrown when an assertion fails. Carries the message and nothing else. */
class AssertionFailure(message: String) : AssertionError(message)

/** Asserts `condition`, naming what was expected and what was true. */
fun assertTrue(condition: Boolean, what: String) {
    if (!condition) {
        throw AssertionFailure("expected $what")
    }
}

/** Asserts two values are equal, printing both when they are not. */
fun <T> assertEquals(want: T, got: T, what: String) {
    if (want != got) {
        throw AssertionFailure("$what: expected <$want>, got <$got>")
    }
}

/** Asserts two byte sequences are equal, printing both as hex when they are not. */
fun assertBytes(want: ByteArray, got: ByteArray, what: String) {
    if (!want.contentEquals(got)) {
        throw AssertionFailure("$what: expected <${want.toHex()}, got <${got.toHex()}>")
    }
}

private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

/** Fails unless [body] throws [T], and returns the exception for further checks. */
inline fun <reified T : Throwable> assertThrows(what: String, body: () -> Unit): T {
    try {
        body()
    } catch (thrown: Throwable) {
        if (thrown is T) {
            return thrown
        }
        throw AssertionFailure("$what: expected ${T::class.java.simpleName}, got $thrown")
    }
    throw AssertionFailure("$what: expected ${T::class.java.simpleName}, and nothing was thrown")
}

/** The suite's entry point. */
object RunTests {
    @JvmStatic
    fun main(args: Array<String>) {
        // Touch the registration objects before running: a `val` initialiser in a
        // file is only run when the file's class is loaded, and a suite whose
        // registration depends on a class nobody referenced would report zero
        // tests and exit 2 — correct, but for a reason that hides the bug.
        ConformanceTest.register()
        RetryIdempotencyTest.register()
        ErrorReasonMappingTest.register()
        StreamResumeTest.register()
        TokenSourceTest.register()
        PaginationTest.register()
        CodecTest.register()
        Harness.test("kotlin_the_driver_derives_its_coverage_from_the_manifest") { theDriverDerivesItsCoverage() }
        Harness.test("kotlin_a_fixture_the_driver_cannot_reach_is_named") { anUnreachableFixtureIsNamed() }
        Harness.test("kotlin_the_report_is_written_from_what_the_driver_executed") { theReportIsWrittenFromWhatTheDriverExecuted() }
        Harness.test("kotlin_a_missing_module_names_the_ones_that_exist") { aMissingModuleNamesTheOnesThatExist() }
        Harness.test("kotlin_the_fixture_server_serves_the_manifest") { theFixtureServerServesTheManifest() }

        println("loams-kotlin: running the conformance suite")
        println("")
        exitProcess(Harness.run(args.toList()))
    }
}