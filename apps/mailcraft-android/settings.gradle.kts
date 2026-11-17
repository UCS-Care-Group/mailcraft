// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
pluginManagement {
    repositories {
        google {
            // Google's repository is for Android's own tooling and libraries; Play Services and
            // Firebase are never fetched from it.
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google\\.android\\.(?!gms).*")
                includeGroupByRegex("com\\.google\\.testing\\.platform.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google {
            // Google's repository is for Android's own tooling and libraries; Play Services and
            // Firebase are never fetched from it.
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google\\.android\\.(?!gms).*")
                includeGroupByRegex("com\\.google\\.testing\\.platform.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        // rustls-platform-verifier's Kotlin component, which the project publishes as a
        // Maven repository on GitHub; the version is the one Cargo.lock names, and the file's
        // hash is pinned in gradle/verification-metadata.xml.
        maven("https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/") {
            content { includeGroup("org.rustls") }
        }
    }
}

rootProject.name = "mailcraft-android"
include(":app")
