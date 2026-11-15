// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

private const val KIB = 1024L

/** A number of bytes as an owner reads it: `340 KB`, `12.4 MB`, at least 1 KB. */
internal fun sizeLabel(bytes: Long): String {
    val kib = maxOf(bytes, KIB) / KIB
    return if (kib < KIB) "$kib KB" else "%.1f MB".format(java.util.Locale.ROOT, kib / KIB.toDouble())
}
