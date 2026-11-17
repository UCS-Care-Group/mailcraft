// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val LightColors: ColorScheme = lightColorScheme(
    primary = Color(0xFF1B5E8C),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFD3E6F5),
    onPrimaryContainer = Color(0xFF05263D),
    secondary = Color(0xFF4A5F6E),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFDDE6EE),
    onSecondaryContainer = Color(0xFF0B1E2B),
    background = Color(0xFFF8FAFB),
    onBackground = Color(0xFF191C1E),
    surface = Color(0xFFF8FAFB),
    onSurface = Color(0xFF191C1E),
    surfaceVariant = Color(0xFFDDE3E8),
    onSurfaceVariant = Color(0xFF41484D),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFF2F5F7),
    surfaceContainer = Color(0xFFECF0F3),
    surfaceContainerHigh = Color(0xFFE6EBEE),
    surfaceContainerHighest = Color(0xFFE0E5E9),
    outline = Color(0xFF71787E),
    outlineVariant = Color(0xFFC1C7CD),
)

private val DarkColors: ColorScheme = darkColorScheme(
    primary = Color(0xFF9CCBF0),
    onPrimary = Color(0xFF00344F),
    primaryContainer = Color(0xFF004B70),
    onPrimaryContainer = Color(0xFFD3E6F5),
    secondary = Color(0xFFB2C9D9),
    onSecondary = Color(0xFF1C3340),
    secondaryContainer = Color(0xFF334A58),
    onSecondaryContainer = Color(0xFFDDE6EE),
    background = Color(0xFF101416),
    onBackground = Color(0xFFE0E3E5),
    surface = Color(0xFF101416),
    onSurface = Color(0xFFE0E3E5),
    surfaceVariant = Color(0xFF41484D),
    onSurfaceVariant = Color(0xFFC1C7CD),
    surfaceContainerLowest = Color(0xFF0B0F11),
    surfaceContainerLow = Color(0xFF181C1E),
    surfaceContainer = Color(0xFF1C2022),
    surfaceContainerHigh = Color(0xFF272B2D),
    surfaceContainerHighest = Color(0xFF323638),
    outline = Color(0xFF8B9298),
    outlineVariant = Color(0xFF41484D),
)

/** The app's colours: light or dark as the system is. */
@Composable
internal fun MailCraftTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (dark) DarkColors else LightColors, content = content)
}
