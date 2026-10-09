package bot.mac.mobile.feature.computer

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.Image
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.platform.platformScreenImageDecoder
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.computer_auto
import bot.mac.mobile.resources.computer_bot_working
import bot.mac.mobile.resources.computer_idle
import bot.mac.mobile.resources.computer_loading
import bot.mac.mobile.resources.computer_no_frame
import bot.mac.mobile.resources.computer_quality
import bot.mac.mobile.resources.computer_quality_high
import bot.mac.mobile.resources.computer_quality_low
import bot.mac.mobile.resources.computer_quality_medium
import bot.mac.mobile.resources.computer_release
import bot.mac.mobile.resources.computer_tabs
import bot.mac.mobile.resources.computer_takeover
import bot.mac.mobile.resources.computer_takeover_active
import bot.mac.mobile.resources.computer_title
import bot.mac.mobile.resources.computer_touch_hint
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

private enum class Quality { AUTO, LOW, MEDIUM, HIGH }

/**
 * The screen channel is deliberately opened only while this page is visible. The repository's
 * screen adapter owns the actual /ws/screen socket; this page only sends input and quality
 * commands, so it remains commonMain and can later be reused by iOS.
 */
@Composable
fun ComputerScreen(repository: MobileRepository, botId: String, tabId: String? = null, onBack: () -> Unit) {
    var selectedTab by remember { mutableStateOf(tabId) }
    var quality by remember { mutableStateOf(Quality.AUTO) }
    var takeover by remember { mutableStateOf(false) }
    var zoom by remember { mutableFloatStateOf(1f) }
    var screen by remember { mutableStateOf<JsonObject?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(botId, selectedTab, quality) {
        repository.call("screen.open", buildJsonObject {
            put("bot_id", botId)
            selectedTab?.let { put("tab_id", it) }
            put("quality", quality.name.lowercase())
        })
        while (true) {
            screen = runCatching { repository.call("screen.state", buildJsonObject { put("bot_id", botId) }) }.getOrNull()
            delay(500)
        }
    }
    androidx.compose.runtime.DisposableEffect(botId) {
        onDispose { scope.launch { repository.call("screen.close", buildJsonObject { put("bot_id", botId) }) } }
    }

    val tabs = screen?.obj("state")?.arr("tabs")?.mapNotNull { it as? JsonObject } ?: screen?.arr("tabs")?.mapNotNull { it as? JsonObject }.orEmpty()
    val driver = screen?.obj("state")?.str("driver") ?: screen?.str("driver")

    Column(Modifier.fillMaxSize().background(Color.Black)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", color = Color.White, style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.computer_title), Modifier.weight(1f), color = Color.White, style = MaterialTheme.typography.titleMedium)
            Text(statusLabel(takeover, driver), color = if (takeover) MaterialTheme.colorScheme.primary else Color.White, style = MaterialTheme.typography.labelMedium)
        }
        HorizontalDivider(color = Color.DarkGray)
        if (tabs.isNotEmpty()) {
            Text(stringResource(Res.string.computer_tabs), color = Color.LightGray, modifier = Modifier.padding(start = 12.dp, top = 6.dp))
            LazyRow(Modifier.fillMaxWidth().padding(horizontal = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                items(tabs, key = { it.str("tab_id") ?: it.hashCode() }) { tab ->
                    val id = tab.str("tab_id")
                    FilterChip(selectedTab == id, { selectedTab = id }, label = { Text(tab.str("title") ?: tab.str("url") ?: "", maxLines = 1) })
                }
            }
        }
        Box(Modifier.fillMaxWidth().weight(1f).padding(8.dp).aspectRatio(1.6f).pointerInput(takeover, zoom) {
            if (!takeover) return@pointerInput
            detectTapGestures { offset ->
                scope.launch { repository.call("screen.input", buildJsonObject { put("bot_id", botId); put("type", "tap"); put("x", offset.x / zoom); put("y", offset.y / zoom); selectedTab?.let { put("tab_id", it) } }) }
            }
        }.pointerInput(takeover) {
            if (takeover) detectTransformGestures { _, pan, scale, _ ->
                zoom = (zoom * scale).coerceIn(1f, 4f)
                if (pan.x != 0f || pan.y != 0f) scope.launch { repository.call("screen.input", buildJsonObject { put("bot_id", botId); put("type", "scroll"); put("dx", pan.x); put("dy", pan.y) }) }
            }
        }) {
            val frame = screen?.obj("frame") ?: screen?.obj("screen_frame")
            if (frame == null) {
                Text(if (screen == null) stringResource(Res.string.computer_loading) else stringResource(Res.string.computer_no_frame), color = Color.LightGray, modifier = Modifier.align(Alignment.Center))
            } else {
                val bytes = frame.arr("bytes").mapNotNull { it.toString().trim('"').toIntOrNull()?.toByte() }.toByteArray()
                val image = remember(bytes.contentHashCode()) { bytes.takeIf { it.isNotEmpty() }?.let { platformScreenImageDecoder().decodeJpeg(it) } }
                if (image != null) Image(image, contentDescription = null, modifier = Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Fit)
                else ScreenFramePlaceholder(frame, zoom)
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.computer_quality), color = Color.LightGray, modifier = Modifier.padding(end = 6.dp))
            Quality.entries.forEach { candidate -> FilterChip(quality == candidate, { quality = candidate }, label = { Text(candidate.label()) }) }
        }
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("×${zoom.toString().take(4)}", color = Color.LightGray, modifier = Modifier.padding(end = 6.dp))
            Slider(zoom, { zoom = it }, valueRange = 1f..4f, modifier = Modifier.weight(1f))
            Button(onClick = {
                takeover = !takeover
                scope.launch { repository.call(if (takeover) "takeover.start" else "takeover.release", buildJsonObject { put("bot_id", botId) }) }
            }) { Text(if (takeover) stringResource(Res.string.computer_release) else stringResource(Res.string.computer_takeover)) }
        }
        Text(stringResource(if (takeover) Res.string.computer_takeover_active else Res.string.computer_touch_hint), color = Color.Gray, modifier = Modifier.align(Alignment.CenterHorizontally).padding(bottom = 8.dp))
    }
}

@Composable
private fun ScreenFramePlaceholder(frame: JsonObject, zoom: Float) {
    // The platform screen adapter replaces this placeholder with a decoded ImageBitmap when the
    // binary frame is available. Keeping the protocol metadata visible also helps on slow links.
    Column(Modifier.fillMaxSize().background(Color(0xFF111118)).padding(12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        Text(frame.str("mime") ?: "image/jpeg", color = Color.LightGray)
        Text(frame.str("seq") ?: "", color = Color.DarkGray, style = MaterialTheme.typography.labelSmall)
        Text("×${zoom.toString().take(4)}", color = Color.DarkGray, style = MaterialTheme.typography.labelSmall)
    }
}

@Composable
private fun statusLabel(takeover: Boolean, driver: String?): String = when {
    takeover -> stringResource(Res.string.computer_takeover_active)
    driver == "idle" || driver == null -> stringResource(Res.string.computer_idle)
    else -> stringResource(Res.string.computer_bot_working)
}

@Composable
private fun Quality.label(): String = when (this) {
    Quality.AUTO -> stringResource(Res.string.computer_auto)
    Quality.LOW -> stringResource(Res.string.computer_quality_low)
    Quality.MEDIUM -> stringResource(Res.string.computer_quality_medium)
    Quality.HIGH -> stringResource(Res.string.computer_quality_high)
}
