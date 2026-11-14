// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.detekt)
}

val repoRoot: File = rootProject.projectDir.resolve("../..").canonicalFile
val rustJniLibs = layout.buildDirectory.dir("rustJniLibs")

android {
    namespace = "work.mailcraft.android"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "work.mailcraft.android"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.12.0"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures { compose = true }

    sourceSets {
        getByName("main") { jniLibs.srcDir(rustJniLibs) }
    }

    packaging {
        // JNA ships its own dispatcher for each ABI; the Rust library is ours.
        jniLibs { useLegacyPackaging = false }
    }

    lint {
        // Lint is part of the build and any finding fails it. Checks that depend on today's
        // date (newer versions of a library) are off so that the same source builds the same
        // way tomorrow; the versions are pinned and hashed on purpose.
        warningsAsErrors = true
        abortOnError = true
        checkAllWarnings = true
        checkDependencies = false
        lintConfig = file("lint.xml")
    }

    testOptions { unitTests.isReturnDefaultValues = false }
}

kotlin {
    compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) }
}

detekt {
    buildUponDefaultConfig = true
    allRules = false
    config.setFrom(rootProject.file("config/detekt/detekt.yml"))
    // Generated bindings are not ours to lint; the app's own sources are.
    source.setFrom("src/main/kotlin", "src/test/kotlin")
    parallel = true
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.browser)
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.kotlinx.coroutines.android)
    implementation("${libs.jna.get()}@aar")
    implementation(libs.platform.verifier)

    testImplementation(libs.junit)
}

// ---- The Rust library and its Kotlin bindings ------------------------------------------------

// The Kotlin half of rustls-platform-verifier has to be the version that Cargo.lock names.
val checkVerifierVersion by tasks.registering {
    val lock = repoRoot.resolve("Cargo.lock")
    val wanted = libs.versions.platformVerifier.get()
    inputs.file(lock)
    inputs.property("wanted", wanted)
    doLast {
        val text = lock.readText()
        val found = Regex("""name = "rustls-platform-verifier-android"\s+version = "([^"]+)"""")
            .find(text)?.groupValues?.get(1)
        check(found == wanted) {
            "gradle/libs.versions.toml pins rustls-platform-verifier $wanted, but Cargo.lock has $found"
        }
    }
}

val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds mailcraft-mobile for Android's arm64-v8a and x86_64 with cargo-ndk."
    dependsOn(checkVerifierVersion)
    workingDir = repoRoot
    val sdk = android.sdkDirectory
    val ndk = sdk.resolve("ndk").resolve(android.ndkVersion)
    doFirst { environment("ANDROID_NDK_HOME", ndk.path) }
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a", "-t", "x86_64",
        "--platform", "29",
        "-o", rustJniLibs.get().asFile.path,
        "build", "--release", "--locked", "-p", "mailcraft-mobile",
    )
    inputs.files(repoRoot.resolve("Cargo.lock"))
    outputs.dir(rustJniLibs)
    // Cargo decides what is out of date.
    outputs.upToDateWhen { false }
}

abstract class UniffiBindgen @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:InputFile
    abstract val library: RegularFileProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val manifest = project.rootProject.projectDir.resolve("uniffi-bindgen/Cargo.toml")
        outputDir.get().asFile.deleteRecursively()
        exec.exec {
            commandLine(
                "cargo", "run", "--locked", "--quiet", "--manifest-path", manifest.path, "--",
                "generate", "--library", library.get().asFile.path,
                "--language", "kotlin", "--no-format",
                "--out-dir", outputDir.get().asFile.path,
            )
        }
    }
}

val uniffiBindgen by tasks.registering(UniffiBindgen::class) {
    group = "rust"
    description = "Generates the Kotlin bindings from the library's own metadata (UniFFI, pinned)."
    dependsOn(cargoNdkBuild)
    library.set(rustJniLibs.map { it.file("arm64-v8a/libmailcraft_mobile.so") })
    outputDir.set(layout.buildDirectory.dir("generated/uniffi/kotlin"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.java?.addGeneratedSourceDirectory(uniffiBindgen, UniffiBindgen::outputDir)
    }
}

tasks.configureEach {
    // The libraries must be in place before anything packages them, and lint reads the
    // generated sources too.
    if (name.startsWith("merge") && name.endsWith("JniLibFolders")) dependsOn(cargoNdkBuild)
}

// ---- The tree that ships: its licences, for `cargo xtask mobile` ------------------------------

// Writes group:name:version<TAB>licence;licence for everything the release build ships, taken
// from each artifact's POM. `cargo xtask mobile` reads it against the allowlist.
val dependencyLicences by tasks.registering {
    group = "verification"
    description = "Lists the licences of the dependencies that ship in the app."
    val out = layout.buildDirectory.file("reports/dependency-licences.tsv")
    outputs.file(out)
    val configuration = configurations.named("releaseRuntimeClasspath")
    // The resolved tree is the input: a changed version (or a new dependency) reruns the task.
    inputs.property(
        "resolvedComponents",
        configuration.map { c ->
            c.incoming.resolutionResult.allComponents.map { it.id.displayName }.sorted().joinToString("\n")
        },
    )
    doLast {
        val ids = configuration.get().incoming.resolutionResult.allComponents
            .mapNotNull { it.id as? org.gradle.api.artifacts.component.ModuleComponentIdentifier }
        val result = dependencies.createArtifactResolutionQuery()
            .forComponents(ids)
            .withArtifacts(
                org.gradle.maven.MavenModule::class.java,
                org.gradle.maven.MavenPomArtifact::class.java,
            )
            .execute()
        val lines = result.resolvedComponents.map { component ->
            val id = component.id as org.gradle.api.artifacts.component.ModuleComponentIdentifier
            val pom = component.getArtifacts(org.gradle.maven.MavenPomArtifact::class.java)
                .filterIsInstance<org.gradle.api.artifacts.result.ResolvedArtifactResult>()
                .firstOrNull()?.file?.readText().orEmpty()
            val licences = Regex("<license>.*?<name>(.*?)</name>", RegexOption.DOT_MATCHES_ALL)
                .findAll(pom).map { it.groupValues[1].trim() }.toList()
            "${id.group}:${id.module}:${id.version}\t${licences.joinToString(";")}"
        }.sorted()
        out.get().asFile.apply { parentFile.mkdirs() }.writeText(lines.joinToString("\n", postfix = "\n"))
    }
}
