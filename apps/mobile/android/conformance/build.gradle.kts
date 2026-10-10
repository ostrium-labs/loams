import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// Runs against a live mock (../mock): start it, then
//   LOAMS_MOCK_URL=http://127.0.0.1:8084 ./gradlew :conformance:test
// Without LOAMS_MOCK_URL every test is skipped, so `./gradlew test` passes offline.
plugins {
    alias(libs.plugins.kotlin.jvm)
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

dependencies {
    testImplementation(project(":transport"))
    testImplementation(libs.kotlinx.serialization.json)
    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}

tasks.test {
    // Never cache: the result depends on a running server.
    outputs.upToDateWhen { false }
    environment("LOAMS_MOCK_URL", System.getenv("LOAMS_MOCK_URL") ?: "")
}
