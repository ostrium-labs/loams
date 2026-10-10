package dev.loams.app.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// Earth tones for a product named after soil; placeholders until the brand palette lands.
private val Light = lightColorScheme(
    primary = Color(0xFF2E5B45),
    onPrimary = Color.White,
    secondary = Color(0xFF8C6A3F),
    error = Color(0xFFB3261E),
    surface = Color(0xFFFBF8F3),
    background = Color(0xFFFBF8F3),
)

private val Dark = darkColorScheme(
    primary = Color(0xFF9BD1B3),
    secondary = Color(0xFFD9B98A),
    surface = Color(0xFF151A17),
    background = Color(0xFF151A17),
)

@Composable
fun LoamsTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) Dark else Light, content = content)
}
