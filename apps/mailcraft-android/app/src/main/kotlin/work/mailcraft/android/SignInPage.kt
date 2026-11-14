// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.browser.customtabs.CustomTabsIntent
import androidx.core.net.toUri
import java.lang.ref.WeakReference
import java.net.URI
import java.net.URISyntaxException
import uniffi.mailcraft_mobile.HostException

/** Whether `url` is an address the app may open as a sign-in page: https, with a host, within bounds. */
internal fun isSignInUrl(url: String): Boolean {
    if (url.length > Redirect.MAX_ADDRESS) return false
    return try {
        val uri = URI(url)
        uri.scheme.equals("https", ignoreCase = true) && !uri.host.isNullOrEmpty() && uri.userInfo == null
    } catch (_: URISyntaxException) {
        false
    }
}

/**
 * Opens the provider's sign-in page in a Custom Tab (the system browser's own tab: the app never
 * sees the password, and the owner's browser session and its compliance checks apply) and arms the
 * [handback] that waits for the redirect. No web view is involved.
 */
@Suppress("ThrowsCount")
internal class SignInPage(
    private val context: Context,
    private val handback: SignInHandback,
    private val foreground: () -> Activity?,
) {
    fun open(url: String) {
        if (!isSignInUrl(url)) throw HostException.Failed()
        val intent = CustomTabsIntent.Builder()
            .setShareState(CustomTabsIntent.SHARE_STATE_OFF)
            .setSendToExternalDefaultHandlerEnabled(false)
            .build()
            .intent
        intent.data = url.toUri()
        handback.arm()
        val activity = foreground()
        try {
            if (activity != null) {
                activity.startActivity(intent)
            } else {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
        } catch (_: ActivityNotFoundException) {
            handback.disarm()
            throw HostException.Failed()
        } catch (_: RuntimeException) {
            // A SecurityException or anything else: nobody will wait for a redirect now.
            handback.disarm()
            throw HostException.Failed()
        }
    }
}

/** Remembers the window that is in front, so that a page opens over it. Held weakly. */
internal class ForegroundActivity {
    private var current: WeakReference<Activity>? = null

    fun resumed(activity: Activity) {
        current = WeakReference(activity)
    }

    fun paused(activity: Activity) {
        if (current?.get() === activity) current = null
    }

    fun get(): Activity? = current?.get()?.takeUnless { it.isFinishing }
}
