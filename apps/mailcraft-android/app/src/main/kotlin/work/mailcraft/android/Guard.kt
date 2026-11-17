// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.IOException
import java.security.GeneralSecurityException
import uniffi.mailcraft_mobile.HostException

/**
 * Runs the body of a platform entry so that nothing the system throws ever reaches UniFFI: a
 * failure is the binding's own [HostException], which the core turns into its own words. The
 * message of what was thrown is dropped on purpose; it may echo a secret or an address.
 */
@Suppress("ThrowsCount")
internal inline fun <T> guarded(body: () -> T): T =
    try {
        body()
    } catch (e: HostException) {
        throw e
    } catch (_: IOException) {
        throw HostException.Io()
    } catch (_: GeneralSecurityException) {
        throw HostException.Failed()
    } catch (_: RuntimeException) {
        throw HostException.Failed()
    }
