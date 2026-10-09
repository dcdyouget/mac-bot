package bot.mac.mobile.feature.computer

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
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
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.Key
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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource
import kotlin.coroutines.coroutineContext
import kotlin.time.Clock
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
    var fps by remember { mutableStateOf(0f) }
    var frameAgeMs by remember { mutableStateOf(0L) }
    var takeoverHostId by remember { mutableStateOf<String?>(null) }
    var lastFrameAt by remember { mutableStateOf<Long?>(null) }
    val scope = rememberCoroutineScope()
    val activeHost by repository.activeHost.collectAsState()
    val emptyState = remember { MutableStateFlow<ScreenState?>(null) }
    val screenState by (activeConnection?.state ?: emptyState).collectAsState()
    val decoder = remember { platformScreenImageDecoder() }
    val frameClockError = stringResource(Res.string.computer_frame_clock_error)
    val decodeError = stringResource(Res.string.computer_decode_error)
    val connectionError = stringResource(Res.string.computer_connection_error)
    val requestError = stringResource(Res.string.computer_request_failed)
    val canInput = takeover && screenState?.driver == "user"
    // Keep stable State holders so the long-lived pointer coroutine always reads
    // the latest Compose values without restarting on every frame or pan update.
    val currentCanInput = rememberUpdatedState(canInput)
    val currentConnection = rememberUpdatedState(activeConnection)
    val currentFrame = rememberUpdatedState(frame)
    val currentZoom = rememberUpdatedState(zoom)
    val currentPan = rememberUpdatedState(pan)
    val currentViewport = rememberUpdatedState(viewport)
    val inputQueue = remember { InputEventQueue<InputCommand>(capacity = 64) }
    val keyboardMutex = remember { Mutex() }

    LaunchedEffect(Unit) {
        while (true) {
            val command = inputQueue.receive()
            try {
                command.connection.sendInput(command.payload)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                // The screen connection owns reconnecting; stale input is safely discarded.
            }
        }
    }

    LaunchedEffect(tabId) { selectedTab = tabId }

    LaunchedEffect(botId, quality, activeHost?.id) {
        val connectionHostId = activeHost?.id
        takeoverError = null
        screenError = null; frameImage = null; frame = null
        zoom = 1f; pan = Offset.Zero
        fps = 0f; frameAgeMs = 0L; lastFrameAt = null
        val frameClock = coroutineContext[MonotonicFrameClock]
        if (frameClock == null) {
            screenError = frameClockError
            return@LaunchedEffect
        }
        val connectionResult = runCatching {
            repository.createScreen(botId, quality.wireName, selectedTab) { incoming ->
                val decoded = runCatching { decoder.decodeJpeg(incoming.jpeg) }.getOrNull()
                if (decoded == null) {
                    withContext(Dispatchers.Main.immediate) { screenError = decodeError }
                    throw IllegalStateException(decodeError)
                }
                withContext(Dispatchers.Main.immediate + frameClock) {
                    frame = incoming
                    frameImage = decoded
                    withFrameNanos { }
                    val renderedAt = Clock.System.now().toEpochMilliseconds()
                    lastFrameAt?.let { previous ->
                        if (renderedAt > previous) fps = (1000f / (renderedAt - previous)).coerceIn(0f, 60f)
                    }
                    frameAgeMs = (renderedAt - incoming.header.timestampMillis).coerceAtLeast(0L)
                    lastFrameAt = renderedAt
                }
            }
        }
        val connection = connectionResult.getOrNull()
        if (connection == null) {
            screenError = connectionResult.exceptionOrNull()?.message ?: connectionError
            return@LaunchedEffect
        }
        activeConnection = connection
        val runner: Job = connection.start()
        try { runner.join() } catch (cancelled: CancellationException) { throw cancelled }
        finally {
            withContext(NonCancellable) {
                if (takeoverHostId == connectionHostId && connectionHostId != null) {
                    releaseTakeover(repository, connectionHostId, botId)
                    takeoverHostId = null
                    takeover = false
                }
                try { connection.stop() } catch (_: Throwable) { }
            }
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
        Row(Modifier.fillMaxWidth().height(40.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(32.dp).clickable(onClick = onBack), contentAlignment = Alignment.Center) {
                Text("‹", color = Color.White, style = MaterialTheme.typography.headlineSmall)
            }
            Text(stringResource(Res.string.computer_title), Modifier.weight(1f), color = Color.White, style = MaterialTheme.typography.titleMedium)
            Text(statusLabel(takeover, screenState?.driver), color = if (takeover) MaterialTheme.colorScheme.primary else Color.White, style = MaterialTheme.typography.labelMedium)
        }
        HorizontalDivider(color = Color.DarkGray)
        if (tabs.isNotEmpty()) {
            LazyRow(
                Modifier.fillMaxWidth().height(40.dp).padding(horizontal = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                item { Text(stringResource(Res.string.computer_tabs), color = Color.LightGray, style = MaterialTheme.typography.labelSmall) }
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
        Box(Modifier.fillMaxWidth().weight(1f).padding(8.dp).clipToBounds().onSizeChanged { viewport = it }
            .pointerInput(Unit) {
                awaitEachGesture {
                    val first = awaitFirstDown(requireUnconsumed = false)
                    val gestureConnection = currentConnection.value ?: return@awaitEachGesture
                    var multiTouch = false
                    var touchStarted = false
                    var firstEvent = true
                    var previousDistance = 0f
                    var previousCentroid = first.position
                    try {
                        while (currentCanInput.value) {
                            val event = awaitPointerEvent(PointerEventPass.Main)
                            val pressed = event.changes.filter { it.pressed }
                            if (pressed.size >= 2) {
                                if (!multiTouch && touchStarted) {
                                    queueTouch(inputQueue, scope, gestureConnection, "end", null)
                                    touchStarted = false
                                }
                                val centroid = pressed.map { it.position }.centroid()
                                val distance = pressed.take(2).let { distance(it[0].position, it[1].position) }
                                if (!multiTouch) {
                                    multiTouch = true
                                    previousDistance = distance
                                    previousCentroid = centroid
                                } else {
                                    val movement = centroid - previousCentroid
                                    if (previousDistance > 0f) {
                                        val scaleDelta = distance / previousDistance
                                        if (kotlin.math.abs(scaleDelta - 1f) > 0.01f) {
                                            zoom = (zoom * scaleDelta).coerceIn(1f, 4f)
                                            pan += movement
                                        } else if (movement.getDistance() > 1f) {
                                            queueWheel(inputQueue, gestureConnection, mapToFrame(centroid, currentViewport.value, currentFrame.value, currentZoom.value, currentPan.value), -movement.x, -movement.y)
                                        }
                                    }
                                    previousDistance = distance
                                    previousCentroid = centroid
                                }
                                event.changes.forEach { it.consume() }
                            } else if (!multiTouch) {
                                val change = pressed.firstOrNull()
                                if (change != null && !firstEvent) {
                                    if (!touchStarted) {
                                        queueTouch(inputQueue, scope, gestureConnection, "start", mapToFrame(change.position, currentViewport.value, currentFrame.value, currentZoom.value, currentPan.value))
                                        touchStarted = true
                                    }
                                    queueTouch(inputQueue, scope, gestureConnection, "move", mapToFrame(change.position, currentViewport.value, currentFrame.value, currentZoom.value, currentPan.value))
                                    change.consume()
                                }
                            }
                            firstEvent = false
                            if (event.changes.all { !it.pressed }) {
                                if (!multiTouch) {
                                    if (!touchStarted) {
                                        queueTouch(inputQueue, scope, gestureConnection, "start", mapToFrame(first.position, currentViewport.value, currentFrame.value, currentZoom.value, currentPan.value))
                                    }
                                    queueTouch(inputQueue, scope, gestureConnection, "end", null)
                                    touchStarted = false
                                }
                                break
                            }
                        }
                    } finally {
                        if (touchStarted) queueTouch(inputQueue, scope, gestureConnection, "end", null)
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
        if (canInput) {
            Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(value = textInput, onValueChange = { next ->
                    val previous = textInput
                    textInput = next
                    if (next != previous) scope.launch {
                        keyboardMutex.withLock {
                            when {
                                next.startsWith(previous) -> enqueueText(inputQueue, activeConnection, next.removePrefix(previous))
                                previous.startsWith(next) -> repeat(previous.length - next.length) { enqueueKey(inputQueue, activeConnection, "Backspace") }
                                else -> {
                                    repeat(previous.length) { enqueueKey(inputQueue, activeConnection, "Backspace") }
                                    enqueueText(inputQueue, activeConnection, next)
                                }
                            }
                        }
                    }
                }, modifier = Modifier.weight(1f).onPreviewKeyEvent { event ->
                    if (event.type == KeyEventType.KeyDown) {
                        composeKeyDescriptor(event.key)?.let { (key, code) ->
                            scope.launch { keyboardMutex.withLock { enqueueKey(inputQueue, activeConnection, key, code) } }
                            true
                        } ?: false
                    } else false
                }, placeholder = { Text(stringResource(Res.string.computer_keyboard_hint)) }, singleLine = true, colors = OutlinedTextFieldDefaults.colors(
                    focusedTextColor = Color.White,
                    unfocusedTextColor = Color.White,
                    cursorColor = Color.White,
                    focusedPlaceholderColor = Color.LightGray,
                    unfocusedPlaceholderColor = Color.LightGray,
                    unfocusedBorderColor = Color.Gray,
                ))
            }
        }
        Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.computer_quality), color = Color.LightGray, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(end = 4.dp))
            Quality.entries.forEach { candidate ->
                FilterChip(quality == candidate, { if (!takeover) quality = candidate }, enabled = !takeover, label = { Text(candidate.label()) })
            }
            Text("×${zoom.toString().take(4)}", color = Color.LightGray, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(start = 4.dp))
            Text(stringResource(Res.string.computer_fps, fps.toInt(), frameAgeMs), color = Color.LightGray, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(start = 4.dp))
            Text(
                takeoverError ?: statusLabel(takeover, screenState?.driver),
                color = if (takeoverError != null) MaterialTheme.colorScheme.error else Color.Gray,
                maxLines = 1,
                style = MaterialTheme.typography.labelSmall,
                modifier = Modifier.weight(1f).padding(start = 4.dp),
            )
            Button(onClick = {
                if (!takeover) scope.launch {
                    takeoverError = captureError(requestError) { repository.call("takeover.start", buildJsonObject { put("bot_id", botId) }) }
                    if (takeoverError == null) {
                        takeoverHostId = activeHost?.id
                        takeover = true
                    }
                } else releaseDialog = true
            }, modifier = Modifier.height(36.dp)) { Text(if (takeover) stringResource(Res.string.computer_release) else stringResource(Res.string.computer_takeover)) }
        }
    }
    if (releaseDialog) AlertDialog(onDismissRequest = { releaseDialog = false }, title = { Text(stringResource(Res.string.computer_release)) }, text = { OutlinedTextField(releaseNote, { releaseNote = it }, label = { Text(stringResource(Res.string.computer_release_note)) }) }, confirmButton = { Button(onClick = {
        scope.launch {
            takeoverError = captureError(requestError) { repository.call("takeover.release", buildJsonObject { put("bot_id", botId); if (releaseNote.isNotBlank()) put("note", releaseNote) }) }
            if (takeoverError == null) { takeoverHostId = null; takeover = false; releaseDialog = false; releaseNote = "" }
        }
    }) { Text(stringResource(Res.string.computer_release)) } }, dismissButton = { TextButton(onClick = { releaseDialog = false }) { Text(stringResource(Res.string.common_close)) } })
}

private val Quality.wireName: String get() = name.lowercase()
private data class InputCommand(val connection: ScreenConnection, val payload: JsonObject)

/**
 * Input delivery keeps a bounded queue without sacrificing lifecycle events.
 * Transient motion can be dropped under slow network conditions; start/end/key
 * events use [sendImportant] and therefore apply backpressure in FIFO order.
 */
internal class InputEventQueue<T>(capacity: Int = 64) {
    private val channel = Channel<T>(capacity = capacity, onBufferOverflow = BufferOverflow.SUSPEND)

    suspend fun receive(): T = channel.receive()

    suspend fun sendImportant(value: T) = channel.send(value)

    fun trySendTransient(value: T): Boolean = channel.trySend(value).isSuccess
}

private fun queueTouch(channel: InputEventQueue<InputCommand>, scope: CoroutineScope, connection: ScreenConnection?, action: String, point: Offset?) {
    connection ?: return
    val command = InputCommand(connection, buildJsonObject {
        put("type", "touch")
        put("action", action)
        put("points", if (point == null) JsonArray(emptyList()) else JsonArray(listOf(buildJsonObject { put("x", point.x); put("y", point.y) })))
    })
    if (action == "move") {
        channel.trySendTransient(command)
    } else {
        scope.launch(start = CoroutineStart.UNDISPATCHED) { channel.sendImportant(command) }
    }
}
private fun queueWheel(channel: InputEventQueue<InputCommand>, connection: ScreenConnection?, point: Offset?, dx: Float, dy: Float) {
    connection ?: return
    channel.trySendTransient(InputCommand(connection, buildJsonObject {
        put("type", "wheel")
        put("x", point?.x ?: 0f)
        put("y", point?.y ?: 0f)
        put("dx", dx)
        put("dy", dy)
    }))
}
private suspend fun enqueueKey(channel: InputEventQueue<InputCommand>, connection: ScreenConnection?, key: String, code: String = key, text: String? = key.takeIf { it.length == 1 }) {
    connection ?: return
    channel.sendImportant(InputCommand(connection, buildJsonObject {
        put("type", "key")
        put("action", "press")
        put("key", key)
        put("code", code)
        put("text", text)
        put("modifiers", JsonArray(emptyList()))
    }))
}
private fun composeKeyDescriptor(key: Key): Pair<String, String>? = when (key) {
    Key.Backspace -> "Backspace" to "Backspace"
    Key.Enter -> "Enter" to "Enter"
    Key.Tab -> "Tab" to "Tab"
    Key.Escape -> "Escape" to "Escape"
    Key.Delete -> "Delete" to "Delete"
    Key.DirectionUp -> "ArrowUp" to "ArrowUp"
    Key.DirectionDown -> "ArrowDown" to "ArrowDown"
    Key.DirectionLeft -> "ArrowLeft" to "ArrowLeft"
    Key.DirectionRight -> "ArrowRight" to "ArrowRight"
    else -> null
}
private suspend fun enqueueText(channel: InputEventQueue<InputCommand>, connection: ScreenConnection?, text: String) {
    text.forEach { enqueueKey(channel, connection, it.toString()) }
}
private suspend fun captureError(fallback: String, block: suspend () -> Unit): String? = try {
    block(); null
} catch (error: CancellationException) {
    throw error
} catch (error: Throwable) {
    error.message ?: fallback
}
private suspend fun releaseTakeover(repository: MobileRepository, hostId: String, botId: String) {
    runCatching { repository.callOnHost(hostId, "takeover.release", buildJsonObject { put("bot_id", botId) }) }
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
