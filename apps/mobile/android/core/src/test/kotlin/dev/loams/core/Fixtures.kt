package dev.loams.core

import java.io.File
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject

/** Reads the repository-root files shared with iOS and the mock (conformance/, proto/). */
object Fixtures {
    val repoRoot: File = File(System.getProperty("loams.repoRoot") ?: "../..").canonicalFile

    fun file(path: String): File = File(repoRoot, path)

    fun json(path: String): JsonObject = Json.parseToJsonElement(file("conformance/fixtures/$path").readText()).jsonObject
}
