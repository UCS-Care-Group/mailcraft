// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val RowHeight = 48.dp

/** What Today asks of the app: each card opens what it is about. */
internal class TodayActions(
    val onMail: () -> Unit = {},
    val onEvent: (ULong) -> Unit = {},
    val onTask: (ULong) -> Unit = {},
)

/**
 * Today: the date, the unread count, the next events and the tasks due, as cards. On a wide window
 * the events and the tasks sit side by side.
 */
@Composable
internal fun TodayScreen(texts: Texts, state: TodayState, actions: TodayActions, modifier: Modifier = Modifier) {
    when (state) {
        TodayState.Loading -> Centered(texts.t("Loading…"), modifier)
        is TodayState.Failed -> Centered(state.message, modifier)
        is TodayState.Ready -> TodayCards(texts, state.model, actions, modifier)
    }
}

@Composable
private fun TodayCards(texts: Texts, model: TodayModel, actions: TodayActions, modifier: Modifier) {
    BoxWithConstraints(modifier = modifier.fillMaxSize()) {
        val wide = maxWidth >= TwoPaneWidth
        Column(
            modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(Gutter),
            verticalArrangement = Arrangement.spacedBy(Gutter),
        ) {
            Text(
                todayHeading(model.date),
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.semantics { heading() },
            )
            UnreadCard(texts, model.unread, actions.onMail)
            if (wide) {
                Row(horizontalArrangement = Arrangement.spacedBy(Gutter)) {
                    EventsCard(texts, model, actions.onEvent, Modifier.weight(1f))
                    TasksCard(texts, model, actions.onTask, Modifier.weight(1f))
                }
            } else {
                EventsCard(texts, model, actions.onEvent, Modifier.fillMaxWidth())
                TasksCard(texts, model, actions.onTask, Modifier.fillMaxWidth())
            }
        }
    }
}

@Composable
private fun Muted(text: String) {
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun dueColour(late: Boolean) =
    if (late) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant

@Composable
private fun CardTitle(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleMedium,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(bottom = 4.dp).semantics { heading() },
    )
}

@Composable
private fun UnreadCard(texts: Texts, unread: Int, onClick: () -> Unit) {
    val label = texts.t("Open the Inbox")
    val open = Modifier.fillMaxWidth().clickable(onClickLabel = label, role = Role.Button, onClick = onClick)
    Card(modifier = open) {
        Column(modifier = Modifier.padding(Gutter)) {
            CardTitle(texts.t("Inbox"))
            Text(
                if (unread == 0) {
                    texts.t("No unread messages")
                } else {
                    texts.tn(unread.toULong(), "{n} unread message", "{n} unread messages")
                },
                style = MaterialTheme.typography.bodyLarge,
            )
        }
    }
}

@Composable
private fun EventsCard(texts: Texts, model: TodayModel, onEvent: (ULong) -> Unit, modifier: Modifier) {
    Card(modifier = modifier) {
        Column(modifier = Modifier.padding(Gutter)) {
            CardTitle(texts.t("Next events"))
            if (model.events.isEmpty()) Muted(texts.t("Nothing coming up."))
            model.events.forEach { event ->
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = RowHeight)
                        .clickable(onClickLabel = texts.t("Open event"), role = Role.Button) { onEvent(event.id) }
                        .padding(vertical = 6.dp),
                ) {
                    Text(event.title.ifBlank { texts.t("(No title)") }, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    val where = if (event.location.isBlank()) "" else " · " + event.location
                    Text(
                        eventWhen(texts, event, model.date) + where,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
    }
}

@Composable
private fun TasksCard(texts: Texts, model: TodayModel, onTask: (ULong) -> Unit, modifier: Modifier) {
    Card(modifier = modifier) {
        Column(modifier = Modifier.padding(Gutter)) {
            CardTitle(texts.t("Tasks due"))
            if (model.tasks.isEmpty()) Muted(texts.t("No tasks due."))
            model.tasks.take(CARD_LIMIT).forEach { task ->
                val late = isOverdue(task, model.date)
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = RowHeight)
                        .clickable(onClickLabel = texts.t("Edit task"), role = Role.Button) { onTask(task.id) }
                        .padding(vertical = 6.dp),
                ) {
                    Text(task.title, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    task.due?.let { due ->
                        Text(
                            (if (late) texts.t("Overdue") + ", " else "") + dueLabel(texts, due, model.date),
                            style = MaterialTheme.typography.bodySmall,
                            color = dueColour(late),
                        )
                    }
                }
            }
            val more = model.tasks.size - CARD_LIMIT
            if (more > 0) {
                val style = MaterialTheme.typography.bodySmall
                Text(texts.tn(more.toULong(), "and {n} more", "and {n} more"), style = style)
            }
        }
    }
}
