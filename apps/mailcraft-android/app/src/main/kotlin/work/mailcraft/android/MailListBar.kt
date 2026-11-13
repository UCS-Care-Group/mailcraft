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
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

private val Gutter = 16.dp
private val IconBox = 18.dp

/** How long typing pauses before the search runs. */
private const val SEARCH_DELAY_MILLIS = 300L

/** The bar over the list: the search box and the funnel; under them the filter that is on, as a chip. */
@Composable
internal fun MailListBar(
    texts: Texts,
    query: MailQuery,
    actions: MailListActions,
    menuOpen: Boolean = false,
) {
    var text by rememberSaveable(query.folder) { mutableStateOf(query.search) }
    LaunchedEffect(text) {
        if (text != query.search) {
            delay(SEARCH_DELAY_MILLIS)
            actions.onSearch(text)
        }
    }
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(space = 4.dp),
    ) {
        OutlinedTextField(
            value = text,
            onValueChange = { text = it.take(MAX_SEARCH) },
            modifier = Modifier.weight(1f),
            singleLine = true,
            placeholder = { Text(texts.t("Search mail")) },
            leadingIcon = { Icon(ListIcons.Search, contentDescription = null, modifier = Modifier.size(IconBox)) },
            trailingIcon = {
                if (text.isNotEmpty()) {
                    IconButton(onClick = { text = "" }) {
                        Icon(
                            ListIcons.Close,
                            contentDescription = texts.t("Clear search"),
                            modifier = Modifier.size(IconBox),
                        )
                    }
                }
            },
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { actions.onSearch(text) }),
        )
        FunnelMenu(texts, query, actions, menuOpen)
    }
    ActiveChips(texts, query, actions)
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
