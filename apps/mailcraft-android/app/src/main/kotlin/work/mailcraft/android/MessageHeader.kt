// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.MessageView

/** Sender, recipients, date and the subject, in words a screen reader reads in order. */
@Composable
internal fun MessageHeader(texts: Texts, view: MessageView) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            view.subject.ifBlank { texts.t("(no subject)") },
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.semantics { heading() },
        )
        HeaderLine(texts.t("From"), listOf(view.from))
        HeaderLine(texts.t("To"), view.to)
        if (view.cc.isNotEmpty()) HeaderLine(texts.t("Cc"), view.cc)
        Text(
            formatDateTime(view.`when`),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** A person as they are shown: the name and the address in full, so that who it really is can be seen. */
internal fun personLabel(who: AddressRecord): String =
    if (who.name.isBlank() || who.name == who.address) who.address else "${who.name} <${who.address}>"

@Composable
private fun HeaderLine(label: String, people: List<AddressRecord>) {
    if (people.isEmpty()) return
    Text(
        "$label: ${people.joinToString(", ") { personLabel(it) }}",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
internal fun AttachmentsLine(texts: Texts, view: MessageView) {
    val shown = view.attachments.filter { !it.inline }.ifEmpty { view.attachments }
    val names = shown.joinToString(", ") { "${it.name} (${sizeLabel(it.size.toLong())})" }
    Text(
        "${texts.tn(shown.size.toULong(), "{n} attachment", "{n} attachments")}: $names",
        style = MaterialTheme.typography.bodyMedium,
        modifier = Modifier.padding(top = 8.dp),
    )
}

