// The Loams cases' app for DBOS Transact Java (DX0 Task 5). Built with the
// Gradle wrapper of the pinned dbos-transact-java tag (dbos-java/run.sh).
plugins { application }

repositories { mavenCentral() }

dependencies {
  implementation("dev.dbos:transact:" + (System.getenv("DBOS_JAVA") ?: "1.2.0"))
  implementation("org.postgresql:postgresql:42.7.4")
  runtimeOnly("org.slf4j:slf4j-simple:2.0.16")
}

java { toolchain { languageVersion.set(JavaLanguageVersion.of(21)) } }

application { mainClass.set("loams.App") }
