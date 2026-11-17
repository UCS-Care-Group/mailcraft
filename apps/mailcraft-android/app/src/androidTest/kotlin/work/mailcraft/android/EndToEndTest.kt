// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.Manifest
import android.app.Activity
import android.app.Instrumentation
import android.app.Notification
import android.app.NotificationManager
import android.content.Intent
import android.net.Uri
import androidx.compose.ui.test.ComposeTimeoutException
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.ComposeTestRule
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.printToString
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.time.LocalDateTime
import java.time.temporal.ChronoUnit
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.standInSentTo

/**
 * The whole app on an emulator against the Graph stand-in (MB3.15): sign in, Send/Receive, read a
 * message, answer it and see the stand-in take the answer, and a calendar reminder's notification.
 *
 * Nothing reaches Microsoft or any real account. The Rust library of a `-Pmailcraft.e2e=true` build
 * sends the sign-in's token endpoint and Graph to the stand-in on the device's loopback address
 * (`adb reverse tcp:8780 tcp:8780` joins it to the computer's, where
 * `cargo run -p mailcraft-net --features stand-in --example standin -- --port 8780 --e2e` runs),
 * and names the sign-in page at `https://stand-in.invalid`, which this test plays: the browser is
 * never opened, and the address the page would send the app back to is handed to the app as the
 * system would. Without that build the test is skipped. See docs/android.md › End to end.
 */
@RunWith(AndroidJUnit4::class)
class EndToEndTest {
    @get:Rule
    val compose: ComposeTestRule = createEmptyComposeRule()

    private val instrumentation: Instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    @Test
    fun signInSyncReadReplyAndReminder() {
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue("needs a -Pmailcraft.e2e=true build and the stand-in", arguments.getString("e2e") == "true")
        // (Given before the window opens, so that no system question stands in front of it.)
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        val page = SignInPageStandIn()
        instrumentation.addMonitor(page)
        try {
            // (Not closed: the redirect hands the window a new intent, after which the scenario no
            // longer knows it; the runner finishes the app's windows when the test ends.)
            val scenario = ActivityScenario.launch(MainActivity::class.java)
            signIn(page, scenario)
            read()
            reply()
            reminder()
        } finally {
            instrumentation.removeMonitor(page)
        }
    }

    /** Add Account, the page (played here), and the app back with the account. */
    private fun signIn(page: SignInPageStandIn, scenario: ActivityScenario<MainActivity>) {
        waitFor(hasText("Add account"))
        compose.onAllNodes(hasText("Add account")).onFirst().performClick()
        waitFor(hasText("Sign in with Microsoft"))
        compose.onAllNodes(hasText("Sign in with Microsoft")).onFirst().performClick()
        val asked = waitForValue(SIGN_IN_WAIT) { page.asked.get() }
        assertEquals("the page goes to the stand-in's name, never Microsoft's", STAND_IN_PAGE_HOST, asked.host)
        val state = asked.getQueryParameter("state")
        val returnTo = asked.getQueryParameter("redirect_uri")
        assertNotNull("the page is told where to come back to", returnTo)
        assertNotNull("the page carries a state", state)
        assertNotNull("the sign-in uses PKCE", asked.getQueryParameter("code_challenge"))
        // What the page does once the owner has signed in: back to the app's own address.
        val redirect = Uri.parse(returnTo).buildUpon()
            .appendQueryParameter("code", "stand-in-code")
            .appendQueryParameter("state", state)
            .build()
        // (From the app's window, as the browser's tab over it would: the app comes back in its own task.)
        val back = Intent(Intent.ACTION_VIEW, redirect).setPackage(context.packageName)
        scenario.onActivity { it.startActivity(back) }
    }

    /** Send/Receive brings the stand-in's Inbox; the message opens with its body. */
    private fun read() {
        // (The sign-in starts a Send/Receive; another is asked for now and then while the mail isn't there.)
        var asked = 0L
        var said = ""
        try {
            compose.waitUntil(SYNC_WAIT) {
                if (System.currentTimeMillis() - asked > SYNC_EVERY) {
                    asked = System.currentTimeMillis()
                    said = runBlocking {
                        runCatching { core().syncNow().message + " / " + core().syncStatus().messages.joinToString() }
                            .getOrElse { it.toString() }
                    }
                }
                compose.onAllNodes(hasText(SUBJECT)).fetchSemanticsNodes().isNotEmpty()
            }
        } catch (timeout: ComposeTimeoutException) {
            throw AssertionError("the stand-in's mail never came; Send/Receive said: $said", timeout)
        }
        compose.onAllNodes(hasText(SUBJECT)).onFirst().performClick()
        waitFor(hasText(BODY, substring = true))
    }

