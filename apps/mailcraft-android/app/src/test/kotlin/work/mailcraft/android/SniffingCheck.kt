// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/**
 * A stand-in for the core's picture check in tests that run without the native library. The rules
 * themselves are the core's (`picture_accepted`, tested in Rust); this only gives these tests
 * something that says yes to the headers they use and no to the rest.
 */
internal val SniffingCheck = PictureCheck { bytes, icons ->
    fun starts(head: ByteArray) = bytes.size >= head.size && head.indices.all { bytes[it] == head[it] }
    starts(byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)) ||
        starts(byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte())) ||
        starts("GIF87a".toByteArray()) || starts("GIF89a".toByteArray()) ||
        (starts("RIFF".toByteArray()) && bytes.size >= 12 && String(bytes, 8, 4, Charsets.ISO_8859_1) == "WEBP") ||
        (icons && starts(byteArrayOf(0, 0, 1, 0)) && bytes.size > 5 && (bytes[4].toInt() != 0 || bytes[5].toInt() != 0))
}
