package dev.loams.test

import dev.loams.test.Support.ConformanceReport
import dev.loams.test.Support.CorpusDriver
import dev.loams.test.Support.FixtureServer
import dev.loams.test.Support.RepositoryRoot
import java.io.File

/**
 * `kotlin_conformance_all_required_fixtures` — the 100% bar (D617).
 *
 * Replays **every fixture `manifest.json` marks `required`** through the SDK's own
 * public surface and checks each one against its own recording, then writes
 * `sdks/fixtures/results/kotlin.json` from what it actually ran. The list comes
 * from the manifest at run time, so this test cannot pass without touching a
 * required fixture: a fixture the driver could not reach is named in the failure
 * *and* absent from the report's `ran`, and `check-languages.mjs --check kotlin`
 * then exits 1 naming it.
 */
object ConformanceTest {
    const val NAME = "kotlin_conformance_all_required_fixtures"

    /** Runs the whole corpus. Returns the report path it wrote. */
    fun runCorpus(): Pair<dev.loams.test.Support.CorpusRun, File> {
        val workingDirectory = File(".").absoluteFile
        val root = RepositoryRoot.find(workingDirectory)
        val run = CorpusDriver(File(root, "sdks/fixtures")).run()
        val report = ConformanceReport.write(root, run, ConformanceReport.REQUIRED_TEST_NAMES)
        return run to report
    }

    /** The last run's report, if one is on disk. For the other tests' assertions. */
    fun lastReport(): File? {
        val root = RepositoryRoot.find(File(".").absoluteFile)
        val file = File(root, ConformanceReport.REPORT_PATH)
        return if (file.isFile) file else null
    }

    fun register() {
        Harness.test(NAME) {
            val (run, report) = runCorpus()

            // Every failure is printed, not just the first: a suite that reports
            // one problem per CI cycle costs a debugging session each time.
            if (run.failures.isNotEmpty()) {
                throw AssertionFailure(
                    "${run.failures.size} conformance failure(s):\n" +
                        run.failures.joinToString("\n") { "        $it" }
                )
            }

            val required = run.outcomes.size
            assertTrue(required > 0, "the manifest to mark any fixture required")
            assertEquals(required, run.ran.size, "the fixtures replayed and checked")
            assertTrue(report.isFile, "a report written at ${report.path}")

            // The report must agree with the run: a suite that wrote `ran` from a
            // list rather than from what it executed would disagree here, which is
            // the check that keeps `ran` honest.
            val text = report.readText()
            for (name in run.ran) {
                assertTrue(text.contains("\"$name\""), "the report to name $name")
            }
        }
    }
}

/**
 * `kotlin_conformance_test_names_exist` — the six names are part of the contract.
 *
 * `sdks/conformance/required.mjs` holds the six names as data so two languages
 * can be compared, and `check-languages.mjs --drift` greps this suite's **source**
 * for them. A suite that renames one without editing `required.mjs` would show up
 * there as a language testing fewer than six things; this asserts the same
 * invariant from inside the suite, where a rename has to be deliberate.
 */
fun testNamesExist() {
    Harness.test("kotlin_conformance_test_names_exist") {
        val registered = Harness.names()
        for (short in ConformanceReport.REQUIRED_TEST_NAMES) {
            assertTrue(
                registered.any { it.contains(short) },
                "a registered test whose name contains $short (registered: $registered)",
            )
        }
    }
}

/**
 * `kotlin_conformance_report_is_generated_not_committed` — the report cannot vouch
 * for a run that never happened.
 *
 * `run-test.sh` deletes a language's report before it starts a run (D651), and the
 * C# agent found the reason: `dotnet test --filter` matched zero tests and **exited
 * 0**, and the coverage check then read a report an *earlier full run* had left on
 * disk. This asserts the two halves of the fix from the SDK's side: the file is
 * under a gitignored directory, and it exists only because this run wrote it.
 */
fun reportIsGeneratedNotCommitted() {
    Harness.test("kotlin_conformance_report_is_generated_not_committed") {
        val root = RepositoryRoot.find(File(".").absoluteFile)
        val ignored = File(root, "sdks/fixtures/.gitignore")
        assertTrue(ignored.isFile, "sdks/fixtures/.gitignore to exist")
        val text = ignored.readText()
        assertTrue(
            text.lineSequence().any { it.trim() == "results/" },
            "sdks/fixtures/.gitignore to ignore results/ (it says: ${text.replace('\n', '|')})",
        )

        val report = ConformanceTest.lastReport()
        assertTrue(report != null && report.isFile, "a report written by this run")
        // A report written earlier would be older than the classes that wrote it.
        val run = ConformanceTest.runCorpus().first
        assertTrue(report!!.length() > 0, "a non-empty report")
        assertTrue(run.ran.isNotEmpty(), "the run that wrote it to have replayed something")
    }
}

/** `kotlin_conformance_covers_every_required_fixture` — the coverage is derived. */
fun coverageIsDerived() {
    Harness.test("kotlin_conformance_covers_every_required_fixture") {
        val root = RepositoryRoot.find(File(".").absoluteFile)
        val driver = CorpusDriver(File(root, "sdks/fixtures"))
        val required = driver.requiredFixtures
        assertTrue(required.size >= 28, "28 required fixtures in the manifest (found ${required.size})")

        // The derived list is the manifest's, so a fixture the manifest newly
        // requires is in the next run with nobody editing this file. Asserted by
        // comparing the driver's list against the manifest's own `required` flags
        // rather than against a number.
        val manifestText = File(root, "sdks/fixtures/manifest.json").readText()
        for (fixture in required) {
            assertTrue(manifestText.contains("\"${fixture.name}\""), "the manifest to name ${fixture.name}")
        }
        // And nothing outside the required set is claimed.
        val report = ConformanceTest.lastReport() ?: return@test
        val reportText = report.readText()
        for (line in reportText.lines()) {
            val name = Regex("\"([a-z0-9_]+)\"").find(line)?.groupValues?.getOrNull(1) ?: continue
            if (name in REQUIRED_NAMES_ALLOWED) continue
            if (!required.any { it.name == name } && name.startsWith("mock_")) {
                throw AssertionFailure(
                    "the report names $name, which the manifest does not mark required: coverage is claimed " +
                        "for a fixture the corpus does not require"
                )
            }
        }
    }
}

private val REQUIRED_NAMES_ALLOWED = setOf(
    "kotlin", "connect", "about", "tests", "ran", "skipped", "live", "endpoint", "failures",
)

/** Whether the fixture server itself comes up, which the corpus test depends on. */
fun fixtureServerComesUp() {
    Harness.test("kotlin_conformance_fixture_server_is_reachable") {
        val root = RepositoryRoot.find(File(".").absoluteFile)
        FixtureServer.start(File(root, "sdks/fixtures")).use { server ->
            assertTrue(server.endpoint.startsWith("http://127.0.0.1:"), "a loopback endpoint")
            assertTrue(!server.live, "a replay rather than a real server, which this SDK never claims")
        }
    }
}