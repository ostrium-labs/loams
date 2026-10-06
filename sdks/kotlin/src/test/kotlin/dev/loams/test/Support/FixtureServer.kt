package dev.loams.test.Support

import dev.loams.JsonValue

/**
 * The fixture server: `sdks/conformance/fixture-server.mjs`, started as a child
 * process, and the endpoint it prints.
 *
 * ## Why the SDK starts Node rather than reimplementing it
 *
 * The harness's job in a conformance run is to be **boring**. If this SDK
 * replayed the corpus in-process it would be testing its own reader against its
 * own driver, and the thing `fixture-server.mjs` exists to catch — a request
 * whose bytes are not the recorded ones — could not be caught at all. Starting
 * the real server is what makes the byte comparison mean anything, and
 * `check-languages.mjs` and `run-test.sh` compare *this* endpoint's answers with
 * what every other language's answers were.
 *
 * ## `live` is false by construction
 *
 * `CorpusRun.live` records whether the endpoint was a real `loams dev`. This SDK
 * never claims it: the only endpoint it starts is the fixture server. A report
 * that said `live` for a replay would be claiming a conformance claim nobody
 * checked.
 */
class FixtureServer private constructor(val endpoint: String, private val process: Process) : AutoCloseable {
    /** Always false: the fixture server is a replay, never a real `loams dev`. */
    val live: Boolean get() = false

    override fun close() {
        process.destroy()
        // `waitFor` bounded, so a hung child cannot hold the suite open past the
        // JVM's exit and leave a listening socket behind for the next run.
        if (!process.waitFor(5, java.util.concurrent.TimeUnit.SECONDS)) {
            process.destroyForcibly()
        }
    }

    companion object {
        /**
         * Starts a fixture server over [corpusDir] and waits for the URL it prints.
         *
         * A server that does not come up within [timeoutMillis] is a **failure
         * with the server's own output attached**, not a silent skip: a conformance
         * run that cannot reach its corpus must not report coverage.
         */
        fun start(
            corpusDir: java.io.File,
            timeoutMillis: Long = 20_000,
        ): FixtureServer {
            val repositoryRoot = RepositoryRoot.find(corpusDir)
            val script = java.io.File(repositoryRoot, "sdks/conformance/fixture-server.mjs")
            require(script.isFile) { "there is no ${script.path}; this is not a checkout of the repository" }

            val builder = ProcessBuilder("node", script.path, "--port", "0", "--fixtures", corpusDir.path)
                .directory(repositoryRoot)
                .redirectErrorStream(true)
            val process = builder.start()
            val output = StringBuilder()
            val reader = process.inputStream.bufferedReader()
            val endpoint = System.nanoTime() + timeoutMillis * 1_000_000
            var url: String? = null
            while (System.nanoTime() < endpoint) {
                val line = reader.readLine() ?: break
                output.append(line).append('\n')
                val marker = "\"url\":\""
                val at = line.indexOf(marker)
                if (at >= 0) {
                    val end = line.indexOf('"', at + marker.length)
                    if (end > at) {
                        url = line.substring(at + marker.length, end)
                        break
                    }
                }
            }
            if (url == null) {
                process.destroyForcibly()
                throw IllegalStateException(
                    "the fixture server did not print a url within ${timeoutMillis}ms. Its output was:\n$output"
                )
            }
            // Drain the rest so the child never blocks on a full pipe, and keep it
            // for a failure message.
            Thread {
                try {
                    while (reader.readLine() != null) { /* drained */ }
                } catch (_: Exception) {
                    // the child is going away; nothing to report
                }
            }.apply { isDaemon = true }.start()
            return FixtureServer(url, process)
        }
    }
}

/** Finds the repository root by the one thing that exists in exactly one place. */
object RepositoryRoot {
    /**
     * Walks up from [start] until `sdks/fixtures/manifest.json` appears.
     *
     * Walking up rather than counting `../../..`: the suite runs from
     * `sdks/kotlin` in CI and from anywhere else by hand, and the manifest is the
     * thing that identifies the root.
     */
    fun find(start: java.io.File): java.io.File {
        var directory: java.io.File? = start.absoluteFile
        while (directory != null) {
            if (java.io.File(directory, "sdks/fixtures/manifest.json").isFile) {
                return directory
            }
            directory = directory.parentFile
        }
        directory = java.io.File(".").absoluteFile
        while (directory != null) {
            if (java.io.File(directory, "sdks/fixtures/manifest.json").isFile) {
                return directory
            }
            directory = directory.parentFile
        }
        throw IllegalStateException(
            "no sdks/fixtures/manifest.json above ${start.absolutePath}, so this is not a checkout of the " +
                "repository and the conformance corpus cannot be found"
        )
    }
}

/** Reads a file as text, or returns null when it does not exist. */
fun readTextOrNull(file: java.io.File): String? = if (file.isFile) file.readText() else null

/** A JSON value's members, or an empty list when it has none. */
fun JsonValue.memberNames(): List<String> = asObject()?.entries?.keys?.toList() ?: emptyList()