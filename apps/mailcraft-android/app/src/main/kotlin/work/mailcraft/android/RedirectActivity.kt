// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Where the system brings the owner back to after the provider's page: it is registered for the
 * app's own scheme and nothing else. It hands the address to the waiting sign-in, then brings
 * the app's window forward over the browser tab (which is closed by that) and goes. It shows nothing.
 */
class RedirectActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val application = application as? MailCraftApplication
        application?.handback?.deliver(intent?.dataString)
        startActivity(
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
        )
        finish()
    }
}