    /** An answer, written and sent; the stand-in has it in Sent Items. */
    private fun reply() {
        compose.onAllNodes(hasText("Reply")).onFirst().performClick()
        waitFor(hasContentDescription("Message"))
        compose.onAllNodes(hasContentDescription("Message")).onFirst().performTextInput(REPLY)
        compose.onAllNodes(hasText("Send")).onFirst().performClick()
        // (The Outbox goes with the next Send/Receive: one is asked for now and then while the
        // answer isn't there.)
        var asked = 0L
        val sent = waitForValue(SYNC_WAIT) {
            sentToStandIn().also {
                if (it == null && System.currentTimeMillis() - asked > SYNC_EVERY) {
                    asked = System.currentTimeMillis()
                    runBlocking { runCatching { core().syncNow() } }
                }
            }
        }
        assertTrue("the answer is to the sender: $sent", sent.contains(SENDER))
    }

    /** A meeting a minute or two away, with a reminder at its start: the system raises it. */
    private fun reminder() {
        val start = LocalDateTime.now().truncatedTo(ChronoUnit.MINUTES).plusMinutes(2)
        val draft = EventDraft(
            title = EVENT,
            startDate = start.toLocalDate(),
            startTime = start.toLocalTime(),
            endDate = start.plusMinutes(30).toLocalDate(),
            endTime = start.plusMinutes(30).toLocalTime(),
            reminder = 0,
        )
        runBlocking { core().execute("calendar.new", draft.params(null)) }
        val manager = context.getSystemService(NotificationManager::class.java)
        waitForValue(REMINDER_WAIT) {
            manager.activeNotifications.firstOrNull {
                it.notification.extras.getCharSequence(Notification.EXTRA_TITLE)?.contains(EVENT) == true
            }
        }
    }

    /** The app's one core, once it is open. */
    private fun core(): Core {
        val application = context.applicationContext as MailCraftApplication
        return checkNotNull((application.started.value as? Phase.Ready)?.value?.core) { "the core isn't open" }
    }

    /**
     * The recipients of the answer in the stand-in's Sent Items, or null while it isn't there. Rust
     * asks the stand-in and reads its answer (`stand_in_sent_to`, crates/mobile/src/e2e.rs).
     */
    private fun sentToStandIn(): List<String>? =
        runCatching { standInSentTo(SUBJECT) }.getOrNull()?.takeIf { it.isNotEmpty() }

    /** Waits for a node that [matcher] finds; else fails with what the window shows (its text, nothing else). */
    private fun waitFor(matcher: SemanticsMatcher, millis: Long = UI_WAIT) {
        try {
            compose.waitUntil(millis) { compose.onAllNodes(matcher).fetchSemanticsNodes().isNotEmpty() }
        } catch (timeout: ComposeTimeoutException) {
            val shown = runCatching { compose.onAllNodes(isRoot()).onFirst().printToString(maxDepth = Int.MAX_VALUE) }
            val said = "${matcher.description} not shown after ${millis / 1_000} s; the window:\n${shown.getOrNull()}"
            throw AssertionError(said, timeout)
        }
    }

    private fun <T : Any> waitForValue(millis: Long, value: () -> T?): T {
        val until = System.currentTimeMillis() + millis
        while (System.currentTimeMillis() < until) {
            value()?.let { return it }
            Thread.sleep(POLL)
        }
        return checkNotNull(value()) { "not there after ${millis / 1_000} s" }
    }

    /**
     * The browser, as far as the sign-in goes: the app's start of the page at the stand-in's name is
     * taken here and never reaches a browser; every other start goes on as it would.
     */
    private class SignInPageStandIn : Instrumentation.ActivityMonitor() {
        val asked = AtomicReference<Uri?>(null)

        override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? {
            val data = intent.data ?: return null
            if (data.host != STAND_IN_PAGE_HOST) return null
            asked.set(data)
            return Instrumentation.ActivityResult(Activity.RESULT_CANCELED, null)
        }
    }

    private companion object {
        /** What `standin --e2e` holds and takes (crates/net/examples/standin.rs, graph/standin.rs). */
        const val STAND_IN_PAGE_HOST = "stand-in.invalid"
        const val SUBJECT = "Quarterly figures for review"
        const val BODY = "The quarterly figures are ready for your review"
        const val SENDER = "ada@example.org"
        const val REPLY = "Thank you, the figures look right."
        const val EVENT = "End-to-end reminder check"

        const val POLL = 500L
        const val UI_WAIT = 30_000L
        const val SIGN_IN_WAIT = 30_000L
        const val SYNC_WAIT = 120_000L
        const val SYNC_EVERY = 10_000L
        const val REMINDER_WAIT = 240_000L
    }
}
