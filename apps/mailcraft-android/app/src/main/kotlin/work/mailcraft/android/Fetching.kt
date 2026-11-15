// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.AttachmentFile
import uniffi.mailcraft_mobile.AttachmentVerdict
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.Event
import uniffi.mailcraft_mobile.FetchFailure
import uniffi.mailcraft_mobile.OpenState
import uniffi.mailcraft_mobile.ServerSearchAnswer

/** What opening a message found: the reading pane shows it, waits for it, or says why it can't. */
internal sealed interface OpenResult {
    /** On the phone whole: read it. */
    data object Here : OpenResult

    /** Being downloaded: [FetchOutcome] follows through the event stream. */
    data object Downloading : OpenResult

    /** Only its headers are here, and the phone is offline. */
    data object Offline : OpenResult

    /** It can't be downloaded: [reason] is the core's, in the owner's language. */
    class Unavailable(val reason: String) : OpenResult

    companion object {
        fun of(state: OpenState): OpenResult = when (state) {
            is OpenState.Here -> Here
            is OpenState.Fetching -> Downloading
            is OpenState.Offline -> Offline
            is OpenState.Unavailable -> Unavailable(state.reason)
        }
    }
}

/** Why a message didn't come, for the reading pane to say in its own words. */
internal enum class FetchProblem {
    Offline,
    SignInNeeded,
    Gone,
    TooLarge,
    Refused,
    Busy,
    Cancelled,
    Failed,
    ;

    companion object {
        fun of(failure: FetchFailure): FetchProblem = when (failure) {
            FetchFailure.OFFLINE -> Offline
            FetchFailure.SIGN_IN_NEEDED -> SignInNeeded
            FetchFailure.GONE -> Gone
            FetchFailure.TOO_LARGE -> TooLarge
            FetchFailure.REFUSED -> Refused
            FetchFailure.BUSY -> Busy
            FetchFailure.CANCELLED -> Cancelled
            FetchFailure.FAILED -> Failed
        }
    }
}

/** The end of a download the reading pane waits for, read from the event stream. */
internal sealed interface FetchOutcome {
    val message: ULong

    class Came(override val message: ULong) : FetchOutcome

    class Failed(override val message: ULong, val problem: FetchProblem) : FetchOutcome

    companion object {
        /** The outcome an event says, or null for an event that isn't about a download. */
        fun of(event: Event): FetchOutcome? = when (event) {
            is Event.MessageFetched -> Came(event.message)
            is Event.FetchFailed -> Failed(event.message, FetchProblem.of(event.reason))
            else -> null
        }
    }
}

/** What may be done with an attachment: the core judges it by its kind, as the desktop does. */
internal enum class AttachmentUse {
    Open,
    AskMacros,
    AskScripts,
    SaveOnly,
    ;

    /** Whether it may be handed to another app to open (after asking, for [AskMacros] and [AskScripts]). */
    val opens: Boolean get() = this != SaveOnly

    companion object {
        fun of(verdict: AttachmentVerdict): AttachmentUse = when (verdict) {
            AttachmentVerdict.OPEN -> Open
            AttachmentVerdict.ASK_MACROS -> AskMacros
            AttachmentVerdict.ASK_SCRIPTS -> AskScripts
            AttachmentVerdict.SAVE_ONLY -> SaveOnly
        }
    }
}

/**
 * What the reading pane and search ask of the core to have mail downloaded: one seam, so that they
 * run without the library. A message's text and attachments are the sender's: shown, never run.
 */
internal interface FetchApi {
    suspend fun open(message: ULong): OpenResult

    /** The attachment's file, waiting for its message when it isn't on the phone yet; null when it can't be had. */
    suspend fun attachment(message: ULong, index: UInt): AttachmentFile?

    /** A search on the account's server (null when it couldn't be asked), and its next page. */
    suspend fun searchServer(account: ULong, query: String): ServerSearchAnswer?

    suspend fun searchServerMore(account: ULong): ServerSearchAnswer?
}

/** [FetchApi] over the Rust core. */
internal class CoreFetching(private val core: Core) : FetchApi {
    override suspend fun open(message: ULong): OpenResult =
        try {
            OpenResult.of(core.openMessage(message))
        } catch (e: CoreException) {
            OpenResult.Unavailable(e.message.orEmpty())
        }

    override suspend fun attachment(message: ULong, index: UInt): AttachmentFile? =
        try {
            core.fetchAttachment(message, index)
        } catch (_: CoreException) {
            null
        }

    override suspend fun searchServer(account: ULong, query: String): ServerSearchAnswer? =
        try {
            core.searchServer(account, query)
        } catch (_: CoreException) {
            null
        }

    override suspend fun searchServerMore(account: ULong): ServerSearchAnswer? =
        try {
            core.searchServerMore(account)
        } catch (_: CoreException) {
            null
        }
}
