package bot.mac.mobile.feature.computer

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MonotonicFrameClock
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.network.ScreenConnection
import bot.mac.mobile.core.network.ScreenFrame
import bot.mac.mobile.core.network.ScreenState
import bot.mac.mobile.core.platform.LandscapeScreen
import bot.mac.mobile.core.platform.platformScreenImageDecoder
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource
import kotlin.coroutines.coroutineContext
import kotlin.math.sqrt

private enum class Quality { AUTO, LOW, HIGH }

@Composable
fun ComputerScreen(repository: MobileRepository, botId: String, tabId: String? = null, onBack: () -> Unit) {
    LandscapeScreen(enabled = true)
    var quality by remember { mutableStateOf(Quality.AUTO) }
    var takeover by remember { mutableStateOf(false) }
    var takeoverError by remember { mutableStateOf<String?>(null) }
    var screenError by remember { mutableStateOf<String?>(null) }
    var activeConnection by remember { mutableStateOf<ScreenConnection?>(null) }
    var selectedTab by remember { mutableStateOf(tabId) }
    var frameImage by remember { mutableStateOf<ImageBitmap?>(null) }
    var frame by remember { mutableStateOf<ScreenFrame?>(null) }
    var zoom by remember { mutableFloatStateOf(1f) }
    var pan by remember { mutableStateOf(Offset.Zero) }
    var viewport by remember { mutableStateOf(IntSize.Zero) }
    var releaseDialog by remember { mutableStateOf(false) }
    var releaseNote by remember { mutableStateOf("") }
    var textInput by remember { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    val activeHost by repository.activeHost.collectAsState()
    val emptyState = remember { MutableStateFlow<ScreenState?>(null) }
    val screenState by (activeConnection?.state ?: emptyState).collectAsState()
    val decoder = remember { platformScreenImageDecoder() }
    val canInput = takeover && screenState?.driver == "user"
    val inputQueue = remember { Channel<InputCommand>(Channel.UNLIMITED) }

    LaunchedEffect(Unit) {
        for (command in inputQueue) {
            runCatching { command.connection.sendInput(command.payload) }
        }
    }

    LaunchedEffect(tabId) { selectedTab = tabId }

    LaunchedEffect(botId, quality, activeHost?.id) {
        takeover = false
        takeoverError = null
        screenError = null; frameImage = null; frame = null
        zoom = 1f; pan = Offset.Zero
        val frameClock = coroutineContext[MonotonicFrameClock]
        if (frameClock == null) {
            screenError = "No Compose frame clock"
            return@LaunchedEffect
        }
        val connectionResult = runCatching {
            repository.createScreen(botId, quality.wireName, selectedTab) { incoming ->
                val decoded = runCatching { decoder.decodeJpeg(incoming.jpeg) }.getOrNull()
                if (decoded == null) {
                    withContext(Dispatchers.Main.immediate) { screenError = "JPEG frame decode failed" }
                    throw IllegalStateException("JPEG frame decode failed")
                }
                withContext(Dispatchers.Main.immediate + frameClock) {
                    frame = incoming
                    frameImage = decoded
                    withFrameNanos { }
                }
            }
        }
        val connection = connectionResult.getOrNull()
        if (connection == null) {
            screenError = connectionResult.exceptionOrNull()?.message ?: "screen connection failed"
            return@LaunchedEffect
        }
        activeConnection = connection
        val runner: Job = connection.start()
        try { runner.join() } catch (cancelled: CancellationException) { throw cancelled }
        finally {
            withContext(NonCancellable) { try { connection.stop() } catch (_: Throwable) { } }
            if (activeConnection === connection) activeConnection = null
        }
    }

    LaunchedEffect(selectedTab, takeover) {
        val connection = activeConnection ?: return@LaunchedEffect
        val stateTab = selectedTab ?: return@LaunchedEffect
        if (takeover) try { connection.switchTab(stateTab) } catch (_: Throwable) { }
    }

    val tabs = screenState?.tabs.orEmpty()
    Column(Modifier.fillMaxSize().background(Color.Black)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Text("‹", color = Color.White, style = MaterialTheme.typography.headlineSmall) }
            Text(stringResource(Res.string.computer_title), Modifier.weight(1f), color = Color.White, style = MaterialTheme.typography.titleMedium)
            Text(statusLabel(takeover, screenState?.driver), color = if (takeover) MaterialTheme.colorScheme.primary else Color.White, style = MaterialTheme.typography.labelMedium)
        }
        HorizontalDivider(color = Color.DarkGray)
        if (tabs.isNotEmpty()) {
            Text(stringResource(Res.string.computer_tabs), color = Color.LightGray, modifier = Modifier.padding(start = 12.dp, top = 6.dp))
            LazyRow(Modifier.fillMaxWidth().padding(horizontal = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                items(tabs, key = { it.tabId }) { tab ->
                    FilterChip(selectedTab == tab.tabId, onClick = {
                        if (canInput) {
                            selectedTab = tab.tabId
                            scope.launch { try { activeConnection?.switchTab(tab.tabId) } catch (_: Throwable) { } }
                        }
                    }, label = { Text(tab.title.ifBlank { tab.url }, maxLines = 1) })
                }
            }
        }
        screenError?.let { Text(stringResource(Res.string.computer_error, it), color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(12.dp)) }
        Box(Modifier.fillMaxWidth().weight(1f).padding(8.dp).aspectRatio(1.6f).onSizeChanged { viewport = it }
            .pointerInput(canInput, activeConnection, frame, zoom, pan, viewport) {
                if (!canInput) return@pointerInput
                awaitEachGesture {
                    val first = awaitFirstDown(requireUnconsumed = false)
                    var multiTouch = false
                    var touchStarted = false
                    var firstEvent = true
                    var previousDistance = 0f
                    var previousCentroid = first.position
                    while (true) {
                        val event = awaitPointerEvent(PointerEventPass.Main)
                        val pressed = event.changes.filter { it.pressed }
                        if (pressed.size >= 2) {
                            multiTouch = true
                            val centroid = pressed.map { it.position }.centroid()
                            val distance = pressed.take(2).let { distance(it[0].position, it[1].position) }
                            if (previousDistance > 0f) zoom = (zoom * (distance / previousDistance)).coerceIn(1f, 4f)
                            pan += centroid - previousCentroid
                            previousDistance = distance
                            previousCentroid = centroid
                            event.changes.forEach { it.consume() }
                        } else if (!multiTouch) {
                            val change = pressed.firstOrNull()
                            if (change != null && !firstEvent) {
                                if (!touchStarted) {
                                    queueTouch(inputQueue, activeConnection, "start", mapToFrame(change.position, viewport, frame, zoom, pan))
                                    touchStarted = true
                                }
                                queueTouch(inputQueue, activeConnection, "move", mapToFrame(change.position, viewport, frame, zoom, pan))
                                change.consume()
                            }
                        }
                        firstEvent = false
                        if (event.changes.all { !it.pressed }) {
                            if (!multiTouch) {
                                if (!touchStarted) {
                                    queueTouch(inputQueue, activeConnection, "start", mapToFrame(first.position, viewport, frame, zoom, pan))
                                }
                                queueTouch(inputQueue, activeConnection, "end", null)
                            }
                            break
                        }
                    }
                }
            }, contentAlignment = Alignment.Center) {
            when {
                frameImage != null -> Image(frameImage!!, contentDescription = frame?.header?.url, modifier = Modifier.fillMaxSize().graphicsLayer(scaleX = zoom, scaleY = zoom, translationX = pan.x, translationY = pan.y), contentScale = ContentScale.Fit)
                screenError != null -> Text(stringResource(Res.string.computer_error, screenError!!), color = MaterialTheme.colorScheme.error)
                screenState == null -> Text(stringResource(Res.string.computer_loading), color = Color.LightGray)
                else -> Text(stringResource(Res.string.computer_no_frame), color = Color.LightGray)
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.computer_quality), color = Color.LightGray, modifier = Modifier.padding(end = 6.dp))
            Quality.entries.forEach { candidate -> FilterChip(quality == candidate, { quality = candidate }, label = { Text(candidate.label()) }) }
            Text("×${zoom.toString().take(4)}", color = Color.LightGray, modifier = Modifier.padding(start = 8.dp))
        }
        if (canInput) OutlinedTextField(value = textInput, onValueChange = { next ->
            val previous = textInput; textInput = next; val added = next.removePrefix(previous)
            if (added.isNotEmpty()) scope.launch { enqueueText(inputQueue, activeConnection, added) } else if (next.length < previous.length) scope.launch { enqueueKey(inputQueue, activeConnection, "Backspace") }
        }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp).onPreviewKeyEvent { event ->
            if (event.type == KeyEventType.KeyDown) { scope.launch { enqueueKey(inputQueue, activeConnection, event.key.toString()) }; true } else false
        }, placeholder = { Text(stringResource(Res.string.computer_keyboard_hint)) }, singleLine = true)
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(if (takeover) Res.string.computer_takeover_active else Res.string.computer_touch_hint), color = Color.Gray, modifier = Modifier.weight(1f))
            Button(onClick = {
                if (!takeover) scope.launch {
                    takeoverError = captureError { repository.call("takeover.start", buildJsonObject { put("bot_id", botId) }) }
                    if (takeoverError == null) takeover = true
                }
                else releaseDialog = true
            }) { Text(if (takeover) stringResource(Res.string.computer_release) else stringResource(Res.string.computer_takeover)) }
        }
        takeoverError?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(bottom = 8.dp).align(Alignment.CenterHorizontally)) }
    }
    if (releaseDialog) AlertDialog(onDismissRequest = { releaseDialog = false }, title = { Text(stringResource(Res.string.computer_release)) }, text = { OutlinedTextField(releaseNote, { releaseNote = it }, label = { Text(stringResource(Res.string.computer_release_note)) }) }, confirmButton = { Button(onClick = {
        scope.launch {
            takeoverError = captureError { repository.call("takeover.release", buildJsonObject { put("bot_id", botId); if (releaseNote.isNotBlank()) put("note", releaseNote) }) }
            if (takeoverError == null) { takeover = false; releaseDialog = false; releaseNote = "" }
        }
    }) { Text(stringResource(Res.string.computer_release)) } }, dismissButton = { TextButton(onClick = { releaseDialog = false }) { Text(stringResource(Res.string.common_close)) } })
}

