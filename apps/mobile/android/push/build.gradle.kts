import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// Push registration and display. FCM is a stub until the Play/Firebase accounts exist (Q420):
// no Firebase dependency, no google-services.json. A `unifiedpush` flavor follows (Q439).
plugins {
    alias(libs.plugins.android.library)
}

android {
    namespace = "dev.loams.push"
    compileSdk = 37
    defaultConfig {
        minSdk = 29
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    testOptions {
        unitTests.all { it.systemProperty("loams.repoRoot", rootProject.projectDir.parentFile.absolutePath) }
    }
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

dependencies {
    api(project(":data"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.serialization.json)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.serialization.json)
}
