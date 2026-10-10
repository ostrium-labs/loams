pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "loams-android"

// Pure JVM: testable without the Android SDK.
include(":core")         // models, pairing payload, decision claims, watch resume, unsealing
include(":proto")        // generated protobuf-javalite + connect-kotlin (committed, see docs/protos.md)
include(":transport")    // Connect clients over OkHttp, TLS pinning, the OAuth token endpoint
include(":conformance")  // JVM tests against the mock server in ../mock
// Android.
include(":data")         // Keystore keys, biometric signing, token store, repositories
include(":push")         // push registration (FCM stub) and sealed-notification display
include(":app")          // Compose UI