private val Quality.wireName: String get() = name.lowercase()
private data class InputCommand(val connection: ScreenConnection, val payload: JsonObject)
private fun queueTouch(channel: Channel<InputCommand>, connection: ScreenConnection?, action: String, point: Offset?) {
    connection ?: return
    channel.trySend(InputCommand(connection, buildJsonObject {
        put("type", "touch")
        put("action", action)
        put("points", if (point == null) JsonArray(emptyList()) else JsonArray(listOf(buildJsonObject { put("x", point.x); put("y", point.y) })))
    }))
}
private suspend fun enqueueKey(channel: Channel<InputCommand>, connection: ScreenConnection?, key: String) {
    connection ?: return
    channel.send(InputCommand(connection, buildJsonObject {
        put("type", "key")
        put("action", "press")
        put("key", key)
        put("code", key)
        put("text", key.takeIf { it.length == 1 })
        put("modifiers", JsonArray(emptyList()))
    }))
}
private suspend fun enqueueText(channel: Channel<InputCommand>, connection: ScreenConnection?, text: String) {
    text.forEach { enqueueKey(channel, connection, it.toString()) }
}
private suspend fun captureError(block: suspend () -> Unit): String? = try {
    block(); null
} catch (error: CancellationException) {
    throw error
} catch (error: Throwable) {
    error.message ?: "request failed"
}
private fun List<Offset>.centroid(): Offset = if (isEmpty()) Offset.Zero else Offset(sumOf { it.x.toDouble() }.toFloat() / size, sumOf { it.y.toDouble() }.toFloat() / size)
private fun distance(first: Offset, second: Offset): Float = sqrt((first.x - second.x) * (first.x - second.x) + (first.y - second.y) * (first.y - second.y))
internal fun mapToFrame(offset: Offset, viewport: IntSize, frame: ScreenFrame?, zoom: Float, pan: Offset): Offset? {
    val header = frame?.header ?: return null
    if (viewport.width == 0 || viewport.height == 0 || header.width <= 0 || header.height <= 0) return null
    val scale = minOf(viewport.width.toFloat() / header.width, viewport.height.toFloat() / header.height)
    val left = (viewport.width - header.width * scale) / 2f; val top = (viewport.height - header.height * scale) / 2f
    val localX = (offset.x - viewport.width / 2f - pan.x) / zoom + viewport.width / 2f
    val localY = (offset.y - viewport.height / 2f - pan.y) / zoom + viewport.height / 2f
    return Offset(((localX - left) / scale).coerceIn(0f, header.width.toFloat()), ((localY - top) / scale).coerceIn(0f, header.height.toFloat()))
}
@Composable private fun statusLabel(takeover: Boolean, driver: String?): String = when { takeover && driver == "user" -> stringResource(Res.string.computer_takeover_active); driver == "idle" || driver == null -> stringResource(Res.string.computer_idle); else -> stringResource(Res.string.computer_bot_working) }
@Composable private fun Quality.label(): String = when (this) { Quality.AUTO -> stringResource(Res.string.computer_auto); Quality.LOW -> stringResource(Res.string.computer_quality_low); Quality.HIGH -> stringResource(Res.string.computer_quality_high) }
