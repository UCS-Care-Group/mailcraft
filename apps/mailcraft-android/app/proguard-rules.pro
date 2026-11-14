# SPDX-FileCopyrightText: MailCraft contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# R8 / ProGuard rules for the release build.

# JNA is how UniFFI's Kotlin bindings reach the Rust library: it finds its pieces by reflection
# and by name from native code.
-dontwarn java.awt.*
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }

# The generated bindings: structures and callback interfaces that the native side names.
-keep class uniffi.** { *; }

# rustls-platform-verifier's Kotlin component, which the Rust side calls into by name over JNI.
-keep class org.rustls.platformverifier.** { *; }

# Our own JNI entry.
-keep class work.mailcraft.android.Bridge { native <methods>; }
-keepclasseswithmembernames class * { native <methods>; }
