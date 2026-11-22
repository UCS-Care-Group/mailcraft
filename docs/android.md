# Building and testing the Android app

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** minor

The Android app (`apps/mailcraft-android`) is a Gradle project over the Rust library of
`crates/mobile`, which `cargo-ndk` builds; the Kotlin bindings are generated from that library at
build time ([architecture.md](architecture.md)). It needs the Android SDK (`ANDROID_HOME`) with
`platforms;android-36`, `build-tools;36.0.0` and `ndk;28.2.13676358`, the Rust targets
`aarch64-linux-android` and `x86_64-linux-android`, `cargo install cargo-ndk --version 4.1.2`, and
Java 21 for the screenshot tests (the app targets 17).

## Every change

`cargo xtask mobile` is what CI's `android-app` job runs: the debug and release builds, Android Lint
and detekt as errors, the unit tests, the screenshot tests (`verifyPaparazziDebug`; after a change
to a screen, `./gradlew recordPaparazziDebug` draws them again), the licences of what ships and
the check that no Google Play Services or Firebase is in it. From `apps/mailcraft-android`, the
Gradle tasks run one by one too: `./gradlew testDebugUnitTest detekt lintDebug verifyPaparazziDebug`.

## End to end

One instrumented test (`app/src/androidTest/.../EndToEndTest.kt`) runs the whole app on an
emulator against the Graph stand-in: it adds an account and signs in, asks for Send/Receive until it
brings the Inbox, opens a message and finds its body, writes an answer and sends it, checks that the
stand-in has it in Sent Items, and makes a meeting two minutes away whose reminder the system then
raises as a notification. It takes a few minutes once the app is built. CI runs it every week, by
hand (Actions › Android end to end) and from the release workflow, never on a pull request
(`.github/workflows/android-e2e.yml`).

Nothing reaches Microsoft or any real account. The test needs a build made with
`-Pmailcraft.e2e=true`, which builds the Rust library with its `e2e` feature (for the emulator's
x86_64 alone): that library sends the sign-in's token endpoint and every call to Microsoft Graph to
the stand-in on the device's own loopback address, `127.0.0.1:8780`, and names the sign-in page at
`https://stand-in.invalid`, a name that is never found. The test plays that page: it takes the
app's start of it before any browser sees it and hands the app the address the page would send it
back to, as the system would. A build with that property refuses to make a release app, and
without it the test is skipped and nothing of the seam is built.

To run it on your own computer, on an emulator (not a phone of yours: the test adds an account and
leaves notifications):

```bash
# Once: an x86_64 emulator image and a device for it.
sdkmanager "emulator" "system-images;android-35;aosp_atd;x86_64"
echo no | avdmanager create avd -n mc-e2e -k "system-images;android-35;aosp_atd;x86_64" -d pixel_6

# The emulator, and the stand-in with the run's data (in two other terminals).
emulator -avd mc-e2e -no-snapshot -no-audio
cargo run -p mailcraft-net --features stand-in --example standin -- --port 8780 --e2e

# The emulator's 127.0.0.1:8780 is this computer's; then the test, on the emulator alone.
adb -s emulator-5554 reverse tcp:8780 tcp:8780
cd apps/mailcraft-android
ANDROID_SERIAL=emulator-5554 ./gradlew -Pmailcraft.e2e=true connectedDebugAndroidTest
```

`ANDROID_SERIAL` keeps Gradle to the emulator when a phone is connected too. Start the stand-in
afresh for each run: it keeps what the last run sent. The app is uninstalled after the run, so the
next starts with no account. A failure leaves its report in
`app/build/reports/androidTests/connected/`.

Until the phone brings calendars down from the server (the shared host's calendar run), the
meeting of the reminder step is made on the phone through `calendar.new`, as the calendar's editor
makes one, and not taken from the stand-in.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-17 | minor | MB3.15: how the app is built and checked, and the end-to-end run on an emulator against the Graph stand-in |
