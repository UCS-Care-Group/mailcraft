// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.InputChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

private val Gutter = 16.dp
private val IconBox = 18.dp

/** How long typing pauses before the search runs. */
private const val SEARCH_DELAY_MILLIS = 300L

/** What is under the top bar: the filter that is on, and where a search looks, as chips. */
@Composable
internal fun MailListBar(texts: Texts, query: MailQuery, actions: MailListActions) {
    ActiveChips(texts, query, actions)
}

/**
 * What the top bar shows in place of the folder's name while the owner searches: a field that sends
 * the search once typing pauses. The cross in [MailBarActions] ends the search and clears it.
 */
@Composable
internal fun MailSearchField(texts: Texts, query: MailQuery, actions: MailListActions) {
    var text by rememberSaveable(query.folder) { mutableStateOf(query.search) }
    // The search the box last sent (trimmed, as the query keeps it): a search of the query's that is
    // not that one came from elsewhere (the same folder chosen again clears it) and replaces the draft.
    var sent by rememberSaveable(query.folder) { mutableStateOf(query.search) }
    val focus = remember { FocusRequester() }
    // Opened by the search button (nothing searched yet): the keyboard comes up, ready to type.
    LaunchedEffect(Unit) { if (query.search.isEmpty()) runCatching { focus.requestFocus() } }
    LaunchedEffect(query.search) {
        text = searchDraft(query.search, sent, text)
        sent = query.search
    }
    LaunchedEffect(text) {
        if (text != query.search) {
            delay(SEARCH_DELAY_MILLIS)
            sent = text.trim()
            actions.onSearch(text)
        }
    }
    TextField(
        value = text,
        onValueChange = { text = it.take(MAX_SEARCH) },
        modifier = Modifier.fillMaxWidth().focusRequester(focus),
        singleLine = true,
        placeholder = { Text(texts.t("Search mail")) },
        colors = TextFieldDefaults.colors(
            focusedContainerColor = Color.Transparent,
            unfocusedContainerColor = Color.Transparent,
            focusedIndicatorColor = Color.Transparent,
            unfocusedIndicatorColor = Color.Transparent,
        ),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
        keyboardActions = KeyboardActions(
            onSearch = {
                sent = text.trim()
                actions.onSearch(text)
            },
        ),
    )
}

/** The top bar's buttons on the Mail tab: search (a cross while searching, which ends it) and the funnel. */
@Composable
internal fun MailBarActions(
    texts: Texts,
    query: MailQuery,
    actions: MailListActions,
    searching: Boolean,
    onSearching: (Boolean) -> Unit,
    menuOpen: Boolean = false,
) {
    if (searching) {
        IconButton(
            onClick = {
                actions.onSearch("")
                onSearching(false)
            },
        ) {
            Icon(ListIcons.Close, contentDescription = texts.t("Clear search"))
        }
    } else {
        IconButton(onClick = { onSearching(true) }) {
            Icon(ListIcons.Search, contentDescription = texts.t("Search mail"))
        }
    }
    FunnelMenu(texts, query, actions, menuOpen)
}

/** The longest search text the box takes. */
private const val MAX_SEARCH = 256

@Composable
private fun FunnelMenu(texts: Texts, query: MailQuery, actions: MailListActions, initiallyOpen: Boolean) {
    var open by remember { mutableStateOf(initiallyOpen) }
    Box {
        IconButton(onClick = { open = true }) {
            Icon(
                ListIcons.Filter,
                contentDescription = texts.t("Filter"),
                tint = if (query.filter != ListFilter.All || query.conversations) {
                    MaterialTheme.colorScheme.primary
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
            )
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            ListFilter.entries.forEach { filter ->
                DropdownMenuItem(
                    text = { Text(filter.label(texts)) },
                    trailingIcon = { if (filter == query.filter) Icon(AppIcons.Check, contentDescription = null) },
                    onClick = {
                        open = false
                        actions.onFilter(filter)
                    },
                )
            }
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text(texts.t("Conversations")) },
                trailingIcon = { if (query.conversations) Icon(AppIcons.Check, contentDescription = null) },
                onClick = {
                    open = false
                    actions.onConversations(!query.conversations)
                },
            )
        }
    }
}

@Composable
private fun ActiveChips(texts: Texts, query: MailQuery, actions: MailListActions) {
    val searching = query.search.isNotBlank()
    if (query.filter == ListFilter.All && !searching) return
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter),
        horizontalArrangement = Arrangement.spacedBy(space = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (query.filter != ListFilter.All) {
            InputChip(
                selected = true,
                onClick = { actions.onFilter(ListFilter.All) },
                label = { Text(query.filter.label(texts)) },
                trailingIcon = {
                    Icon(
                        ListIcons.Close,
                        contentDescription = texts.t("Remove filter"),
                        modifier = Modifier.size(IconBox).semantics { contentDescription = texts.t("Remove filter") },
                    )
                },
            )
        }
        if (searching) {
            FilterChip(
                selected = !query.everywhere,
                onClick = { actions.onEverywhere(false) },
                label = { Text(texts.t("This folder")) },
            )
            FilterChip(
                selected = query.everywhere,
                onClick = { actions.onEverywhere(true) },
                label = { Text(texts.t("All accounts")) },
            )
        }
    }
}

/**
 * What the search box holds once the query's search is [search]: the [draft] while that is the search
 * the box [sent] (what is typed after it stays), else [search], which came from elsewhere.
 */
internal fun searchDraft(search: String, sent: String, draft: String): String = if (search == sent) draft else search
