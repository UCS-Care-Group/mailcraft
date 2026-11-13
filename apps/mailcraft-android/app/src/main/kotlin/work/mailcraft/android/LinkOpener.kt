// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.Activity
import android.content.Context
import android.content.Intent
import androidx.core.net.toUri
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.HostException

/** What following a link came to. */
internal enum class LinkResult {
    Opened,

    /** No app on the phone takes it. */
    NoApp,

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
 * Where the address [url] is handed. A web address goes to the browser and a phone number to the dialer
 * (which only shows the number: nothing is dialled). The core has checked and written the address
 * already; a scheme other than these is never handed on here either.
 */
internal fun requestFor(url: String): LinkRequest? {
    val lower = url.lowercase()
    return when {
        lower.startsWith("http://") || lower.startsWith("https://") -> LinkRequest(Intent.ACTION_VIEW, url)
        lower.startsWith("tel:") -> LinkRequest(Intent.ACTION_DIAL, url)
        else -> null
    }
}

/**
 * The part of the platform that shows an address to the system, from the window in front. The core
 * asks for it through `Platform.openLink`, after it has checked the address.
 */
internal class SystemLinks(
    private val context: Context,
    private val foreground: () -> Activity?,
) {
    fun launch(url: String) {
        val request = requestFor(url) ?: throw HostException.Unsupported()
        val intent = Intent(request.action, request.address.toUri()).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            val activity = foreground()
            if (activity != null) {
                activity.startActivity(intent)
            } else {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
        } catch (_: RuntimeException) {
            // (No app takes it, or the system refused.)
            throw HostException.Failed()
        }
    }
}

/** [LinkOpener] over the core: it checks the address and has the platform show it. */
internal class CoreLinks(private val core: Core) : LinkOpener {
    override fun open(link: LinkInfo): LinkResult {
        // (A mail address is for the composer, which the screen opens itself; it is never handed to the platform.)
        if (link.goes == LinkGoes.Composer) return LinkResult.Ignored
        return try {
            core.openLink(link.shown)
            LinkResult.Opened
        } catch (_: HostException.Unsupported) {
            LinkResult.Ignored
        } catch (_: HostException) {
            LinkResult.NoApp
        }
    }
}
