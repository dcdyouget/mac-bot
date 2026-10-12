package bot.mac.mobile.feature.memory

import androidx.compose.foundation.background
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import bot.mac.mobile.core.design.BotAvatar
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.protocol.*
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.datetime.Instant
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime
import org.jetbrains.compose.resources.stringResource
import kotlin.math.roundToInt

@Composable
fun MemoryScreen(
    repository: MobileRepository,
    onBack: () -> Unit,
    onOpenEntry: (String) -> Unit,
    onOpenProfileField: (String) -> Unit,
    onOpenSource: (chatId: String, messageId: String) -> Unit,
) {
    val state by repository.state.collectAsState()
    val activeHost by repository.activeHost.collectAsState()
    val hostId = activeHost?.id.orEmpty()
    val scope = rememberCoroutineScope()
    var query by rememberSaveable { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var deleteConflict by remember { mutableStateOf<Pair<JsonObject, JsonObject>?>(null) }
    var loaded by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val loadingError = stringResource(Res.string.feature_loading_error)
    val quotaError = stringResource(Res.string.memory_quota_full)
    val conflictError = stringResource(Res.string.memory_conflict_simple)
    val deletedText = stringResource(Res.string.memory_deleted)
    val undoText = stringResource(Res.string.memory_undo)
    val conflictDeleteMine = stringResource(Res.string.memory_conflict_delete_mine)
    val conflictKeepLatest = stringResource(Res.string.memory_conflict_keep_latest)

    LaunchedEffect(hostId) {
        loaded = false
        error = null
        if (hostId.isNotBlank()) {
            runCatching {
                repository.callOnHost(hostId, "memory.user.list")
                repository.callOnHost(hostId, "memory.profile.get")
            }.onSuccess { loaded = true }.onFailure { error = loadingError; loaded = true }
        }
    }

    val entries = sortMemoryEntries(state.memoryEntries.filter { it.str("content").contains(query, ignoreCase = true) })
    fun deleteEntry(entry: JsonObject, expectedVersion: Long = entry.long("version")) {
        scope.launch {
            runCatching {
                repository.callOnHost(hostId, "memory.user.delete", buildJsonObject {
                    put("entry_id", entry.str("id"))
                    put("expected_version", expectedVersion)
                })
            }.onSuccess {
                val snackbarResult = scope.async { snackbar.showSnackbar(deletedText, undoText, withDismissAction = true, duration = SnackbarDuration.Indefinite) }
                delay(5_000)
                snackbar.currentSnackbarData?.dismiss()
                val result = snackbarResult.await()
                if (result == SnackbarResult.ActionPerformed) {
                    runCatching { repository.callOnHost(hostId, "memory.user.create", buildJsonObject { put("content", entry.str("content")) }) }
                        .onFailure { error = if ((it as? NetworkError)?.code == "quota_full") quotaError else loadingError }
                }
            }.onFailure { failure ->
                val current = (failure as? NetworkError)?.details?.obj("details")?.obj("current")
                if ((failure as? NetworkError)?.code == "conflict" && current != null) deleteConflict = entry to current
                else error = if ((failure as? NetworkError)?.code == "conflict") conflictError else loadingError
            }
        }
    }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            FontScaleToolbar {
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
                    Text(stringResource(Res.string.memory_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                    TextButton(onClick = { onOpenEntry("") }) { Text("＋", style = MaterialTheme.typography.titleLarge) }
                }
            }
            error?.let { Text(it, Modifier.fillMaxWidth().padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.error) }
            LazyColumn(
                Modifier.weight(1f).fillMaxWidth(),
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 28.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                item {
                    Text(stringResource(Res.string.memory_desc), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                item {
                    if (!loaded) ProfileSkeleton()
                    else ProfileCard(repository, hostId, state.userProfile)
                }
                if (state.memoryQuota.isNotEmpty()) item { QuotaCard(state.memoryQuota) }
                item {
                    OutlinedTextField(
                        value = query,
                        onValueChange = { query = it },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text(stringResource(Res.string.memory_search)) },
                    )
                }
                if (!loaded) {
                    item { Box(Modifier.fillMaxWidth().padding(vertical = 28.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator() } }
                } else if (entries.isEmpty()) {
                    item {
                        Text(
                            stringResource(if (query.isBlank()) Res.string.memory_empty else Res.string.memory_search_empty),
                            Modifier.fillMaxWidth().padding(vertical = 20.dp),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                } else {
                    items(entries, key = { it.str("id") }) { entry ->
                        MemoryEntryRow(
                            entry = entry,
                            query = query,
                            repository = repository,
                            chats = state.chats,
                            bots = state.bots,
                            onClick = { onOpenEntry(entry.str("id")) },
                            onOpenSource = onOpenSource,
                            onSwipeDelete = { deleteEntry(entry) },
                        )
                    }
                }
                item { Text(stringResource(Res.string.memory_mobile_hint), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }

    deleteConflict?.let { (entry, current) ->
        AlertDialog(
            onDismissRequest = { deleteConflict = null },
            title = { Text(stringResource(Res.string.memory_conflict_title)) },
            text = { Text(stringResource(Res.string.memory_conflict, current.obj("updated_by").str("kind"), current.long("version")) + "\n" + current.str("content")) },
            confirmButton = {
                TextButton(onClick = { deleteConflict = null; deleteEntry(entry, current.long("version")) }) {
                    Text(conflictDeleteMine, color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = {
                TextButton(onClick = {
                    deleteConflict = null
                    scope.launch { runCatching { repository.callOnHost(hostId, "memory.user.list") }.onFailure { error = loadingError } }
                }) { Text(conflictKeepLatest) }
            },
        )
    }
}

@Composable
private fun ProfileSkeleton() {
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface, RoundedCornerShape(14.dp)).padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(Modifier.width(92.dp).height(18.dp).background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(5.dp)))
        repeat(4) { Box(Modifier.fillMaxWidth().height(36.dp).background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.7f), RoundedCornerShape(5.dp))) }
    }
}

@Composable
private fun ProfileCard(repository: MobileRepository, hostId: String, profile: JsonObject) {
    val scope = rememberCoroutineScope()
    val palette = LocalMobilePalette.current
    val version = profile.long("version")
    var editState by remember(hostId) { mutableStateOf(MemoryProfileEditState(profile)) }
    var requestInFlight by remember(hostId) { mutableStateOf(false) }
    val saveMutex = remember(hostId) { Mutex() }
    val conflict = editState.conflict
    var error by remember(hostId) { mutableStateOf<String?>(null) }
    var savedField by remember(hostId) { mutableStateOf<String?>(null) }
    var savedGeneration by remember(hostId) { mutableStateOf(0) }
    var expandedLanguage by remember { mutableStateOf(false) }
    val loadingError = stringResource(Res.string.feature_loading_error)

    LaunchedEffect(version, requestInFlight) {
        editState = editState.receiveRemote(profile, requestInFlight)
    }
    fun showSaved(field: String) {
        savedField = field
        val generation = ++savedGeneration
        scope.launch {
            delay(1_600)
            if (savedGeneration == generation && savedField == field) savedField = null
        }
    }
    fun save(field: String, submitted: String) {
        if (profile.isEmpty()) return
        scope.launch {
            saveMutex.withLock {
                if (!editState.isCurrentSubmission(field, submitted)) return@withLock
                val value = submitted.trim().takeIf(String::isNotBlank)
                if ((value ?: "") == editState.base.str(field)) return@withLock
                requestInFlight = true
                error = null
                runCatching {
                    repository.callOnHost(hostId, "memory.profile.update", profileUpdateParams(field, value, editState.expectedVersion))
                }.onSuccess {
                    val updated = editState.saveSucceeded(it.obj("profile"), field, submitted)
                    editState = updated
                    if (updated.drafts[field].orEmpty() == updated.base.str(field)) showSaved(field)
                    else savedField = null
                }.onFailure { failure ->
                    val remote = (failure as? NetworkError)?.details?.obj("details")?.obj("current")
                    if ((failure as? NetworkError)?.code == "conflict" && remote != null) editState = editState.copy(conflict = remote)
                    else error = loadingError
                }
                requestInFlight = false
            }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
      conflict?.let { remote ->
        Surface(Modifier.fillMaxWidth(), color = palette.warning.copy(alpha = 0.14f), shape = RoundedCornerShape(10.dp)) {
            Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(Res.string.memory_profile_conflict_banner, remote.long("version")), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = palette.warning)
                TextButton(onClick = {
                    scope.launch { runCatching { repository.callOnHost(hostId, "memory.profile.get") }.onSuccess {
                        val fresh = it.obj("profile"); editState = editState.useRemote(fresh)
                    }.onFailure { error = loadingError } }
                }) { Text(stringResource(Res.string.memory_profile_conflict_reload), color = palette.warning) }
            }
        }
      }
      Column(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface, RoundedCornerShape(14.dp)).padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(5.dp),
      ) {
        Text(stringResource(Res.string.memory_profile_title), style = MaterialTheme.typography.titleMedium.copy(fontSize = 15.sp), fontWeight = FontWeight.SemiBold)
        ProfileInlineField(
            label = stringResource(Res.string.memory_profile_name), value = editState.drafts["name"].orEmpty(), placeholder = stringResource(Res.string.memory_profile_name_placeholder),
            maxLength = 20, saved = savedField == "name", onValueChange = { savedField = null; editState = editState.edit("name", it) }, onSave = { save("name", it) },
        )
        Row(Modifier.fillMaxWidth().heightIn(min = 42.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.memory_profile_language), Modifier.width(68.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Box(Modifier.weight(1f)) {
                val language = editState.drafts["language"].orEmpty()
                val languageLabel = when (language) {
                    "zh-CN" -> stringResource(Res.string.memory_profile_language_zh)
                    "en-US" -> stringResource(Res.string.memory_profile_language_en)
                    "ja-JP" -> stringResource(Res.string.memory_profile_language_ja)
                    "ko-KR" -> stringResource(Res.string.memory_profile_language_ko)
                    else -> language.ifBlank { stringResource(Res.string.memory_profile_language_system_option) }
                }
                Text(languageLabel, Modifier.fillMaxWidth().clickable { expandedLanguage = true }.padding(vertical = 8.dp), style = MaterialTheme.typography.bodyMedium)
                DropdownMenu(expanded = expandedLanguage, onDismissRequest = { expandedLanguage = false }) {
                    listOf(null to stringResource(Res.string.memory_profile_language_system_option), "zh-CN" to stringResource(Res.string.memory_profile_language_zh), "en-US" to stringResource(Res.string.memory_profile_language_en), "ja-JP" to stringResource(Res.string.memory_profile_language_ja), "ko-KR" to stringResource(Res.string.memory_profile_language_ko)).forEach { (tag, label) ->
                    DropdownMenuItem(text = { Text(label) }, onClick = { expandedLanguage = false; editState = editState.edit("language", tag.orEmpty()); savedField = null; save("language", tag.orEmpty()) })
                    }
                }
            }
            if (savedField == "language") SaveCheck(palette.accent)
        }
        ProfileInlineField(
            label = stringResource(Res.string.memory_profile_identity), value = editState.drafts["identity"].orEmpty(), placeholder = stringResource(Res.string.memory_profile_identity_placeholder),
            maxLength = 100, saved = savedField == "identity", onValueChange = { savedField = null; editState = editState.edit("identity", it) }, onSave = { save("identity", it) },
        )
        Row(Modifier.fillMaxWidth().heightIn(min = 38.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.memory_profile_timezone), Modifier.width(68.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(stringResource(Res.string.memory_profile_timezone_readonly, profile.str("timezone").ifBlank { "—" }), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
      }
    }
}

@Composable
private fun ProfileInlineField(label: String, value: String, placeholder: String, maxLength: Int, saved: Boolean, onValueChange: (String) -> Unit, onSave: (String) -> Unit) {
    val palette = LocalMobilePalette.current
    Row(Modifier.fillMaxWidth().heightIn(min = 42.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.width(68.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedTextField(value = value, onValueChange = { if (it.unicodeScalarCount() <= maxLength) onValueChange(it) }, modifier = Modifier.weight(1f).onFocusChanged { if (!it.isFocused) onSave(value) }, placeholder = { Text(placeholder, style = MaterialTheme.typography.bodySmall) }, singleLine = true, textStyle = MaterialTheme.typography.bodyMedium, keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { onSave(value) }),)
        if (saved) SaveCheck(palette.accent)
    }
}

@Composable
private fun SaveCheck(color: androidx.compose.ui.graphics.Color) {
    var visible by remember { mutableStateOf(false) }
    val savedLabel = stringResource(Res.string.memory_profile_save_success)
    LaunchedEffect(Unit) { visible = true }
    androidx.compose.animation.AnimatedVisibility(visible = visible) { Text("✓", Modifier.padding(start = 6.dp).semantics { contentDescription = savedLabel }, color = color, style = MaterialTheme.typography.labelMedium) }
}

@Composable
private fun QuotaCard(quota: JsonObject) {
    val used = quota.long("used_chars").coerceAtLeast(0L)
    val limit = quota.long("limit_chars").coerceAtLeast(1L)
    val fraction = (used.toFloat() / limit.toFloat()).coerceIn(0f, 1f)
    val palette = LocalMobilePalette.current
    val barColor = when (memoryQuotaLevel(used, limit)) {
        MemoryQuotaLevel.FULL -> MaterialTheme.colorScheme.error
        MemoryQuotaLevel.WARNING -> palette.warning
        MemoryQuotaLevel.NORMAL -> palette.accent
    }
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface, RoundedCornerShape(14.dp)).padding(14.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.memory_usage), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            Text(stringResource(Res.string.memory_usage_value, used, limit), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Box(Modifier.fillMaxWidth().padding(top = 10.dp).height(6.dp).background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(3.dp))) {
            Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(barColor, RoundedCornerShape(3.dp)))
        }
        Text(stringResource(Res.string.memory_usage_hint), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun MemoryEntryRow(
    entry: JsonObject,
    query: String,
    repository: MobileRepository,
    chats: List<JsonObject>,
    bots: List<JsonObject>,
    onClick: () -> Unit,
    onOpenSource: (chatId: String, messageId: String) -> Unit,
    onSwipeDelete: () -> Unit,
) {
    var offsetX by remember(entry.str("id")) { mutableStateOf(0f) }
    val source = entry.obj("source")
    val sender = source.obj("by")
    val sourceChat = chats.firstOrNull { it.str("id") == source.str("chat_id") }
    val chatTitle = sourceChat?.str("title").orEmpty().ifBlank { sourceChat?.str("name").orEmpty() }
    val botName = bots.firstOrNull { it.str("id") == sender.str("bot_id") }?.str("name") ?: sender.str("bot_id")
    val date = formatMemoryTimestamp(entry.str("updated_at"))
    val sourceLabel = when (sender.str("kind")) {
        "user" -> stringResource(Res.string.memory_source_user)
        "system" -> stringResource(Res.string.memory_source_curator)
        "bot" -> stringResource(Res.string.memory_source_bot, botName)
        else -> stringResource(Res.string.memory_source_curator)
    }
    val sourceChatId = source.str("chat_id")
    val sourceMessageId = source.str("message_id")
    Row(
        Modifier.fillMaxWidth()
            .offset { IntOffset(offsetX.roundToInt(), 0) }
            .pointerInput(entry.str("id")) {
                var total = 0f
                val swipeThreshold = 64.dp.toPx()
                val maxOffset = 140.dp.toPx()
                detectHorizontalDragGestures(
                    onDragStart = { total = 0f },
                    onHorizontalDrag = { change, amount ->
                        change.consume()
                        total += amount
                        offsetX = (offsetX + amount).coerceIn(-maxOffset, 0f)
                    },
                    onDragEnd = {
                        if (total < -swipeThreshold) onSwipeDelete()
                        offsetX = 0f
                    },
                    onDragCancel = { offsetX = 0f },
                )
            }
            .background(MaterialTheme.colorScheme.surface, RoundedCornerShape(14.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(highlightMemoryText(entry.str("content"), query, MaterialTheme.colorScheme.primary), style = MaterialTheme.typography.bodyLarge, lineHeight = 23.sp)
            Row(Modifier.fillMaxWidth().padding(top = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                when (sender.str("kind")) {
                    "system" -> SparklesIcon(MaterialTheme.colorScheme.primary)
                    "bot" -> {
                        val bot = bots.firstOrNull { it.str("id") == sender.str("bot_id") }
                        Box(Modifier.padding(end = 4.dp).size(16.dp)) { BotAvatar(repository, bot, main = bot?.get("is_main")?.toString() == "true", size = 16.dp) }
                    }
                }
                Text(sourceLabel, style = MaterialTheme.typography.labelMedium.copy(fontSize = 13.sp), color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                if (sender.str("kind") != "user") {
                    if (chatTitle.isNotBlank() && sourceChatId.isNotBlank() && sourceMessageId.isNotBlank()) {
                        Text(" · $chatTitle", Modifier.weight(1f).clickable { onOpenSource(sourceChatId, sourceMessageId) }, style = MaterialTheme.typography.labelMedium.copy(fontSize = 13.sp), color = MaterialTheme.colorScheme.primary, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    } else if (chatTitle.isNotBlank()) Text(" · $chatTitle", Modifier.weight(1f), style = MaterialTheme.typography.labelMedium.copy(fontSize = 13.sp), color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    if (date.isNotBlank()) Text(" · $date", style = MaterialTheme.typography.labelMedium.copy(fontSize = 13.sp), color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                }
            }
        }
        Text("›", Modifier.padding(start = 8.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun SparklesIcon(color: androidx.compose.ui.graphics.Color) {
    Canvas(Modifier.padding(end = 4.dp).size(16.dp)) {
        val stroke = androidx.compose.ui.graphics.drawscope.Stroke(width = size.minDimension / 12f, cap = androidx.compose.ui.graphics.StrokeCap.Round)
        val center = androidx.compose.ui.geometry.Offset(size.width * 0.52f, size.height * 0.48f)
        drawLine(color, androidx.compose.ui.geometry.Offset(center.x, size.height * 0.02f), androidx.compose.ui.geometry.Offset(center.x, size.height * 0.94f), stroke.width, stroke.cap)
        drawLine(color, androidx.compose.ui.geometry.Offset(size.width * 0.06f, center.y), androidx.compose.ui.geometry.Offset(size.width * 0.98f, center.y), stroke.width, stroke.cap)
        drawLine(color, androidx.compose.ui.geometry.Offset(size.width * 0.81f, size.height * 0.67f), androidx.compose.ui.geometry.Offset(size.width * 0.81f, size.height * 0.98f), stroke.width, stroke.cap)
        drawLine(color, androidx.compose.ui.geometry.Offset(size.width * 0.66f, size.height * 0.82f), androidx.compose.ui.geometry.Offset(size.width * 0.96f, size.height * 0.82f), stroke.width, stroke.cap)
    }
}

@Composable
fun MemoryEntryEditorScreen(repository: MobileRepository, entryId: String, onBack: () -> Unit, onOpenSource: (chatId: String, messageId: String) -> Unit) {
    val state by repository.state.collectAsState()
    val hostId = repository.activeHost.collectAsState().value?.id.orEmpty()
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    val existing = state.memoryEntries.firstOrNull { it.str("id") == entryId }
    var content by rememberSaveable(hostId, entryId) { mutableStateOf("") }
    var baseContent by rememberSaveable(hostId, entryId) { mutableStateOf("") }
    var baseVersion by rememberSaveable(hostId, entryId) { mutableStateOf(0L) }
    var entryLoaded by rememberSaveable(hostId, entryId) { mutableStateOf(entryId.isBlank()) }
    var error by remember { mutableStateOf<String?>(null) }
    var conflict by remember { mutableStateOf<JsonObject?>(null) }
    var deleteConflict by remember { mutableStateOf<JsonObject?>(null) }
    var deleting by remember { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    val loadingError = stringResource(Res.string.feature_loading_error)
    val quotaError = stringResource(Res.string.memory_quota_full)
    val conflictError = stringResource(Res.string.memory_conflict_simple)
    val conflictDeleteMine = stringResource(Res.string.memory_conflict_delete_mine)
    val conflictKeepLatest = stringResource(Res.string.memory_conflict_keep_latest)
    val deletedText = stringResource(Res.string.memory_deleted)
    val undoText = stringResource(Res.string.memory_undo)
    LaunchedEffect(hostId, entryId) {
        if (entryId.isNotBlank()) {
            runCatching {
                val result = repository.callOnHost(hostId, "memory.user.list")
                val current = result.objects("entries").firstOrNull { it.str("id") == entryId }
                if (current != null) {
                    content = current.str("content")
                    baseContent = current.str("content")
                    baseVersion = current.long("version")
                    entryLoaded = true
                } else error = loadingError
            }.onFailure { error = loadingError }
        }
    }
    LaunchedEffect(existing?.long("version"), saving, entryLoaded) {
        if (entryLoaded && !saving && existing != null && existing.long("version") > baseVersion) {
            if (content == baseContent) {
                content = existing.str("content")
                baseContent = existing.str("content")
                baseVersion = existing.long("version")
            } else conflict = existing
        }
    }
    val isNew = entryId.isBlank()
    fun save(expectedVersion: Long = baseVersion) {
        scope.launch {
            if (saving) return@launch
            saving = true
            runCatching {
                if (isNew) repository.callOnHost(hostId, "memory.user.create", buildJsonObject { put("content", content.trim()) })
                else repository.callOnHost(hostId, "memory.user.update", buildJsonObject {
                    put("entry_id", entryId)
                    put("content", content.trim())
                    put("expected_version", expectedVersion)
                })
            }.onSuccess { onBack() }.onFailure { failure ->
                val remote = (failure as? NetworkError)?.details?.obj("details")?.obj("current")
                when {
                    (failure as? NetworkError)?.code == "conflict" && remote != null -> conflict = remote
                    (failure as? NetworkError)?.code == "quota_full" -> error = quotaError
                    else -> error = loadingError
                }
            }
            saving = false
        }
    }
    Box(Modifier.fillMaxSize()) {
      Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).imePadding()) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
                Text(stringResource(if (isNew) Res.string.memory_add_title else Res.string.memory_edit_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            TextButton(onClick = { save() }, enabled = !saving && entryLoaded && content.isNotBlank() && content.unicodeScalarCount() <= 500) { Text("✓", style = MaterialTheme.typography.titleLarge) }
            }
        }
        Column(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
            OutlinedTextField(
                value = content,
                onValueChange = { if (it.unicodeScalarCount() <= 500) content = it },
                modifier = Modifier.fillMaxWidth().weight(1f),
                minLines = 6,
                maxLines = 18,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            )
            Text(stringResource(Res.string.memory_edit_count, content.unicodeScalarCount()), Modifier.fillMaxWidth().padding(vertical = 8.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (existing != null) {
                Text(
                    memoryEntrySourceText(existing, state.chats, state.bots),
                    Modifier.clickable {
                        val source = existing.obj("source")
                        val chatId = source.str("chat_id")
                        val messageId = source.str("message_id")
                        if (chatId.isNotBlank() && messageId.isNotBlank()) onOpenSource(chatId, messageId)
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                )
                Button(onClick = { deleting = true }, enabled = !saving, modifier = Modifier.fillMaxWidth().padding(top = 18.dp), colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer)) {
                    Text(stringResource(Res.string.memory_delete_entry))
                }
            }
            error?.let { Text(it, Modifier.padding(top = 8.dp), color = MaterialTheme.colorScheme.error) }
        }
      }
      SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }
    deleting.takeIf { it }?.let {
        AlertDialog(
            onDismissRequest = { deleting = false },
            title = { Text(stringResource(Res.string.memory_delete_entry)) },
            text = { Text(content) },
            confirmButton = {
                TextButton(onClick = {
                    deleting = false
                    scope.launch {
                        if (saving) return@launch
                        saving = true
                        runCatching { repository.callOnHost(hostId, "memory.user.delete", buildJsonObject { put("entry_id", entryId); put("expected_version", baseVersion) }) }
                            .onSuccess {
                                val result = scope.async { snackbar.showSnackbar(deletedText, undoText, withDismissAction = true, duration = SnackbarDuration.Indefinite) }
                                delay(5_000)
                                snackbar.currentSnackbarData?.dismiss()
                                if (result.await() == SnackbarResult.ActionPerformed) {
                                    runCatching { repository.callOnHost(hostId, "memory.user.create", buildJsonObject { put("content", content) }) }
                                        .onFailure { error = if ((it as? NetworkError)?.code == "quota_full") quotaError else loadingError }
                                }
                                saving = false
                                onBack()
                            }
                            .onFailure { failure ->
                                saving = false
                                val remote = (failure as? NetworkError)?.details?.obj("details")?.obj("current")
                                if ((failure as? NetworkError)?.code == "conflict" && remote != null) deleteConflict = remote
                                else error = if ((failure as? NetworkError)?.code == "conflict") conflictError else loadingError
                            }
                    }
                }) { Text(stringResource(Res.string.common_delete), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { deleting = false }) { Text(stringResource(Res.string.common_cancel)) } },
        )
    }
    conflict?.let { remote ->
        val updatedByKind = remote.obj("updated_by").str("kind")
        val updatedBy = when (updatedByKind) {
            "user" -> stringResource(Res.string.memory_source_user)
            "bot" -> {
                val botId = remote.obj("updated_by").str("bot_id")
                stringResource(Res.string.memory_source_bot, state.bots.firstOrNull { it.str("id") == botId }?.str("name") ?: botId)
            }
            else -> stringResource(Res.string.memory_source_curator)
        }
        val conflictText = stringResource(Res.string.memory_conflict, updatedBy, remote.long("version"))
        AlertDialog(
            onDismissRequest = { conflict = null },
            title = { Text(stringResource(Res.string.memory_conflict_title)) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) { Text(conflictText); Text(remote.str("content"), style = MaterialTheme.typography.bodyMedium) } },
            confirmButton = { TextButton(onClick = { val version = remote.long("version"); conflict = null; save(version) }) { Text(stringResource(Res.string.memory_conflict_mine)) } },
            dismissButton = { TextButton(onClick = { content = remote.str("content"); baseContent = remote.str("content"); baseVersion = remote.long("version"); conflict = null }) { Text(stringResource(Res.string.memory_conflict_theirs)) } },
        )
    }
    deleteConflict?.let { remote ->
        AlertDialog(
            onDismissRequest = { deleteConflict = null },
            title = { Text(stringResource(Res.string.memory_conflict_title)) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.memory_conflict, stringResource(Res.string.memory_profile_title), remote.long("version")))
                Text(remote.str("content"), style = MaterialTheme.typography.bodyMedium)
            } },
            confirmButton = {
                TextButton(onClick = {
                    deleteConflict = null
                    scope.launch {
                        runCatching { repository.callOnHost(hostId, "memory.user.delete", buildJsonObject { put("entry_id", entryId); put("expected_version", remote.long("version")) }) }
                            .onSuccess { onBack() }
                            .onFailure { error = if ((it as? NetworkError)?.code == "conflict") conflictError else loadingError }
                    }
                }) { Text(conflictDeleteMine, color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = {
                TextButton(onClick = {
                    content = remote.str("content")
                    baseVersion = remote.long("version")
                    deleteConflict = null
                }) { Text(conflictKeepLatest) }
            },
        )
    }
}

@Composable
fun MemoryProfileEditorScreen(repository: MobileRepository, field: String, onBack: () -> Unit) {
    val hostId = repository.activeHost.collectAsState().value?.id.orEmpty()
    val scope = rememberCoroutineScope()
    val maxLength = when (field) { "name" -> 20; "language" -> 35; else -> 100 }
    var value by rememberSaveable(hostId, field) { mutableStateOf("") }
    var useSystem by rememberSaveable(hostId, field) { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var conflict by remember { mutableStateOf<JsonObject?>(null) }
    var baseVersion by rememberSaveable(hostId, field) { mutableStateOf(0L) }
    var profileLoaded by rememberSaveable(hostId, field) { mutableStateOf(false) }
    val loadingError = stringResource(Res.string.feature_loading_error)
    LaunchedEffect(hostId, field) {
        runCatching {
            val result = repository.callOnHost(hostId, "memory.profile.get")
            val current = result.obj("profile")
            value = current.str(field)
            useSystem = field == "language" && value.isBlank()
            baseVersion = current.long("version")
            profileLoaded = true
        }.onFailure { error = loadingError }
    }
    val label = stringResource(when (field) {
        "name" -> Res.string.memory_profile_name
        "language" -> Res.string.memory_profile_language
        else -> Res.string.memory_profile_identity
    })
    fun save(expectedVersion: Long = baseVersion) {
        scope.launch {
            runCatching {
                repository.callOnHost(hostId, "memory.profile.update", buildJsonObject {
                    put("expected_version", expectedVersion)
                    put(field, if (field == "language" && useSystem) JsonNull else value.trim().takeIf(String::isNotBlank)?.let(::JsonPrimitive) ?: JsonNull)
                })
            }.onSuccess { onBack() }.onFailure { failure ->
                val remote = (failure as? NetworkError)?.details?.obj("details")?.obj("current")
                if ((failure as? NetworkError)?.code == "conflict" && remote != null) conflict = remote
                else error = loadingError
            }
        }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).imePadding()) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onBack) { Text(stringResource(Res.string.common_back)) }
                Text(label, Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                TextButton(onClick = { save() }, enabled = profileLoaded) { Text("✓", style = MaterialTheme.typography.titleLarge) }
            }
        }
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            OutlinedTextField(
                value = value,
                onValueChange = {
                    if (it.unicodeScalarCount() <= maxLength) {
                        value = it
                        if (field == "language") useSystem = false
                    }
                },
                modifier = Modifier.fillMaxWidth(),
                label = { Text(stringResource(when (field) { "name" -> Res.string.memory_profile_name_placeholder; "identity" -> Res.string.memory_profile_identity_placeholder; else -> Res.string.memory_profile_language })) },
                minLines = if (field == "identity") 3 else 1,
                maxLines = if (field == "identity") 4 else 1,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            )
            if (field == "language") {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = useSystem, onCheckedChange = { useSystem = it })
                    Text(stringResource(Res.string.memory_profile_language_system_option), Modifier.clickable { useSystem = !useSystem })
                }
            }
            Text(stringResource(Res.string.memory_profile_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }
    conflict?.let { remote ->
        AlertDialog(
            onDismissRequest = { conflict = null },
            title = { Text(stringResource(Res.string.memory_conflict_title)) },
            text = { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(Res.string.memory_conflict, stringResource(Res.string.memory_profile_title), remote.long("version")))
                Text(remote.str(field).ifBlank { stringResource(Res.string.memory_profile_language_system_option) })
            } },
            confirmButton = { TextButton(onClick = { val version = remote.long("version"); conflict = null; save(version) }) { Text(stringResource(Res.string.memory_conflict_mine)) } },
            dismissButton = { TextButton(onClick = { value = remote.str(field); useSystem = field == "language" && value.isBlank(); baseVersion = remote.long("version"); conflict = null }) { Text(stringResource(Res.string.memory_conflict_theirs)) } },
        )
    }
}

@Composable
private fun memoryEntrySourceText(entry: JsonObject, chats: List<JsonObject>, bots: List<JsonObject>): String {
    val source = entry.obj("source")
    val sender = source.obj("by")
    val chat = chats.firstOrNull { it.str("id") == source.str("chat_id") }
    val title = chat?.str("title").orEmpty().ifBlank { chat?.str("name").orEmpty() }
    val date = formatMemoryTimestamp(entry.str("updated_at"))
    val origin = when (sender.str("kind")) {
        "user" -> stringResource(Res.string.memory_source_user)
        "bot" -> stringResource(Res.string.memory_source_bot, bots.firstOrNull { it.str("id") == sender.str("bot_id") }?.str("name") ?: sender.str("bot_id"))
        else -> stringResource(Res.string.memory_source_curator)
    }
    return if (title.isBlank()) "$origin · $date" else stringResource(Res.string.memory_source_detail_with_origin, origin, title, date)
}
