// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.Locale
import uniffi.mailcraft_mobile.OpenVerdict

/**
 * One attachment of the open message as the reading pane lists it: what it is called (the name the
 * core made safe), how big it is, and what may be done with it under the engine's opening verdict.
 */
internal data class AttachmentItem(
    val index: UInt,
    val name: String,
    val saveName: String,
    val size: Long,
    val verdict: OpenVerdict,
    /** The owner's words for a kind that is never opened or shared (from the core), empty otherwise. */
    val blocked: String = "",
    /** A calendar file, which can be brought into the calendar. */
    val calendar: Boolean = false,
)

/** What can be done with an attachment. */
internal enum class AttachmentAction { Open, Save, Share, AddToCalendar }

/** The kinds the preview inside the app draws: told here by their name only to choose an icon and a first try. */
private val PICTURE_KINDS = setOf("png", "jpg", "jpeg", "gif", "webp")

/** Whether the name says a picture (the bytes decide whether it is shown as one). */
internal fun looksLikeAPictureByName(name: String): Boolean =
    name.substringAfterLast('.', "").lowercase(Locale.ROOT) in PICTURE_KINDS

/**
 * The actions an attachment offers, first the one a tap does. A blocked kind (a program, a script, a
 * package, a file of no known kind, and every installer) can only be saved: never opened or shared. A calendar
 * file goes to the calendar first.
 */
internal fun actionsOf(item: AttachmentItem): List<AttachmentAction> = buildList {
    val blocked = item.verdict == OpenVerdict.BLOCKED || isInstallable(item.name)
    if (item.calendar && !blocked) add(AttachmentAction.AddToCalendar)
    if (!blocked) add(AttachmentAction.Open)
    add(AttachmentAction.Save)
    if (!blocked) add(AttachmentAction.Share)
}

/** What a tap on the row does. */
internal fun primaryOf(item: AttachmentItem): AttachmentAction = actionsOf(item).first()

/** Whether the owner is asked before an attachment of this verdict is opened (macros, scripts). */
internal fun asksFirst(verdict: OpenVerdict): Boolean = verdict == OpenVerdict.MACROS || verdict == OpenVerdict.SCRIPTS

/** The media type to give another app for a file called [name]: from its kind, else the generic one. */
internal fun mimeFor(name: String, byKind: (String) -> String?): String =
    byKind(name.substringAfterLast('.', "").lowercase(Locale.ROOT)) ?: "application/octet-stream"

/** What the attachments' views ask of the app. */
internal class AttachmentActions(
    val onAct: (ULong, AttachmentItem, AttachmentAction) -> Unit,
    val onConfirm: () -> Unit,
    val onDismiss: () -> Unit,
    /** The document the owner chose to save to (its address), or null when they backed out. */
    val onSaveTo: (String?) -> Unit,
    val onPreviewAct: (AttachmentAction) -> Unit,
    val onNoticeShown: () -> Unit,
) {
    companion object {
        val None = AttachmentActions({ _, _, _ -> }, {}, {}, {}, {}, {})
    }
}

/** What the attachments' views ask, answered by this controller. */
internal fun AttachmentController.actions() = AttachmentActions(
    onAct = ::act,
    onConfirm = ::confirm,
    onDismiss = ::dismiss,
    onSaveTo = ::saveTo,
    onPreviewAct = ::previewAct,
    onNoticeShown = ::noticeShown,
)
