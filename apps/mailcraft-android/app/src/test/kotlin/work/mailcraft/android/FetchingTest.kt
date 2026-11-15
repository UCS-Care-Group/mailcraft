// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.AttachmentVerdict
import uniffi.mailcraft_mobile.Event
import uniffi.mailcraft_mobile.FetchFailure
import uniffi.mailcraft_mobile.OpenState

class FetchingTest {
    @Test
    fun theCoresAnswersAreReadIntoTheAppsOwnTypes() {
        assertEquals(Opened.Here, Opened.of(OpenState.Here))
        assertEquals(Opened.Downloading, Opened.of(OpenState.Fetching))
        assertEquals(Opened.Offline, Opened.of(OpenState.Offline))
        val unavailable = Opened.of(OpenState.Unavailable("no sign-in"))
        assertTrue(unavailable is Opened.Unavailable && unavailable.reason == "no sign-in")
        for (failure in FetchFailure.entries) {
            assertEquals(failure.name.replace("_", ""), FetchProblem.of(failure).name.uppercase())
        }
    }

    @Test
    fun theEventsOfADownloadAreItsOutcomeAndNoOtherEventIs() {
        val came = FetchOutcome.of(Event.MessageFetched(7u))
        assertTrue(came is FetchOutcome.Came && came.message == 7uL)
        val failed = FetchOutcome.of(Event.FetchFailed(8u, FetchFailure.TOO_LARGE))
        assertTrue(failed is FetchOutcome.Failed && failed.problem == FetchProblem.TooLarge)
        assertNull(FetchOutcome.of(Event.RevChanged(1u)))
    }

    @Test
    fun aProgramSentInMailIsNeverOpened() {
        assertFalse(AttachmentUse.of(AttachmentVerdict.SAVE_ONLY).opens)
        assertTrue(AttachmentUse.of(AttachmentVerdict.OPEN).opens)
        assertEquals(AttachmentUse.AskMacros, AttachmentUse.of(AttachmentVerdict.ASK_MACROS))
        assertEquals(AttachmentUse.AskScripts, AttachmentUse.of(AttachmentVerdict.ASK_SCRIPTS))
    }
}
