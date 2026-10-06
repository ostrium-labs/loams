import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "dev.loams.app"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.loams.app"
        minSdk = 29 // Android 10 (Q431)
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // AppAuth's redirect receiver: dev.loams.app:/oauth2redirect. The verified app link
        // https://loams.dev/app/auth/callback replaces it once assetlinks.json is served (Q281).
        manifestPlaceholders["appAuthRedirectScheme"] = "dev.loams.app"
    }

    buildTypes {
        debug {
            // Debug builds may talk plain http to the local mock on loopback only
            // (src/debug/res/xml/network_security_config.xml).
            buildConfigField("boolean", "ALLOW_INSECURE_LOOPBACK", "true")
            buildConfigField("String", "DEFAULT_SERVER", "\"http://10.0.2.2:8084\"")
        }
        release {
            buildConfigField("boolean", "ALLOW_INSECURE_LOOPBACK", "false")
            buildConfigField("String", "DEFAULT_SERVER", "\"\"")
            // TODO(Q420): R8 rules for protobuf-javalite, Tink and AppAuth, and release signing
            // with the Play upload key; unsigned until the store accounts exist.
            isMinifyEnabled = false
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests {
            isIncludeAndroidResources = true
            all { it.systemProperty("loams.repoRoot", rootProject.projectDir.parentFile.absolutePath) }
        }
    }

    lint {
        abortOnError = true
        checkDependencies = true
        // Version nags are not build failures; Dependabot-style bumps are separate PRs.
        disable += setOf("GradleDependency", "NewerVersionAvailable", "AndroidGradlePluginVersion", "OldTargetApi")
    }

    packaging {
        resources.excludes += setOf("META-INF/versions/9/OSGI-INF/MANIFEST.MF", "META-INF/INDEX.LIST")
    }
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

dependencies {
    implementation(project(":push"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.datastore.preferences)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.appauth)
    implementation(libs.zxing.embedded)
    debugImplementation(libs.compose.ui.tooling)
    debugImplementation(libs.compose.ui.test.manifest)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.turbine)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.core)
    testImplementation(libs.androidx.test.ext.junit)
    testImplementation(platform(libs.compose.bom))
    testImplementation(libs.compose.ui.test.junit4)
}
