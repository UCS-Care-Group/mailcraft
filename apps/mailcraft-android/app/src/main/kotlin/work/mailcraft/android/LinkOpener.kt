// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.core.net.toUri

/** What following a link came to. */
internal enum class LinkResult {
    Opened,

    /** No app on the phone takes it. */
    NoApp,

    /** It is for the composer, which is not built yet. */
    ComposerPending,

    /** Nothing is followed (a part of the message itself). */
    Ignored,
}

/** Follows the links of a message: only to the browser, the dialer and the composer. */
internal interface LinkOpener {
    fun open(link: LinkInfo): LinkResult
}

/** The system action and address a link is handed to, or null when it is not handed to the system. */
internal class LinkRequest(val action: String, val address: String)

/**
 * Where [link] is handed. A web address goes to the browser and a phone number to the dialer (which
 * only shows the number: nothing is dialled). The address is the one the core checked and wrote, and
 * a scheme other than the five the core follows is never handed on.
 */
internal fun requestFor(link: LinkInfo): LinkRequest? {
    val lower = link.shown.lowercase()
    return when (link.goes) {
        LinkGoes.Browser ->
            if (lower.startsWith("http://") || lower.startsWith("https://")) {
                LinkRequest(Intent.ACTION_VIEW, link.shown)
            } else {
                null
            }
        LinkGoes.Dialer -> if (lower.startsWith("tel:")) LinkRequest(Intent.ACTION_DIAL, link.shown) else null
        LinkGoes.Composer, LinkGoes.Part -> null
    }
}

/** [LinkOpener] over the system's own ways of opening an address, from the window in front. */
internal class AndroidLinks(
    private val context: Context,
    private val foreground: () -> Activity?,
) : LinkOpener {
    override fun open(link: LinkInfo): LinkResult {
        if (link.goes == LinkGoes.Composer) return LinkResult.ComposerPending
        val request = requestFor(link) ?: return LinkResult.Ignored
        val intent = Intent(request.action, request.address.toUri()).addCategory(Intent.CATEGORY_BROWSABLE)
        return try {
            val activity = foreground()
            if (activity != null) {
                activity.startActivity(intent)
            } else {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            LinkResult.Opened
        } catch (_: ActivityNotFoundException) {
            LinkResult.NoApp
        } catch (_: SecurityException) {
            LinkResult.NoApp
        }
    }
}
