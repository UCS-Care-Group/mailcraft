// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.MessageView

private val DateGap = 8.dp

/**
 * The subject, then the sender with the date and time at the right. Shut, that is one line and the
 * first recipients; [open] (a tap on the line) shows From, To and Cc in full, with every address so
 * that who a message is really from can be seen. A screen reader reads it in order.
 */
@Composable
internal fun MessageHeader(texts: Texts, view: MessageView, open: Boolean, onToggle: (Boolean) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            view.subject.ifBlank { texts.t("(no subject)") },
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.semantics { heading() },
        )
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(
                    onClickLabel = if (open) texts.t("Hide details") else texts.t("Show details"),
                    role = Role.Button,
                ) { onToggle(!open) }
                .padding(vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(DateGap),
        ) {
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                if (open) {
                    HeaderLine(texts.t("From"), listOf(view.from))
                    HeaderLine(texts.t("To"), view.to)
                    if (view.cc.isNotEmpty()) HeaderLine(texts.t("Cc"), view.cc)
                } else {
                    Text(
                        shortName(view.from),
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = FontWeight.Medium,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                    senderDomain(view.from)?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    if (view.to.isNotEmpty()) {
                        Text(
                            "${texts.t("To")}: ${view.to.joinToString(", ") { shortName(it) }}",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
            Text(
                formatDateTime(view.`when`, LocalMessageZone.current, LocalMessageLocale.current),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                // (At a large text size the date wraps rather than squeezing the sender out.)
                modifier = Modifier.widthIn(max = 120.dp),
            )
        }
    }
}

/** The part of the real From address after its last @, lower-cased; null when there is none. Never from the name. */
internal fun senderDomain(who: AddressRecord): String? =
    who.address.substringAfterLast('@', "").trim().lowercase().ifEmpty { null }

/** A person as they are shown in one line: the name, or the address when there is none. */
internal fun shortName(who: AddressRecord): String = who.name.ifBlank { who.address }

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
